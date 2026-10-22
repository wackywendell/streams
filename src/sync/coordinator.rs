//! Catalog decisions over a locked journal and a semantic repository port.
//!
//! The production coordinator has no filesystem or subprocess implementation.
//! Repository adapters perform effects; tests supply observations and failures.

#[cfg(test)]
mod tests;

use super::envelope::{Origin, encode_operation};
use super::git_io::GitError;
use super::history::History;
use super::outcomes::{LocalStatus, Result as SyncResult, SharingStatus};
use super::policy::{Policy, SyncEvent, Timing};
use super::protocol::{
    ProtocolMachine,
    publication::{Recovery, RetainedAttempt},
};
use super::replay::{Replay, ReplayChange, replay};
use super::state::{
    GitConfig, PublicationAttempt, PublicationMarker, RepairRequired, State, SyncMetadata,
};
use super::{Freshness, Oid, ReadResult, Seconds};
use crate::model::{Record, ValidationError};
use crate::operations::{Authority, DomainOutcome, Operation, Reconciliation, reconcile};
use crate::storage::{Snapshot, StorageError};
use crate::yaml_store::YamlStore;
use std::collections::BTreeMap;

#[derive(Debug, thiserror::Error)]
pub(crate) enum CatalogError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Repair(#[from] RepairRequired),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Effects available only inside the owning catalog transaction.
/// Histories must be validated catalog histories, not arbitrary Git objects.
/// Failures do not imply rollback; in particular a failed push may have published.
pub(super) trait Repository {
    fn check_layout(&self) -> Result<(), CatalogError>;
    fn check_target(&self, config: &GitConfig) -> Result<(), RepairRequired>;
    fn clean(&self) -> Result<(), CatalogError>;
    fn head(&self) -> Result<String, CatalogError>;
    fn history(&self, tip: Option<&str>) -> Result<History, CatalogError>;
    fn ancestor(&self, before: &str, after: &str) -> Result<bool, CatalogError>;
    fn snapshot(&self) -> Result<Snapshot, CatalogError>;
    fn fetch(&self, branch: &str) -> Result<Option<String>, CatalogError>;
    fn candidate(
        &self,
        local: &History,
        remote: &History,
        changes: Vec<ReplayChange>,
    ) -> Result<String, CatalogError>;
    fn retain(&self, revision: &str) -> Result<(), CatalogError>;
    fn push(&self, candidate: &Oid, branch: &str) -> Result<(), CatalogError>;
    /// Move the checkout/ref and verify exact materialized records; does not manage recovery markers.
    fn checkout(&self, old: &Oid, new: &Oid) -> Result<(), CatalogError>;
    /// Persist the record and operation commit, checking the captured snapshot and old tip.
    fn commit_record(
        &self,
        record: &Record,
        expected: &Snapshot,
        start: Option<&str>,
        message: &str,
    ) -> Result<String, CatalogError>;
}

/// Borrowed with both ports under one serialization scope; permits cannot escape it.
pub(super) struct Coordinator<'tx> {
    pub(super) io: &'tx dyn Repository,
    pub(super) state: &'tx State<'tx>,
    pub(super) host_id: &'tx str,
    pub(super) policy: &'tx Policy,
    pub(super) clock: &'tx dyn Fn() -> f64,
}

fn repair(error: impl std::fmt::Display) -> RepairRequired {
    RepairRequired::new(error.to_string())
}

impl Coordinator<'_> {
    pub(super) fn now(&self) -> Result<Seconds, CatalogError> {
        Seconds::try_from((self.clock)()).map_err(|e| repair(e).into())
    }

    pub(super) fn config(&self) -> Result<GitConfig, RepairRequired> {
        let config = self
            .state
            .config()?
            .filter(|c| c.host() == self.host_id)
            .ok_or_else(|| repair("catalog is not initialized for this host"))?;
        self.io.check_target(&config)?;
        Ok(config)
    }

    pub(super) fn ready(&self) -> Result<GitConfig, CatalogError> {
        self.state.check_save()?;
        self.io.check_layout()?;
        let config = self.config()?;
        let metadata = self.state.metadata()?;
        let publication = self.state.publication()?;
        if config.remote().is_none()
            && (metadata != SyncMetadata::default() || publication.is_some())
        {
            return Err(repair(
                "local Git catalog has remote synchronization state; inspect before continuing",
            )
            .into());
        }
        if let Some(ref publication) = publication {
            let attempt = publication.attempt();
            if attempt.branch() != config.branch() || Some(attempt.remote()) != config.remote() {
                return Err(repair(
                    "publication marker does not match the configured remote and branch",
                )
                .into());
            }
        }
        if matches!(publication, Some(PublicationMarker::Adopting(_))) {
            return Err(repair(
                "interrupted local adoption; inspect publication.json and retained refs",
            )
            .into());
        }
        self.io.clean().map_err(repair)?;
        Ok(config)
    }

    pub(super) fn history(&self) -> Result<History, CatalogError> {
        let history = self.io.history(Some(&self.io.head()?))?;
        self.state
            .validate_fingerprints(history.fingerprints(), true)?;
        Ok(history)
    }

    pub(super) fn saved(&self) -> Result<(History, Snapshot), CatalogError> {
        let history = self.history()?;
        let snapshot = self.io.snapshot()?;
        if &snapshot.catalog != history.catalog() {
            return Err(repair("working records disagree with supported history").into());
        }
        Ok((history, snapshot))
    }

    pub(super) fn authorities(
        &self,
        local: &History,
    ) -> Result<BTreeMap<String, Authority>, CatalogError> {
        let remote = self
            .io
            .history(self.state.metadata()?.confirmed.as_ref().map(Oid::as_str))?;
        let mut authorities = local.authorities().clone();
        authorities.extend(remote.authorities().clone());
        Ok(authorities)
    }

    pub(super) fn fetch(&self) -> Result<Option<String>, CatalogError> {
        let config = self.config()?;
        if config.remote().is_none() {
            return Err(repair("local Git catalog has no remote to fetch").into());
        }
        let tip = self.io.fetch(config.branch())?;
        let history = self.io.history(tip.as_deref())?;
        let mut metadata = self.state.metadata()?;
        if let Some(previous) = &metadata.confirmed
            && (tip.is_none()
                || !self
                    .io
                    .ancestor(previous.as_str(), tip.as_deref().unwrap())?)
        {
            return Err(repair("confirmed remote history was removed or replaced").into());
        }
        if tip.is_some() {
            self.state
                .validate_fingerprints(history.fingerprints(), true)?;
        }
        metadata.confirmed = tip.clone().map(Oid::try_from).transpose().map_err(repair)?;
        metadata.fetched_at = Some(self.now()?);
        metadata.refresh_retry_after = Seconds::ZERO;
        metadata.refresh_error.clear();
        self.state.write_metadata(&metadata)?;
        Ok(tip)
    }

    pub(super) fn adopt(
        &self,
        old: &Oid,
        new: &Oid,
        marker: &PublicationAttempt,
    ) -> Result<(), CatalogError> {
        self.state
            .fence_adoption(&marker.adopting(old.clone(), new.clone()))?;
        self.io.checkout(old, new)?;
        self.state.clear("publication.json")?;
        Ok(())
    }

    pub(super) fn record_confirmation(
        &self,
        marker: &PublicationAttempt,
    ) -> Result<PublicationAttempt, CatalogError> {
        let mut metadata = self.state.metadata()?;
        if let Some(timestamp) = marker.confirmed_at() {
            if metadata.last_confirmed_attempt.as_ref() != Some(marker.candidate())
                || metadata.last_push != Some(timestamp)
            {
                return Err(repair(
                    "publication confirmation disagrees with its scheduling receipt",
                )
                .into());
            }
            return Ok(marker.clone());
        }
        if metadata.last_confirmed_attempt.as_ref() != Some(marker.candidate()) {
            metadata.last_push = Some(self.now()?);
            metadata.last_confirmed_attempt = Some(marker.candidate().clone());
            self.state.write_metadata(&metadata)?;
        }
        let confirmed = marker.clone().with_confirmed_at(metadata.last_push);
        self.state
            .write_publication(&PublicationMarker::Attempting(confirmed.clone()))?;
        Ok(confirmed)
    }

    pub(super) fn success(&self) -> Result<(), CatalogError> {
        let mut metadata = self.state.metadata()?;
        metadata.last_sync = Some(self.now()?);
        metadata.retry_after = Seconds::ZERO;
        self.state.write_metadata(&metadata)?;
        Ok(())
    }

    pub(super) fn failure(&self) -> Result<(), CatalogError> {
        let mut metadata = self.state.metadata()?;
        metadata.retry_after =
            Seconds::try_from(self.now()?.get() + self.policy.cooldown_seconds())
                .map_err(repair)?;
        self.state.write_metadata(&metadata)?;
        Ok(())
    }

    pub fn save(
        &self,
        operation: &Operation,
        origin: Option<&Origin>,
    ) -> Result<SyncResult, ValidationError> {
        operation.check_producer(self.host_id)?;
        let mut origin = origin.cloned().unwrap_or_default();
        origin
            .entry("host".into())
            .or_insert_with(|| self.host_id.to_owned());
        encode_operation(operation, Some(&origin))?;
        let mut sharing = SharingStatus::Blocked;
        let catalog = self;
        let outcome: Result<_, CatalogError> = (|| {
            let config = catalog.state.config()?;
            sharing = if config.is_some_and(|c| c.remote().is_none()) {
                SharingStatus::LocalOnly
            } else {
                SharingStatus::Pending
            };
            catalog.ready()?;
            let (history, snapshot) = catalog.saved()?;
            let mut machine = ProtocolMachine::observe(catalog.state, 1)?;
            let previous = history.operation(operation.id());
            let remembered = catalog.state.seen(operation)?;
            if let Some(previous) = previous
                && &previous.operation != operation
            {
                return Err(repair(format!(
                    "operation ID reused with different intent: {}",
                    operation.id()
                ))
                .into());
            }
            if previous.is_some() || remembered {
                return Ok(SyncResult::new(sharing, LocalStatus::Unchanged)
                    .with_domain(Reconciliation::no_change(
                        operation.id(),
                        "operation already saved",
                    ))
                    .with_local_rev(history.tip().map(str::to_owned)));
            }
            let authorities = catalog.authorities(&history)?;
            let auth = operation.authority_for(&authorities);
            let result = reconcile(&snapshot.catalog, operation, auth);
            let Reconciliation::Changes { record, .. } = &result else {
                if result.outcome() == DomainOutcome::NoChange {
                    catalog.state.remember(operation)?;
                }
                return Ok(SyncResult::new(sharing, LocalStatus::Unchanged)
                    .with_domain(result)
                    .with_local_rev(history.tip().map(str::to_owned)));
            };
            let message = encode_operation(operation, Some(&origin))?;
            let permit =
                machine.begin_save(catalog.state, operation, record, &snapshot, history.tip())?;
            let commit = permit.run(&message, |permit| -> Result<String, CatalogError> {
                catalog.io.commit_record(
                    permit.record(),
                    permit.expected(),
                    permit.start_tip(),
                    &message,
                )
            })?;
            Ok(SyncResult::new(sharing, LocalStatus::Committed)
                .with_domain(result)
                .with_local_rev(Some(commit)))
        })();
        Ok(match outcome {
            Ok(result) => result,
            Err(error) => SyncResult::new(
                if sharing == SharingStatus::LocalOnly {
                    sharing
                } else {
                    SharingStatus::Blocked
                },
                LocalStatus::RepairRequired,
            )
            .with_detail(error.to_string()),
        })
    }

    pub fn sync(&self, event: SyncEvent) -> SyncResult {
        let mut sharing = SharingStatus::Blocked;
        let mut had_publication = false;
        let catalog = self;
        let outcome = {
            let outcome = (|| {
                sharing = if catalog
                    .state
                    .config()?
                    .is_some_and(|c| c.remote().is_none())
                {
                    SharingStatus::LocalOnly
                } else {
                    SharingStatus::Pending
                };
                catalog.ready()?;
                let (local, _) = catalog.saved()?;
                if sharing == SharingStatus::LocalOnly {
                    return Ok(SyncResult::new(sharing, LocalStatus::Unchanged)
                        .with_local_rev(local.tip().map(str::to_owned)));
                }
                let metadata = catalog.state.metadata()?;
                let known = catalog
                    .io
                    .history(metadata.confirmed.as_ref().map(Oid::as_str))?;
                let pending_stream = local.operations().iter().any(|entry| {
                    matches!(entry.operation.after(), crate::model::Record::Stream(_))
                        && !known.has_operation(entry.operation.id())
                });
                let timing =
                    Timing::new(metadata.last_sync, metadata.last_push, metadata.retry_after);
                if !catalog
                    .policy
                    .due(event, timing, catalog.now()?, pending_stream)
                {
                    return Ok(
                        SyncResult::new(SharingStatus::Pending, LocalStatus::Unchanged)
                            .with_local_rev(local.tip().map(str::to_owned))
                            .with_remote_rev(known.tip().map(str::to_owned)),
                    );
                }
                let mut machine =
                    ProtocolMachine::observe(catalog.state, catalog.policy.attempts())?;
                let mut remote = catalog.io.history(catalog.fetch()?.as_deref())?;
                let mut retained = machine.inspect_retained(catalog, local.tip(), &remote)?;
                had_publication |= matches!(retained, Some(RetainedAttempt::Confirmed(_)));
                loop {
                    let candidate = if let Some(candidate) = retained
                        .as_ref()
                        .and_then(|retained| retained.reusable_candidate(&local, &remote))
                    {
                        candidate.to_owned()
                    } else {
                        match replay(&local, &remote)? {
                            Replay::Ready { changes, .. } => {
                                catalog.io.candidate(&local, &remote, changes)?
                            }
                            Replay::Blocked(conflict) => {
                                return Ok(SyncResult::new(
                                    SharingStatus::Blocked,
                                    LocalStatus::Unchanged,
                                )
                                .with_local_rev(local.tip().map(str::to_owned))
                                .with_remote_rev(remote.tip().map(str::to_owned))
                                .with_conflicts(vec![conflict])
                                .with_detail("domain conflict blocks the outgoing batch"));
                            }
                        }
                    };
                    if Some(candidate.as_str()) == remote.tip() {
                        let permit = machine.begin_remote_adoption(
                            catalog,
                            &local,
                            &remote,
                            match retained.take() {
                                Some(RetainedAttempt::Confirmed(receipt)) => Some(receipt),
                                _ => None,
                            },
                        )?;
                        return match permit.apply() {
                            Ok(revision) => {
                                catalog.success()?;
                                Ok(SyncResult::new(
                                    SharingStatus::Confirmed,
                                    LocalStatus::Unchanged,
                                )
                                .with_local_rev(Some(revision))
                                .with_remote_rev(remote.tip().map(str::to_owned)))
                            }
                            Err(error) => Ok(SyncResult::new(
                                if had_publication {
                                    SharingStatus::Confirmed
                                } else {
                                    SharingStatus::Blocked
                                },
                                LocalStatus::RepairRequired,
                            )
                            .with_local_rev(local.tip().map(str::to_owned))
                            .with_remote_rev(remote.tip().map(str::to_owned))
                            .with_detail(error.to_string())),
                        };
                    }
                    // A new batch has its own publication receipt; prior confirmation
                    // still affects sharing, but cannot authorize this attempt's adoption.
                    let Some(mut permit) = machine.prepare_push(
                        catalog,
                        &local,
                        &remote,
                        &candidate,
                        retained.take(),
                    )?
                    else {
                        catalog.failure()?;
                        return Ok(
                            SyncResult::new(SharingStatus::Failed, LocalStatus::Unchanged)
                                .with_local_rev(local.tip().map(str::to_owned))
                                .with_remote_rev(remote.tip().map(str::to_owned))
                                .with_detail("competing-push retry budget exhausted"),
                        );
                    };
                    match permit.push() {
                        Ok(()) => {
                            had_publication = true;
                            let adoption = permit.acknowledge()?.record_confirmation()?;
                            return match adoption.apply() {
                                Ok(revision) => {
                                    catalog.success()?;
                                    Ok(SyncResult::new(
                                        SharingStatus::Confirmed,
                                        LocalStatus::Unchanged,
                                    )
                                    .with_local_rev(Some(revision))
                                    .with_remote_rev(Some(candidate)))
                                }
                                Err(error) => Ok(SyncResult::new(
                                    SharingStatus::Confirmed,
                                    LocalStatus::RepairRequired,
                                )
                                .with_local_rev(local.tip().map(str::to_owned))
                                .with_remote_rev(Some(candidate))
                                .with_detail(error.to_string())),
                            };
                        }
                        Err(CatalogError::Git(push_error)) => {
                            let observed = match catalog.fetch() {
                                Ok(tip) => catalog.io.history(tip.as_deref())?,
                                Err(CatalogError::Git(_)) => {
                                    permit.leave()?;
                                    catalog.failure()?;
                                    return Ok(SyncResult::new(
                                        SharingStatus::Uncertain,
                                        LocalStatus::Unchanged,
                                    )
                                    .with_local_rev(local.tip().map(str::to_owned))
                                    .with_detail(push_error.to_string()));
                                }
                                Err(error) => {
                                    permit.leave()?;
                                    return Err(error);
                                }
                            };
                            match permit.recover(&observed)? {
                                Recovery::Confirmed(witness) => {
                                    had_publication = true;
                                    retained = Some(RetainedAttempt::Confirmed(witness.record()?));
                                    remote = observed;
                                    // Confirmation returns Ready, not Adopting: replay again
                                    // against the fetched tip before any checkout mutation.
                                }
                                Recovery::Unconfirmed(permit) => {
                                    if observed.tip() != remote.tip() && permit.has_push_budget() {
                                        retained = permit.retry(&observed)?;
                                        had_publication |=
                                            matches!(retained, Some(RetainedAttempt::Confirmed(_)));
                                        remote = observed;
                                    } else {
                                        permit.leave()?;
                                        catalog.failure()?;
                                        return Ok(SyncResult::new(
                                            SharingStatus::Failed,
                                            LocalStatus::Unchanged,
                                        )
                                        .with_local_rev(local.tip().map(str::to_owned))
                                        .with_remote_rev(observed.tip().map(str::to_owned))
                                        .with_detail(push_error.to_string()));
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            permit.leave()?;
                            return Err(error);
                        }
                    }
                }
            })();
            if let Err(ref error) = outcome
                && sharing != SharingStatus::LocalOnly
                && !matches!(error, CatalogError::Repair(_))
            {
                let _ = catalog.failure();
            }
            outcome
        };
        match outcome {
            Ok(result) => result,
            Err(error) => {
                let (sharing, local) = if sharing == SharingStatus::LocalOnly {
                    (sharing, LocalStatus::RepairRequired)
                } else if matches!(error, CatalogError::Repair(_)) {
                    (SharingStatus::Blocked, LocalStatus::RepairRequired)
                } else {
                    (SharingStatus::Failed, LocalStatus::Unchanged)
                };
                SyncResult::new(sharing, local).with_detail(error.to_string())
            }
        }
    }
    pub fn snapshot(&self) -> Result<Snapshot, CatalogError> {
        self.ready()?;
        Ok(self.saved()?.1)
    }

    pub(super) fn inspect(&self) -> Result<super::Inspection, CatalogError> {
        self.ready()?;
        let history = self.io.history(Some(&self.io.head()?))?;
        self.state
            .validate_fingerprints(history.fingerprints(), false)?;
        let snapshot = self.io.snapshot()?;
        if &snapshot.catalog != history.catalog() {
            return Err(repair("working records disagree with supported history").into());
        }
        Ok(super::Inspection {
            snapshot,
            authorities: self.authorities(&history)?,
        })
    }

    pub fn authority(&self, stream: &str) -> Result<Option<Authority>, CatalogError> {
        self.ready()?;
        Ok(self.authorities(&self.history()?)?.get(stream).cloned())
    }

    pub fn read(&self, refresh: bool) -> Result<ReadResult, CatalogError> {
        let config = self.ready()?;
        let (local, snapshot) = self.saved()?;
        if config.remote().is_none() {
            return Ok(ReadResult {
                catalog: snapshot.catalog.clone(),
                local: snapshot,
                confirmed: None,
                freshness: Freshness::LocalOnly,
                fetched_at: None,
                conflicts: Vec::new(),
                detail: String::new(),
            });
        }
        let metadata = self.state.metadata()?;
        let mut freshness = Freshness::Cached;
        if self.policy.read_due(
            metadata.fetched_at,
            self.now()?,
            metadata.refresh_retry_after,
            refresh,
        ) {
            match self.fetch() {
                Ok(_) => freshness = Freshness::Refreshed,
                Err(CatalogError::Git(error)) => {
                    let mut metadata = self.state.metadata()?;
                    metadata.refresh_retry_after =
                        Seconds::try_from(self.now()?.get() + self.policy.cooldown_seconds())
                            .map_err(repair)?;
                    metadata.refresh_error = error.to_string();
                    self.state.write_metadata(&metadata)?;
                    freshness = Freshness::Failed;
                }
                Err(error) => return Err(error),
            }
        }
        let metadata = self.state.metadata()?;
        let remote = self
            .io
            .history(metadata.confirmed.as_ref().map(Oid::as_str))?;
        let (projected, conflicts) = match replay(&local, &remote)? {
            Replay::Ready { catalog, .. } => (catalog, Vec::new()),
            Replay::Blocked(conflict) => (snapshot.catalog.clone(), vec![conflict]),
        };
        let confirmed = if metadata.fetched_at.is_some() {
            Some(YamlStore::decode_snapshot(remote.files())?)
        } else {
            None
        };
        Ok(ReadResult {
            catalog: projected,
            local: snapshot,
            confirmed,
            freshness,
            fetched_at: metadata.fetched_at.map(Seconds::get),
            conflicts,
            detail: metadata.refresh_error,
        })
    }

    pub fn inspect_remote(&self) -> Result<Snapshot, CatalogError> {
        if self.ready()?.remote().is_none() {
            return Err(repair("local Git catalog has no remote to inspect").into());
        }
        let tip = self.fetch()?;
        Ok(YamlStore::decode_snapshot(
            self.io.history(tip.as_deref())?.files(),
        )?)
    }

    pub fn confirmed_snapshot(&self) -> Result<Snapshot, CatalogError> {
        let tip = self.state.metadata()?.confirmed;
        Ok(YamlStore::decode_snapshot(
            self.io.history(tip.as_ref().map(Oid::as_str))?.files(),
        )?)
    }
}
