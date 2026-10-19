//! Local file-store coordination with no Git or network dependency, stepped via CatalogProtocol.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use serde::{Deserialize, Serialize};

use crate::model::{Record, ValidationError, check_text};
use crate::operations::{Authority, Operation, Reconciliation, reconcile};
use crate::storage::{Snapshot, StorageError};
use crate::yaml_store::YamlStore;

use super::ReadResult;
use super::envelope::{decode_operation, encode_operation};
use super::outcomes::{LocalStatus, Result as SyncResult, SharingStatus};
use super::protocol::ProtocolMachine;
use super::state::{RepairRequired, State};

pub fn check_producer(operation: &Operation, host_id: &str) -> Result<(), ValidationError> {
    match &operation.after {
        Record::Session(s) => {
            if s.host_id != host_id {
                return Err(ValidationError::new(
                    "cannot write another host's producer-owned record",
                ));
            }
        }
        Record::Host(h) => {
            if h.id != host_id {
                return Err(ValidationError::new(
                    "cannot write another host's producer-owned record",
                ));
            }
        }
        Record::Stream(s) => {
            if (operation.before.is_none() || operation.actor == "automatic")
                && s.owner_host_id != host_id
            {
                return Err(ValidationError::new(
                    "stream creation and inference require the owning host",
                ));
            }
        }
    }
    Ok(())
}

pub fn authority_for(
    operation: &Operation,
    authorities: &BTreeMap<String, Authority>,
) -> Option<Authority> {
    match &operation.after {
        Record::Session(s) => authorities.get(&s.stream_id).cloned(),
        Record::Stream(s) => authorities.get(&s.id).cloned(),
        Record::Host(_) => None,
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalReceipts {
    creations: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct SaveMarker<'a> {
    operation: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    start: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct LocalCatalog {
    pub store: YamlStore,
    pub host_id: String,
    pub state: State,
}

impl LocalCatalog {
    pub fn new(store: YamlStore, host_id: impl Into<String>) -> Result<Self, ValidationError> {
        let host_id = host_id.into();
        check_text(&host_id, "host_id", false)?;
        let state = State::new(&store.root);
        Ok(Self {
            store,
            host_id,
            state,
        })
    }

    pub fn ready(&self) -> Result<BTreeMap<String, Authority>, RepairRequired> {
        self.state.check_save()?;
        let allowed: BTreeSet<&str> = [
            ".catalog.lock",
            ".catalog-sync",
            ".catalog-staging",
            "streams",
            "sessions",
            "hosts",
        ]
        .into();
        for entry in fs::read_dir(&self.store.root)
            .map_err(|e| RepairRequired::new(format!("cannot read catalog root: {e}")))?
        {
            let entry = entry.map_err(|e| RepairRequired::new(e.to_string()))?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !allowed.contains(name_str.as_ref()) {
                return Err(RepairRequired::new(
                    "unexpected contents in local catalog; inspect before continuing",
                ));
            }
        }
        self.state.check_local()?;
        self.creations()
    }

    fn creations(&self) -> Result<BTreeMap<String, Authority>, RepairRequired> {
        let receipts = self
            .state
            .read_record::<LocalReceipts>("local.json")?
            .unwrap_or_default();
        let invalid =
            |msg: &str| RepairRequired::new(format!("invalid local creation receipts: {msg}"));
        let mut result = BTreeMap::new();
        for (stream_id, message) in &receipts.creations {
            let (operation, _) = decode_operation(message).map_err(|e| invalid(&e.to_string()))?;
            let Record::Stream(ref stream) = operation.after else {
                return Err(invalid("invalid local creation receipt"));
            };
            if operation.before.is_some() || &stream.id != stream_id {
                return Err(invalid("invalid local creation receipt"));
            }
            let auth = Authority::new(&stream.owner_host_id, &operation.id)
                .map_err(|e| invalid(&e.to_string()))?;
            result.insert(stream_id.clone(), auth);
        }
        Ok(result)
    }

    pub fn snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.store
            .with_transaction(|tx| {
                self.ready()
                    .map_err(|e| StorageError::new(e.to_string(), false))?;
                tx.read()
            })
            .map_err(|e| RepairRequired::new(e.to_string()))
    }

    pub fn authority(&self, stream_id: &str) -> Result<Option<Authority>, RepairRequired> {
        self.store
            .with_transaction(|tx| {
                let authorities = self
                    .ready()
                    .map_err(|e| StorageError::new(e.to_string(), false))?;
                let catalog = tx.read()?.catalog;
                Ok(if catalog.stream(stream_id).is_some() {
                    authorities.get(stream_id).cloned()
                } else {
                    None
                })
            })
            .map_err(|e| RepairRequired::new(e.to_string()))
    }

    pub fn save(
        &self,
        operation: &Operation,
        origin: Option<&BTreeMap<String, String>>,
    ) -> Result<SyncResult, ValidationError> {
        check_producer(operation, &self.host_id)?;
        let encoded = encode_operation(operation, origin)?;
        let outcome = self.store.with_transaction(|tx| {
            let authorities = self
                .ready()
                .map_err(|e| StorageError::new(e.to_string(), false))?;
            let mut machine = ProtocolMachine::observe(&self.state, 1)
                .map_err(|e| StorageError::new(e.to_string(), false))?;
            let snapshot = tx.read()?;
            if self
                .state
                .seen(operation)
                .map_err(|e| StorageError::new(e.to_string(), false))?
            {
                return Ok(
                    SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Unchanged).with_domain(
                        Reconciliation::no_change(&operation.id, "operation already saved"),
                    ),
                );
            }
            let auth = authority_for(operation, &authorities);
            let result = reconcile(&snapshot.catalog, operation, auth.as_ref());
            if result.outcome != "changes" {
                if result.outcome == "no-change" {
                    self.state
                        .remember(operation)
                        .map_err(|e| StorageError::new(e.to_string(), false))?;
                }
                return Ok(
                    SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Unchanged)
                        .with_domain(result),
                );
            }
            let witness = machine
                .begin_save(operation, result.record.as_ref().unwrap(), &snapshot, None)
                .map_err(|e| StorageError::new(e.to_string(), false))?;
            let save_marker = SaveMarker {
                operation: &encoded,
                start: witness.start_tip(),
            };
            if let Err(e) = self.state.write_record("save.json", &save_marker) {
                let _ = witness.fail();
                return Err(StorageError::new(e.to_string(), false));
            }
            let step_res = (|| -> Result<(), StorageError> {
                if operation.before.is_none()
                    && let Record::Stream(ref s) = operation.after
                {
                    let mut receipts = self
                        .state
                        .read_record::<LocalReceipts>("local.json")
                        .map_err(|e| StorageError::new(e.to_string(), false))?
                        .unwrap_or_default();
                    receipts.creations.insert(s.id.clone(), encoded.clone());
                    self.state
                        .write_record("local.json", &receipts)
                        .map_err(|e| StorageError::new(e.to_string(), false))?;
                }
                tx.save(witness.record(), witness.expected())?;
                self.state
                    .remember(witness.operation())
                    .map_err(|e| StorageError::new(e.to_string(), false))?;
                self.state
                    .clear("save.json")
                    .map_err(|e| StorageError::new(e.to_string(), false))?;
                Ok(())
            })();
            if let Err(err) = step_res {
                let _ = witness.fail();
                return Err(err);
            }
            witness
                .complete()
                .map_err(|e| StorageError::new(e.to_string(), false))?;
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

    pub fn read(&self, _refresh: bool) -> Result<ReadResult, RepairRequired> {
        let snapshot = self.snapshot()?;
        Ok(ReadResult {
            catalog: snapshot.catalog.clone(),
            local: snapshot,
            confirmed: None,
            freshness: "local-only".to_string(),
            fetched_at: None,
            conflicts: Vec::new(),
            detail: String::new(),
        })
    }

    pub fn sync(&self, _event: &str) -> SyncResult {
        match self.snapshot() {
            Ok(_) => SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Unchanged),
            Err(err) => SyncResult::new(SharingStatus::LocalOnly, LocalStatus::RepairRequired)
                .with_detail(err.to_string()),
        }
    }
}
