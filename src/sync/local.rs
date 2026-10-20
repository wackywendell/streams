//! Local storage coordination with no Git or network dependency, stepped via CatalogProtocol.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Record, ValidationError, check_text};
use crate::operations::{Authority, DomainOutcome, Operation, Reconciliation, reconcile};
use crate::storage::{Snapshot, Store, Transaction};
use crate::yaml_store::YamlStore;

use super::ReadResult;
use super::envelope::{decode_operation, encode_operation};
use super::outcomes::{LocalStatus, Result as SyncResult, SharingStatus};
use super::protocol::ProtocolMachine;
use super::state::{RepairRequired, State, StateHook};

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalReceipts {
    creations: BTreeMap<String, String>,
}

/// Offline saves with durable retry receipts, without initializing Git or contacting a remote.
#[derive(Clone)]
pub struct LocalCatalog<S: Store = YamlStore> {
    store: S,
    host_id: String,
    hook: Option<StateHook>,
}

impl<S: Store + std::fmt::Debug> std::fmt::Debug for LocalCatalog<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalCatalog")
            .field("store", &self.store)
            .field("host_id", &self.host_id)
            .finish_non_exhaustive()
    }
}

/// Inspect recovery fences and creation evidence within the owning transaction.
pub(crate) fn ready(
    tx: &impl Transaction,
    state: &State<'_>,
) -> Result<BTreeMap<String, Authority>, RepairRequired> {
    state.check_save()?;
    tx.check_local()?;
    state.check_local()?;
    creations(state)
}

fn creations(state: &State<'_>) -> Result<BTreeMap<String, Authority>, RepairRequired> {
    let receipts = state
        .read_record::<LocalReceipts>("local.json")?
        .unwrap_or_default();
    let invalid =
        |msg: &str| RepairRequired::new(format!("invalid local creation receipts: {msg}"));
    let mut result = BTreeMap::new();
    for (stream_id, message) in &receipts.creations {
        let (operation, _) = decode_operation(message).map_err(|e| invalid(&e.to_string()))?;
        let Some((id, auth)) = operation.created_authority() else {
            return Err(invalid("invalid local creation receipt"));
        };
        if &id != stream_id {
            return Err(invalid("invalid local creation receipt"));
        }
        result.insert(stream_id.clone(), auth);
    }
    Ok(result)
}

impl<S: Store> LocalCatalog<S> {
    pub fn new(store: S, host_id: impl Into<String>) -> Result<Self, ValidationError> {
        let host_id = host_id.into();
        check_text(&host_id, "host_id", false)?;
        Ok(Self {
            store,
            host_id,
            hook: None,
        })
    }

    /// Install journal fault injection without changing the owning persistence scope.
    pub fn set_state_hook(&mut self, hook: StateHook) {
        self.hook = Some(hook);
    }

    /// Read under the catalog lock, refusing state that needs recovery before further use.
    pub fn snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.store.with_transaction(|tx| {
            let state = State::from_journal(tx.journal()).with_optional_hook(self.hook.clone());
            ready(tx, &state)?;
            Ok(tx.read()?)
        })
    }

    /// Recover ownership from the original durable stream-creation envelope.
    pub fn authority(&self, stream_id: &str) -> Result<Option<Authority>, RepairRequired> {
        self.store.with_transaction(|tx| {
            let state = State::from_journal(tx.journal()).with_optional_hook(self.hook.clone());
            let authorities = ready(tx, &state)?;
            let catalog = tx.read()?.catalog;
            Ok(if catalog.stream(stream_id).is_some() {
                authorities.get(stream_id).cloned()
            } else {
                None
            })
        })
    }

    /// Reconcile and persist locally before acknowledging acceptance.
    /// Durable receipts fence changed-intent retries; a repair outcome must not be blindly retried.
    pub fn save(
        &self,
        operation: &Operation,
        origin: Option<&BTreeMap<String, String>>,
    ) -> Result<SyncResult, ValidationError> {
        operation.check_producer(&self.host_id)?;
        let encoded = encode_operation(operation, origin)?;
        let outcome = self
            .store
            .with_transaction(|tx| -> Result<_, RepairRequired> {
                let state = State::from_journal(tx.journal()).with_optional_hook(self.hook.clone());
                let authorities = ready(tx, &state)?;
                let mut machine = ProtocolMachine::observe(&state, 1)?;
                let snapshot = tx.read()?;
                if state.seen(operation)? {
                    return Ok(
                        SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Unchanged)
                            .with_domain(Reconciliation::no_change(
                                operation.id(),
                                "operation already saved",
                            )),
                    );
                }
                let auth = operation.authority_for(&authorities);
                let result = reconcile(&snapshot.catalog, operation, auth);
                let Reconciliation::Changes { record, .. } = &result else {
                    if result.outcome() == DomainOutcome::NoChange {
                        state.remember(operation)?;
                    }
                    return Ok(
                        SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Unchanged)
                            .with_domain(result),
                    );
                };
                let witness = machine.begin_save(&state, operation, record, &snapshot, None)?;
                witness.run(&encoded, |witness| -> Result<(), RepairRequired> {
                    if operation.before().is_none()
                        && let Record::Stream(s) = operation.after()
                    {
                        let mut receipts = state
                            .read_record::<LocalReceipts>("local.json")?
                            .unwrap_or_default();
                        receipts.creations.insert(s.id.clone(), encoded.clone());
                        state.write_record("local.json", &receipts)?;
                    }
                    tx.save(witness.record(), witness.expected())?;
                    Ok(())
                })?;
                Ok(
                    SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Persisted)
                        .with_domain(result),
                )
            });

        match outcome {
            Ok(res) => Ok(res),
            Err(err) => Ok(
                SyncResult::new(SharingStatus::LocalOnly, LocalStatus::RepairRequired)
                    .with_detail(err.to_string()),
            ),
        }
    }

    /// Return the local view; refresh has no network meaning for this backend.
    pub fn read(&self, _refresh: bool) -> Result<ReadResult, RepairRequired> {
        let snapshot = self.snapshot()?;
        Ok(ReadResult {
            catalog: snapshot.catalog.clone(),
            local: snapshot,
            confirmed: None,
            freshness: super::Freshness::LocalOnly,
            fetched_at: None,
            conflicts: Vec::new(),
            detail: String::new(),
        })
    }

    /// Check local readiness without implying that any publication occurred.
    pub fn sync(&self, _event: super::policy::SyncEvent) -> SyncResult {
        match self.snapshot() {
            Ok(_) => SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Unchanged),
            Err(err) => SyncResult::new(SharingStatus::LocalOnly, LocalStatus::RepairRequired)
                .with_detail(err.to_string()),
        }
    }
}
