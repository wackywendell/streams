//! Publication permits retain the exact catalog and identities inspected before effects.

use super::{Action, Phase, ProtocolMachine};
use crate::sync::{
    Oid,
    coordinator::{CatalogError, Coordinator},
    history::History,
    replay::{Replay, replay},
    state::{GitConfig, PublicationAttempt, PublicationMarker, RepairRequired},
};

fn invalid(message: &str) -> RepairRequired {
    RepairRequired::new(message)
}

fn inspect(catalog: &Coordinator<'_>, tip: Option<&str>) -> Result<GitConfig, CatalogError> {
    let config = catalog.ready()?;
    if Some(catalog.io.head()?).as_deref() != tip {
        return Err(invalid("local tip changed since publication inspection").into());
    }
    Ok(config)
}

fn validate_candidate(
    catalog: &Coordinator<'_>,
    candidate: &str,
    local: &History,
    remote: &History,
) -> Result<History, CatalogError> {
    let history = catalog.io.history(Some(candidate))?;
    let Replay::Ready {
        catalog: projected,
        changes,
    } = replay(local, remote)?
    else {
        return Err(invalid("candidate does not match the inspected publication batch").into());
    };
    let base = remote
        .tip()
        .or(local.root())
        .ok_or_else(|| invalid("candidate has no supported base"))?;
    let actual = history
        .operations()
        .iter()
        .filter(|entry| !remote.has_operation(entry.operation.id()))
        .map(|entry| (&entry.operation, &entry.origin));
    let expected = changes
        .iter()
        .map(|change| (&change.operation, &change.origin));
    if !actual.eq(expected)
        || history.catalog() != &projected
        || !catalog.io.ancestor(base, candidate)?
    {
        return Err(invalid("candidate does not match the inspected publication batch").into());
    }
    Ok(history)
}

// Validate a retained attempt against its original base and local tip, not the
// newer fetched history: additional local saves must not invalidate confirmation.
fn validate_attempt(
    catalog: &Coordinator<'_>,
    attempt: &PublicationAttempt,
    local_tip: Option<&str>,
) -> Result<History, CatalogError> {
    let config = inspect(catalog, local_tip)?;
    if config.branch() != attempt.branch() || config.remote() != Some(attempt.remote()) {
        return Err(invalid("publication target changed since inspection").into());
    }
    let original = catalog.io.history(Some(attempt.local_tip().as_str()))?;
    if !catalog
        .io
        .ancestor(attempt.local_tip().as_str(), local_tip.unwrap())?
    {
        return Err(invalid("retained publication has unrelated local history").into());
    }
    let base = catalog.io.history(attempt.base().map(Oid::as_str))?;
    let candidate = validate_candidate(catalog, attempt.candidate().as_str(), &original, &base)?;
    let operations: Vec<_> = candidate
        .operations()
        .iter()
        .filter(|entry| !base.has_operation(entry.operation.id()))
        .map(|entry| entry.operation.id().to_owned())
        .collect();
    if operations != attempt.operations() {
        return Err(invalid("retained publication operations were changed").into());
    }
    Ok(candidate)
}

fn is_confirmed(
    catalog: &Coordinator<'_>,
    attempt: &PublicationAttempt,
    candidate: &History,
    remote: &History,
) -> Result<bool, CatalogError> {
    if let Some(tip) = remote.tip()
        && catalog.io.ancestor(attempt.candidate().as_str(), tip)?
    {
        return Ok(true);
    }
    Ok(!attempt.operations().is_empty()
        && attempt.operations().iter().all(|id| {
            match (remote.operation(id), candidate.operation(id)) {
                (Some(a), Some(b)) => a == b,
                _ => false,
            }
        }))
}

pub(crate) struct InspectedAttempt<'catalog> {
    catalog: &'catalog Coordinator<'catalog>,
    attempt: PublicationAttempt,
    candidate: History,
    remote_tip: Option<String>,
}

pub(crate) struct ConfirmedAttemptWitness<'catalog>(InspectedAttempt<'catalog>);

/// Confirmation whose scheduling baseline and exact attempt are durably recorded.
pub(crate) struct RecordedConfirmation<'catalog>(InspectedAttempt<'catalog>);

impl<'catalog> ConfirmedAttemptWitness<'catalog> {
    pub(crate) fn record(mut self) -> Result<RecordedConfirmation<'catalog>, CatalogError> {
        self.0.check_marker(Some(&self.0.attempt))?;
        self.0.check_observation()?;
        self.0.attempt = self.0.catalog.record_confirmation(&self.0.attempt)?;
        Ok(RecordedConfirmation(self.0))
    }
}

pub(crate) enum RetainedAttempt<'catalog> {
    Confirmed(RecordedConfirmation<'catalog>),
    Unconfirmed(InspectedAttempt<'catalog>),
}

impl<'catalog> RetainedAttempt<'catalog> {
    fn inspected(&self) -> &InspectedAttempt<'catalog> {
        match self {
            Self::Confirmed(RecordedConfirmation(inspected)) => inspected,
            Self::Unconfirmed(inspected) => inspected,
        }
    }

    pub(crate) fn reusable_candidate(&self, local: &History, remote: &History) -> Option<&str> {
        match self {
            Self::Unconfirmed(inspected)
                if inspected.remote_tip.as_deref() == remote.tip()
                    && inspected.attempt.base().map(Oid::as_str) == remote.tip()
                    && Some(inspected.attempt.local_tip().as_str()) == local.tip() =>
            {
                Some(inspected.attempt.candidate().as_str())
            }
            _ => None,
        }
    }
}

impl InspectedAttempt<'_> {
    fn check_marker(&self, expected: Option<&PublicationAttempt>) -> Result<(), CatalogError> {
        if self.catalog.state.publication()? != expected.cloned().map(PublicationMarker::Attempting)
        {
            return Err(invalid("publication marker changed since inspection").into());
        }
        Ok(())
    }

    fn check_observation(&self) -> Result<(), CatalogError> {
        if self
            .catalog
            .state
            .metadata()?
            .confirmed
            .as_ref()
            .map(Oid::as_str)
            != self.remote_tip.as_deref()
        {
            return Err(invalid("remote observation changed since inspection").into());
        }
        Ok(())
    }

    fn check_live(&self, expected: Option<&PublicationAttempt>) -> Result<(), CatalogError> {
        let config = inspect(self.catalog, Some(self.attempt.local_tip().as_str()))?;
        if config.branch() != self.attempt.branch()
            || config.remote() != Some(self.attempt.remote())
        {
            return Err(invalid("publication target changed since inspection").into());
        }
        self.check_marker(expected)?;
        self.check_observation()
    }
}

pub(crate) struct PushPermit<'machine, 'catalog> {
    machine: &'machine mut ProtocolMachine,
    inspected: InspectedAttempt<'catalog>,
    stored: Option<PublicationAttempt>,
    acknowledged: bool,
}

pub(crate) enum Recovery<'machine, 'catalog> {
    Confirmed(ConfirmedAttemptWitness<'catalog>),
    Unconfirmed(PushPermit<'machine, 'catalog>),
}

pub(crate) struct AdoptionPermit<'machine, 'catalog> {
    machine: &'machine mut ProtocolMachine,
    catalog: &'catalog Coordinator<'catalog>,
    old: Oid,
    new: Oid,
    attempt: PublicationAttempt,
}

impl ProtocolMachine {
    pub(in crate::sync) fn inspect_retained<'catalog>(
        &mut self,
        catalog: &'catalog Coordinator<'catalog>,
        local_tip: Option<&str>,
        remote: &History,
    ) -> Result<Option<RetainedAttempt<'catalog>>, CatalogError> {
        let Some(marker) = catalog.state.publication()? else {
            return Ok(None);
        };
        let attempt = marker.attempt().clone();
        let candidate = validate_attempt(catalog, &attempt, local_tip)?;
        let confirmed = is_confirmed(catalog, &attempt, &candidate, remote)?;
        let inspected = InspectedAttempt {
            catalog,
            attempt,
            candidate,
            remote_tip: remote.tip().map(str::to_owned),
        };
        if confirmed {
            self.step(Action::RecoverConfirmedBeforePush)?;
            Ok(Some(RetainedAttempt::Confirmed(
                ConfirmedAttemptWitness(inspected).record()?,
            )))
        } else {
            Ok(Some(RetainedAttempt::Unconfirmed(inspected)))
        }
    }

    pub(in crate::sync) fn prepare_push<'machine, 'catalog>(
        &'machine mut self,
        catalog: &'catalog Coordinator<'catalog>,
        local: &History,
        remote: &History,
        candidate: &str,
        retained: Option<RetainedAttempt<'catalog>>,
    ) -> Result<Option<PushPermit<'machine, 'catalog>>, CatalogError> {
        if matches!(self.status, Phase::Ready) && !self.has_push_budget() {
            return Ok(None);
        }
        let config = inspect(catalog, local.tip())?;
        let prior = retained.as_ref().map(RetainedAttempt::inspected);
        if let Some(prior) = prior
            && (!std::ptr::eq(prior.catalog, catalog)
                || prior.remote_tip.as_deref() != remote.tip())
        {
            return Err(invalid("retained publication belongs to another observation").into());
        }
        if catalog.state.publication()?
            != prior.map(|prior| PublicationMarker::Attempting(prior.attempt.clone()))
            || catalog
                .state
                .metadata()?
                .confirmed
                .as_ref()
                .map(Oid::as_str)
                != remote.tip()
        {
            return Err(invalid("publication facts changed since inspection").into());
        }
        let stored = prior.map(|prior| prior.attempt.clone());
        let reusable = retained
            .filter(|retained| retained.reusable_candidate(local, remote) == Some(candidate));
        let (history, action) = match reusable {
            Some(RetainedAttempt::Unconfirmed(inspected)) => {
                (inspected.candidate, Action::ReuseExistingCandidate)
            }
            _ => (
                validate_candidate(catalog, candidate, local, remote)?,
                Action::PrepareFreshCandidate,
            ),
        };
        let local_tip = local
            .tip()
            .ok_or_else(|| invalid("local catalog has no tip"))?;
        let target = config
            .remote()
            .ok_or_else(|| invalid("publication requires a remote"))?;
        let attempt = PublicationAttempt::new(
            remote
                .tip()
                .map(str::parse)
                .transpose()
                .map_err(RepairRequired::new)?,
            candidate.parse().map_err(RepairRequired::new)?,
            local_tip.parse().map_err(RepairRequired::new)?,
            config.branch(),
            target,
            history
                .operations()
                .iter()
                .filter(|entry| !remote.has_operation(entry.operation.id()))
                .map(|entry| entry.operation.id().to_owned())
                .collect(),
            None,
        )
        .map_err(RepairRequired::new)?;
        if matches!(action, Action::ReuseExistingCandidate) {
            if stored.as_ref() != Some(&attempt) {
                return Err(invalid("candidate reuse identity changed").into());
            }
            self.context
                .booleans
                .insert("candidate_reusable".into(), true);
        }
        self.step(action)?;
        Ok(Some(PushPermit {
            machine: self,
            inspected: InspectedAttempt {
                catalog,
                attempt,
                candidate: history,
                remote_tip: remote.tip().map(str::to_owned),
            },
            stored,
            acknowledged: false,
        }))
    }

    pub(in crate::sync) fn begin_remote_adoption<'machine, 'catalog>(
        &'machine mut self,
        catalog: &'catalog Coordinator<'catalog>,
        local: &History,
        remote: &History,
        confirmed: Option<RecordedConfirmation<'catalog>>,
    ) -> Result<AdoptionPermit<'machine, 'catalog>, CatalogError> {
        let config = inspect(catalog, local.tip())?;
        let new = remote
            .tip()
            .ok_or_else(|| invalid("cannot adopt an unpublished root"))?;
        validate_candidate(catalog, new, local, remote)?;
        if catalog
            .state
            .metadata()?
            .confirmed
            .as_ref()
            .map(Oid::as_str)
            != remote.tip()
        {
            return Err(invalid("remote observation changed before adoption").into());
        }
        let old = local
            .tip()
            .ok_or_else(|| invalid("local catalog has no tip"))?;
        let (attempt, old, new) = if let Some(RecordedConfirmation(witness)) = confirmed {
            if !std::ptr::eq(witness.catalog, catalog)
                || witness.remote_tip.as_deref() != remote.tip()
            {
                return Err(invalid("recorded confirmation belongs to another observation").into());
            }
            witness.check_marker(Some(&witness.attempt))?;
            witness.check_observation()?;
            if config.branch() != witness.attempt.branch()
                || config.remote() != Some(witness.attempt.remote())
            {
                return Err(invalid("publication target changed since inspection").into());
            }
            // The receipt describes the published batch, even when adopting a
            // newer descendant. The actual checkout transition is recorded separately.
            (
                witness.attempt,
                old.parse().map_err(RepairRequired::new)?,
                new.parse().map_err(RepairRequired::new)?,
            )
        } else {
            let target = config
                .remote()
                .ok_or_else(|| invalid("adoption requires a remote"))?;
            let old: Oid = old.parse().map_err(RepairRequired::new)?;
            let new: Oid = new.parse().map_err(RepairRequired::new)?;
            let attempt = PublicationAttempt::new(
                Some(new.clone()),
                new.clone(),
                old.clone(),
                config.branch(),
                target,
                vec![],
                None,
            )
            .map_err(RepairRequired::new)?;
            (attempt, old, new)
        };
        self.step(Action::NoPushNeededBeginAdopt)?;
        Ok(AdoptionPermit {
            machine: self,
            catalog,
            old,
            new,
            attempt,
        })
    }
}

impl<'machine, 'catalog> PushPermit<'machine, 'catalog> {
    pub(crate) fn push(&mut self) -> Result<(), CatalogError> {
        let catalog = self.inspected.catalog;
        let attempt = &self.inspected.attempt;
        catalog.io.retain(attempt.candidate().as_str())?;
        self.inspected.check_live(self.stored.as_ref())?;
        catalog
            .state
            .write_publication(&PublicationMarker::Attempting(attempt.clone()))?;
        self.stored = Some(attempt.clone());
        catalog.io.push(attempt.candidate(), attempt.branch())?;
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
            catalog: self.inspected.catalog,
            old: self.inspected.attempt.local_tip().clone(),
            new: self.inspected.attempt.candidate().clone(),
            attempt: self.inspected.attempt,
        })
    }

    pub(crate) fn recover(
        mut self,
        remote: &History,
    ) -> Result<Recovery<'machine, 'catalog>, CatalogError> {
        self.inspected.remote_tip = remote.tip().map(str::to_owned);
        self.inspected.check_live(self.stored.as_ref())?;
        if is_confirmed(
            self.inspected.catalog,
            &self.inspected.attempt,
            &self.inspected.candidate,
            remote,
        )? {
            // A retain failure may precede the new marker. Fence its replacement
            // against the actual prior marker before recording this confirmation.
            if self.stored.as_ref() != Some(&self.inspected.attempt) {
                self.inspected
                    .catalog
                    .state
                    .write_publication(&PublicationMarker::Attempting(
                        self.inspected.attempt.clone(),
                    ))?;
            }
            self.machine.step(Action::ConfirmLostPushAfterFetch)?;
            Ok(Recovery::Confirmed(ConfirmedAttemptWitness(self.inspected)))
        } else {
            Ok(Recovery::Unconfirmed(self))
        }
    }

    pub(crate) fn has_push_budget(&self) -> bool {
        self.machine.has_push_budget()
    }

    pub(crate) fn retry(
        self,
        observed: &History,
    ) -> Result<Option<RetainedAttempt<'catalog>>, CatalogError> {
        if observed.tip() == self.inspected.attempt.base().map(Oid::as_str) {
            return Err(invalid("push retry requires a changed remote").into());
        }
        self.machine.step(Action::RetryPushOnRemoteRace)?;
        if self.stored.as_ref() == Some(&self.inspected.attempt) {
            Ok(Some(RetainedAttempt::Unconfirmed(self.inspected)))
        } else if self.stored.is_some() {
            // A failure before writing the new marker left an older batch in the
            // journal. Reinspect that exceptional path instead of retaining two histories.
            self.machine.inspect_retained(
                self.inspected.catalog,
                Some(self.inspected.attempt.local_tip().as_str()),
                observed,
            )
        } else {
            Ok(None)
        }
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
        metadata.fetched_at = Some(self.catalog.now()?);
        self.catalog.state.write_metadata(&metadata)?;
        self.attempt = self.catalog.record_confirmation(&self.attempt)?;
        Ok(self)
    }

    pub(crate) fn apply(self) -> Result<String, CatalogError> {
        let result = (|| {
            // Retain once, before any adoption mutation, including preflight failure.
            self.catalog.io.retain(self.old.as_str())?;
            self.catalog.io.retain(self.new.as_str())?;
            inspect(self.catalog, Some(self.old.as_str()))?;
            self.catalog.adopt(&self.old, &self.new, &self.attempt)
        })();
        match result {
            Ok(()) => {
                self.machine.step(Action::CompleteAdoption)?;
                Ok(self.new.into())
            }
            Err(error) => {
                self.machine.step(Action::FailAdoptionNeedsRepair)?;
                // A failed clear may have unlinked the marker before directory fsync
                // failed. Reassert the fence even after checkout/ref updates: absence
                // does not prove adoption completed durably. Do not duplicate retained
                // refs, or overwrite an already durable identical fence.
                self.catalog
                    .state
                    .fence_adoption(&self.attempt.adopting(self.old, self.new))?;
                Err(error)
            }
        }
    }
}
