//! Save fencing through the same IOA table used by Temper's verifier.
//!
//! Identity-bound save and publication permits execute the verified phase transitions.

mod import;
pub(crate) mod publication;

use std::sync::OnceLock;
use strum::{AsRefStr, EnumString};

#[derive(Debug, Clone, Copy, EnumString, AsRefStr)]
enum Phase {
    Ready,
    Saving,
    AttemptingPush,
    Adopting,
    RepairRequired,
}

#[derive(Debug, Clone, Copy, AsRefStr)]
enum Action {
    BeginSave,
    CompleteSave,
    FailSaveNeedsRepair,
    PrepareFreshCandidate,
    ReuseExistingCandidate,
    RecoverConfirmedBeforePush,
    NoPushNeededBeginAdopt,
    ConfirmPushSuccess,
    ConfirmLostPushAfterFetch,
    RetryPushOnRemoteRace,
    LeavePushFailedOrUncertain,
    CompleteAdoption,
    FailAdoptionNeedsRepair,
}

use temper_jit::table::{Effect, EvalContext, TransitionTable, effect_args};

use super::state::{PublicationMarker, RepairRequired, State};
use crate::{
    model::Record,
    operations::Operation,
    storage::{Journal, Snapshot},
};

pub const CATALOG_PROTOCOL_IOA: &str = include_str!("../../specs/catalog_protocol.ioa.toml");

/// Parse the embedded specification once; a malformed shipped spec is a programming error.
pub fn protocol_table() -> &'static TransitionTable {
    static TABLE: OnceLock<TransitionTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        TransitionTable::try_from_ioa_source(CATALOG_PROTOCOL_IOA)
            .expect("embedded catalog protocol must parse")
    })
}

/// Reject missing projections instead of using Temper's false/zero defaults.
pub fn verify_total_context(
    table: &TransitionTable,
    ctx: &EvalContext,
) -> Result<(), RepairRequired> {
    for name in table.initial_values.booleans.keys() {
        if !ctx.booleans.contains_key(name) {
            return Err(RepairRequired::new(format!(
                "missing boolean projection: {name}"
            )));
        }
    }
    for name in table.initial_values.counters.keys() {
        if !ctx.counters.contains_key(name) {
            return Err(RepairRequired::new(format!(
                "missing counter projection: {name}"
            )));
        }
    }
    for name in table.initial_values.lists.keys() {
        if !ctx.lists.contains_key(name) {
            return Err(RepairRequired::new(format!(
                "missing list projection: {name}"
            )));
        }
    }
    for name in table.guard_field_names(None) {
        if !ctx.fields.contains_key(&name) {
            return Err(RepairRequired::new(format!(
                "missing field projection: {name}"
            )));
        }
    }
    Ok(())
}

/// The permit borrows its originating machine and exact immutable save inputs.
/// It cannot be cloned, transferred to another machine, or completed twice.
pub(crate) struct SavePermit<'machine, 'input> {
    machine: &'machine mut ProtocolMachine,
    operation: &'input Operation,
    record: &'input Record,
    expected: &'input Snapshot,
    start_tip: Option<&'input str>,
}

impl SavePermit<'_, '_> {
    pub(crate) fn operation(&self) -> &Operation {
        self.operation
    }
    pub(crate) fn record(&self) -> &Record {
        self.record
    }
    pub(crate) fn expected(&self) -> &Snapshot {
        self.expected
    }
    pub(crate) fn start_tip(&self) -> Option<&str> {
        self.start_tip
    }
    pub(crate) fn complete(self) -> Result<(), RepairRequired> {
        self.machine.step(Action::CompleteSave)
    }
    pub(crate) fn fail(self) -> Result<(), RepairRequired> {
        self.machine.step(Action::FailSaveNeedsRepair)
    }
}

/// Private phase and total projection; transitions are available only through borrowed permits.
pub(crate) struct ProtocolMachine {
    status: Phase,
    context: EvalContext,
}

impl ProtocolMachine {
    pub(crate) fn observe<J: Journal>(
        state: &State<J>,
        attempt_budget: usize,
    ) -> Result<Self, RepairRequired> {
        let save = state.read("save.json")?.is_some();
        let publication = state.publication()?;
        let adopting = matches!(publication, Some(PublicationMarker::Adopting(_)));
        let mut context = EvalContext::default();
        context.booleans.extend([
            ("save_marker".into(), save),
            ("publication_marker".into(), publication.is_some()),
            ("adopting_marker".into(), adopting),
            ("confirmed_evidence".into(), false),
            ("candidate_reusable".into(), false),
        ]);
        context
            .counters
            .insert("attempt_budget".into(), attempt_budget);
        verify_total_context(protocol_table(), &context)?;
        Ok(Self {
            status: if save || adopting {
                Phase::RepairRequired
            } else {
                Phase::Ready
            },
            context,
        })
    }

    // Raw dispatch is private: callers cannot bypass the evidence-bearing API.
    fn step(&mut self, action: Action) -> Result<(), RepairRequired> {
        let table = protocol_table();
        let action = action.as_ref();
        verify_total_context(table, &self.context)?;
        let result = table
            .evaluate_ctx(self.status.as_ref(), &self.context, action)
            .filter(|r| r.success)
            .ok_or_else(|| {
                RepairRequired::new(format!(
                    "protocol rejected {action} in {}",
                    self.status.as_ref()
                ))
            })?;
        let mut next = self.context.clone();
        for effect in result.effects {
            let invalid = || RepairRequired::new("unsupported or incomplete protocol effect");
            match &effect {
                Effect::SetState(_) => {}
                Effect::SetBool { var, value } => {
                    let value =
                        effect_args::boolean(value, &next.booleans, &serde_json::Value::Null)
                            .ok_or_else(invalid)?;
                    *next.booleans.get_mut(var).ok_or_else(invalid)? = value;
                }
                Effect::SetCounter { var, value }
                | Effect::AddCounter { var, value }
                | Effect::SubCounter { var, value } => {
                    let value = effect_args::count(value, &next.counters, &serde_json::Value::Null)
                        .ok_or_else(invalid)?;
                    let slot = next.counters.get_mut(var).ok_or_else(invalid)?;
                    *slot = match effect {
                        Effect::SetCounter { .. } => value,
                        Effect::AddCounter { .. } => slot.checked_add(value).ok_or_else(invalid)?,
                        _ => slot.saturating_sub(value),
                    };
                }
                _ => return Err(invalid()),
            }
        }
        let status = result
            .new_state
            .parse()
            .map_err(|_| RepairRequired::new("protocol returned an unknown phase"))?;
        self.context = next;
        self.status = status;
        Ok(())
    }

    pub(crate) fn begin_save<'machine, 'input>(
        &'machine mut self,
        operation: &'input Operation,
        record: &'input Record,
        expected: &'input Snapshot,
        start_tip: Option<&'input str>,
    ) -> Result<SavePermit<'machine, 'input>, RepairRequired> {
        if record.key() != operation.target() {
            return Err(RepairRequired::new(
                "save result does not match operation target",
            ));
        }
        self.step(Action::BeginSave)?;
        Ok(SavePermit {
            machine: self,
            operation,
            record,
            expected,
            start_tip,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::Stream, operations::Actor};

    #[test]
    fn save_permit_controls_completion_and_dropped_permit_stays_fenced() {
        let dir = tempfile::tempdir().unwrap();
        let state = State::new(dir.path());
        let record = Record::Stream(Stream::new("work", "Work", None, "host").unwrap());
        let op = Operation::new("create", None, record.clone(), Actor::User, None, vec![]).unwrap();
        let snapshot = Snapshot::default();
        let mut machine = ProtocolMachine::observe(&state, 1).unwrap();
        let permit = machine.begin_save(&op, &record, &snapshot, None).unwrap();
        assert_eq!(permit.operation(), &op);
        permit.complete().unwrap();
        {
            let _permit = machine.begin_save(&op, &record, &snapshot, None).unwrap();
        }
        assert!(machine.begin_save(&op, &record, &snapshot, None).is_err());
    }
}
