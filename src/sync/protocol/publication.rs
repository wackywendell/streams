//! Publication permits retain the exact catalog and identities inspected before effects.

use super::{Action, ProtocolMachine};
use crate::sync::{
    git::{CatalogError, GitCatalog},
    history::{History, read_history, records},
    replay::replay,
    state::{GitConfig, PublicationAttempt, PublicationMarker, RepairRequired},
};

fn invalid(message: &str) -> RepairRequired {
    RepairRequired::new(message)
}

fn inspect(catalog: &GitCatalog, local: &History) -> Result<GitConfig, CatalogError> {
    let config = catalog.ready()?;
    if Some(catalog.git.head()?).as_ref() != local.tip.as_ref() {
        return Err(invalid("local tip changed since publication inspection").into());
    }
    Ok(config)
}

fn validate_candidate(
    catalog: &GitCatalog,
    candidate: &str,
    local: &History,
    remote: &History,
) -> Result<History, CatalogError> {
    let history = read_history(&catalog.git, Some(candidate))?;
    let projected = replay(local, remote)?;
    let base = remote
        .tip
        .as_ref()
        .or(local.root.as_ref())
        .ok_or_else(|| invalid("candidate has no supported base"))?;
    let actual = history
        .operations
        .iter()
        .filter(|entry| !remote.has_operation(&entry.operation.id))
        .map(|entry| (&entry.operation, &entry.origin));
    let expected = projected
        .changes
        .iter()
        .map(|change| (&change.operation, &change.origin));
    if !projected.conflicts.is_empty()
        || !actual.eq(expected)
        || records(&history.catalog) != records(&projected.catalog)
        || !catalog.git.ancestor(base, candidate)?
    {
        return Err(invalid("candidate does not match the inspected publication batch").into());
    }
    Ok(history)
}

// Validate a retained attempt against its original base and local tip, not the
// newer fetched history: additional local saves must not invalidate confirmation.
fn validate_attempt(
    catalog: &GitCatalog,
    attempt: &PublicationAttempt,
    local: &History,
) -> Result<History, CatalogError> {
    let config = inspect(catalog, local)?;
    if config.branch != attempt.branch || config.remote.as_ref() != Some(&attempt.remote) {
        return Err(invalid("publication target changed since inspection").into());
    }
    let original = read_history(&catalog.git, Some(&attempt.local_tip))?;
    if !catalog
        .git
        .ancestor(&attempt.local_tip, local.tip.as_deref().unwrap())?
    {
        return Err(invalid("retained publication has unrelated local history").into());
    }
    let base = read_history(&catalog.git, attempt.base.as_deref())?;
    let candidate = validate_candidate(catalog, &attempt.candidate, &original, &base)?;
    let operations: Vec<_> = candidate
        .operations
        .iter()
        .filter(|entry| !base.has_operation(&entry.operation.id))
        .map(|entry| entry.operation.id.clone())
        .collect();
    if operations != attempt.operations {
        return Err(invalid("retained publication operations were changed").into());
    }
    Ok(candidate)
}

fn is_confirmed(
    catalog: &GitCatalog,
    attempt: &PublicationAttempt,
    candidate: &History,
    remote: &History,
) -> Result<bool, CatalogError> {
    if let Some(tip) = &remote.tip
        && catalog.git.ancestor(&attempt.candidate, tip)?
    {
        return Ok(true);
    }
    Ok(!attempt.operations.is_empty()
        && attempt.operations.iter().all(|id| {
            match (remote.operation(id), candidate.operation(id)) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        }))
}

pub(crate) struct ConfirmedAttemptWitness<'catalog> {
    catalog: &'catalog GitCatalog,
    attempt: PublicationAttempt,
    remote_tip: Option<String>,
}

/// Confirmation whose scheduling baseline and exact attempt are durably recorded.
pub(crate) struct RecordedConfirmation<'catalog>(ConfirmedAttemptWitness<'catalog>);

impl<'catalog> ConfirmedAttemptWitness<'catalog> {
    pub(crate) fn record(mut self) -> Result<RecordedConfirmation<'catalog>, CatalogError> {
        // Confirmation is tied to this candidate and this validated remote observation.
        if self.catalog.state.metadata()?.confirmed != self.remote_tip {
            return Err(invalid("confirmed remote observation changed before recording").into());
        }
        self.attempt = self.catalog.record_confirmation(&self.attempt)?;
        Ok(RecordedConfirmation(self))
    }
}

pub(crate) struct CandidateReuseWitness<'catalog> {
    catalog: &'catalog GitCatalog,
    attempt: PublicationAttempt,
}

pub(crate) struct PushPermit<'machine, 'catalog> {
    machine: &'machine mut ProtocolMachine,
    catalog: &'catalog GitCatalog,
    attempt: PublicationAttempt,
    acknowledged: bool,
}

pub(crate) enum Recovery<'machine, 'catalog> {
    Confirmed(ConfirmedAttemptWitness<'catalog>),
    Unconfirmed(PushPermit<'machine, 'catalog>),
}

pub(crate) struct AdoptionPermit<'machine, 'catalog> {
    machine: &'machine mut ProtocolMachine,
    catalog: &'catalog GitCatalog,
    old: String,
    new: String,
    attempt: PublicationAttempt,
}

impl ProtocolMachine {
    pub(crate) fn recover_before_push<'catalog>(
        &mut self,
        catalog: &'catalog GitCatalog,
        attempt: &PublicationAttempt,
        local: &History,
        remote: &History,
    ) -> Result<Option<ConfirmedAttemptWitness<'catalog>>, CatalogError> {
        let candidate = validate_attempt(catalog, attempt, local)?;
        if !is_confirmed(catalog, attempt, &candidate, remote)? {
            return Ok(None);
        }
        self.step(Action::RecoverConfirmedBeforePush)?;
        Ok(Some(ConfirmedAttemptWitness {
            catalog,
            attempt: attempt.clone(),
            remote_tip: remote.tip.clone(),
        }))
    }

    pub(crate) fn reusable_candidate<'catalog>(
        &self,
        catalog: &'catalog GitCatalog,
        attempt: &PublicationAttempt,
        local: &History,
        remote: &History,
    ) -> Result<Option<CandidateReuseWitness<'catalog>>, CatalogError> {
        validate_attempt(catalog, attempt, local)?;
        if attempt.base != remote.tip || Some(&attempt.local_tip) != local.tip.as_ref() {
            return Ok(None);
        }
        Ok(Some(CandidateReuseWitness {
            catalog,
            attempt: attempt.clone(),
        }))
    }

    pub(crate) fn prepare_push<'machine, 'catalog>(
        &'machine mut self,
        catalog: &'catalog GitCatalog,
        local: &History,
        remote: &History,
        candidate: &str,
        reuse: Option<CandidateReuseWitness<'catalog>>,
    ) -> Result<PushPermit<'machine, 'catalog>, CatalogError> {
        let config = inspect(catalog, local)?;
        // A retained attempt must be checked for confirmation before replacement.
        if let Some(marker) = catalog.state.publication()? {
            let old = marker.attempt();
            let prior = validate_attempt(catalog, old, local)?;
            if is_confirmed(catalog, old, &prior, remote)? && old.confirmed_at.is_none() {
                return Err(invalid(
                    "prior publication confirmation must be recorded before pushing",
                )
                .into());
            }
        }
        let history = validate_candidate(catalog, candidate, local, remote)?;
        let attempt = PublicationAttempt::new(
            remote.tip.clone(),
            candidate,
            local
                .tip
                .as_deref()
                .ok_or_else(|| invalid("local catalog has no tip"))?,
            config.branch,
            config
                .remote
                .ok_or_else(|| invalid("publication requires a remote"))?,
            history
                .operations
                .iter()
                .filter(|entry| !remote.has_operation(&entry.operation.id))
                .map(|entry| entry.operation.id.clone())
                .collect(),
            None,
        )
        .map_err(RepairRequired::new)?;
        let action = if let Some(reuse) = reuse {
            if !std::ptr::eq(reuse.catalog, catalog) || reuse.attempt != attempt {
                return Err(invalid("candidate reuse identity changed").into());
            }
            self.context
                .booleans
                .insert("candidate_reusable".into(), true);
            Action::ReuseExistingCandidate
        } else {
            Action::PrepareFreshCandidate
        };
        self.step(action)?;
        Ok(PushPermit {
            machine: self,
            catalog,
            attempt,
            acknowledged: false,
        })
    }

    pub(crate) fn begin_remote_adoption<'machine, 'catalog>(
        &'machine mut self,
        catalog: &'catalog GitCatalog,
        local: &History,
        remote: &History,
        confirmed: Option<RecordedConfirmation<'catalog>>,
    ) -> Result<AdoptionPermit<'machine, 'catalog>, CatalogError> {
        let config = inspect(catalog, local)?;
        let new = remote
            .tip
            .as_deref()
            .ok_or_else(|| invalid("cannot adopt an unpublished root"))?;
        validate_candidate(catalog, new, local, remote)?;
        let old = local
            .tip
            .as_deref()
            .ok_or_else(|| invalid("local catalog has no tip"))?;
        let attempt = if let Some(RecordedConfirmation(witness)) = confirmed {
            if !std::ptr::eq(witness.catalog, catalog) || witness.remote_tip != remote.tip {
                return Err(invalid("recorded confirmation belongs to another observation").into());
            }
            let candidate = validate_attempt(catalog, &witness.attempt, local)?;
            if !is_confirmed(catalog, &witness.attempt, &candidate, remote)?
                || catalog
                    .state
                    .publication()?
                    .as_ref()
                    .map(|marker| marker.attempt())
                    != Some(&witness.attempt)
            {
                return Err(invalid("recorded confirmation changed before adoption").into());
            }
            // The receipt describes the published batch, even when adopting a
            // newer descendant. The actual checkout transition is recorded separately.
            witness.attempt
        } else {
            PublicationAttempt::new(
                remote.tip.clone(),
                new,
                old,
                config.branch,
                config
                    .remote
                    .ok_or_else(|| invalid("adoption requires a remote"))?,
                vec![],
                None,
            )
            .map_err(RepairRequired::new)?
        };
        self.step(Action::NoPushNeededBeginAdopt)?;
        Ok(AdoptionPermit {
            machine: self,
            catalog,
            old: old.into(),
            new: new.into(),
            attempt,
        })
    }
}

impl<'machine, 'catalog> PushPermit<'machine, 'catalog> {
    pub(crate) fn push(&mut self) -> Result<(), CatalogError> {
        let local = read_history(&self.catalog.git, Some(&self.attempt.local_tip))?;
        validate_attempt(self.catalog, &self.attempt, &local)?;
        self.catalog.retain(&self.attempt.candidate)?;
        self.catalog
            .state
            .write_publication(&PublicationMarker::Attempting(self.attempt.clone()))?;
        self.catalog.git.run(
            &[
                "push",
                "origin",
                &format!(
                    "{}:refs/heads/{}",
                    self.attempt.candidate, self.attempt.branch
                ),
            ],
            None,
            None,
            &[0],
        )?;
        self.acknowledged = true;
        Ok(())
    }

    pub(crate) fn acknowledge(self) -> Result<AdoptionPermit<'machine, 'catalog>, RepairRequired> {
        if !self.acknowledged {
            return Err(invalid("push has no acknowledgement"));
        }
        self.machine.step(Action::ConfirmPushSuccess)?;
        Ok(AdoptionPermit {
            machine: self.machine,
            catalog: self.catalog,
            old: self.attempt.local_tip.clone(),
            new: self.attempt.candidate.clone(),
            attempt: self.attempt,
        })
    }

    pub(crate) fn recover(
        self,
        remote: &History,
    ) -> Result<Recovery<'machine, 'catalog>, CatalogError> {
        let local = read_history(&self.catalog.git, Some(&self.attempt.local_tip))?;
        let candidate = validate_attempt(self.catalog, &self.attempt, &local)?;
        if is_confirmed(self.catalog, &self.attempt, &candidate, remote)? {
            self.machine.step(Action::ConfirmLostPushAfterFetch)?;
            Ok(Recovery::Confirmed(ConfirmedAttemptWitness {
                catalog: self.catalog,
                attempt: self.attempt,
                remote_tip: remote.tip.clone(),
            }))
        } else {
            Ok(Recovery::Unconfirmed(self))
        }
    }

    pub(crate) fn retry(self, observed: &History) -> Result<(), RepairRequired> {
        if observed.tip == self.attempt.base {
            return Err(invalid("push retry requires a changed remote"));
        }
        self.machine.step(Action::RetryPushOnRemoteRace)
    }

    pub(crate) fn leave(self) -> Result<(), RepairRequired> {
        self.machine.step(Action::LeavePushFailedOrUncertain)
    }
}

impl AdoptionPermit<'_, '_> {
    // Bookkeeping failures precede adoption and use sync's outer error category.
    // Own the updated receipt so every later recovery fence keeps its timestamp.
    pub(crate) fn record_confirmation(mut self) -> Result<Self, CatalogError> {
        let mut metadata = self.catalog.state.metadata()?;
        metadata.confirmed = Some(self.new.clone());
        metadata.fetched_at = Some((self.catalog.clock)());
        self.catalog.state.write_metadata(&metadata)?;
        self.attempt = self.catalog.record_confirmation(&self.attempt)?;
        Ok(self)
    }

    pub(crate) fn apply(self) -> Result<String, CatalogError> {
        let result = (|| {
            // Retain once, before any adoption mutation, including preflight failure.
            self.catalog.retain(&self.old)?;
            self.catalog.retain(&self.new)?;
            let local = read_history(&self.catalog.git, Some(&self.old))?;
            inspect(self.catalog, &local)?;
            self.catalog.adopt(&self.old, &self.new, &self.attempt)
        })();
        match result {
            Ok(()) => {
                self.machine.step(Action::CompleteAdoption)?;
                Ok(self.new)
            }
            Err(error) => {
                self.machine.step(Action::FailAdoptionNeedsRepair)?;
                // A failed clear may have unlinked the marker before directory fsync
                // failed. Reassert the fence even after checkout/ref updates: absence
                // does not prove adoption completed durably. Do not duplicate retained
                // refs, or overwrite an already durable identical fence.
                let marker = PublicationMarker::Adopting(
                    self.attempt
                        .adopting(&self.old, &self.new)
                        .map_err(RepairRequired::new)?,
                );
                if self.catalog.state.publication()?.as_ref() != Some(&marker) {
                    self.catalog.state.write_publication(&marker)?;
                }
                Err(error)
            }
        }
    }
}
