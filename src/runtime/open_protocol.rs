//! Embedded native-open ordering; identity and durable evidence belong to AttemptGuard.

use std::sync::OnceLock;
use strum::{AsRefStr, EnumString};
use temper_jit::table::{Effect, EvalContext, TransitionTable, effect_args};

use super::{Result, invalid};

pub const NATIVE_OPEN_IOA: &str = include_str!("../../specs/native_open.ioa.toml");

pub fn protocol_table() -> &'static TransitionTable {
    static TABLE: OnceLock<TransitionTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        TransitionTable::try_from_ioa_source(NATIVE_OPEN_IOA)
            .expect("embedded native-open protocol must parse")
    })
}

#[derive(Debug, Clone, Copy, AsRefStr, EnumString)]
enum Phase {
    Prepared,
    Dispatching,
    Acknowledged,
    Bound,
    Complete,
    Uncertain,
}
#[derive(Debug, Clone, Copy, AsRefStr)]
pub(super) enum Action {
    Dispatch,
    Acknowledge,
    Bind,
    Complete,
    Fail,
}

#[derive(Clone)]
pub(super) struct Protocol {
    phase: Phase,
    context: EvalContext,
}
impl Protocol {
    /// Constructed only by a fresh attempt writer, after durable marker creation.
    pub(super) fn new() -> Result<Self> {
        let mut context = EvalContext::default();
        context.booleans.extend([
            ("marker".into(), true),
            ("pending".into(), false),
            ("ready".into(), false),
        ]);
        crate::sync::protocol::verify_total_context(protocol_table(), &context)
            .map_err(|e| invalid(e.to_string()))?;
        Ok(Self {
            phase: Phase::Prepared,
            context,
        })
    }
    pub(super) fn next(&self, action: Action) -> Result<Self> {
        let table = protocol_table();
        crate::sync::protocol::verify_total_context(table, &self.context)
            .map_err(|e| invalid(e.to_string()))?;
        let transition = table
            .evaluate_ctx(self.phase.as_ref(), &self.context, action.as_ref())
            .filter(|r| r.success)
            .ok_or_else(|| {
                invalid(format!(
                    "native-open protocol rejects {} in {}",
                    action.as_ref(),
                    self.phase.as_ref()
                ))
            })?;
        let mut next = self.clone();
        for effect in transition.effects {
            match effect {
                Effect::SetState(_) => {}
                Effect::SetBool { var, value } => {
                    let value = effect_args::boolean(
                        &value,
                        &next.context.booleans,
                        &serde_json::Value::Null,
                    )
                    .ok_or_else(|| invalid("invalid native-open boolean effect"))?;
                    *next
                        .context
                        .booleans
                        .get_mut(&var)
                        .ok_or_else(|| invalid("unknown native-open projection"))? = value;
                }
                _ => return Err(invalid("unsupported native-open effect")),
            }
        }
        next.phase = transition
            .new_state
            .parse()
            .map_err(|_| invalid("unknown native-open phase"))?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uncertainty_and_completion_never_mint_another_dispatch() {
        let initial = Protocol::new().unwrap();
        assert!(initial.next(Action::Complete).is_err());
        let dispatched = initial.next(Action::Dispatch).unwrap();
        assert!(dispatched.next(Action::Dispatch).is_err());
        assert!(dispatched.next(Action::Bind).is_err());
        let failed = dispatched.next(Action::Fail).unwrap();
        assert!(failed.next(Action::Dispatch).is_err());
        assert!(failed.next(Action::Bind).is_err());
        let bound = dispatched
            .next(Action::Acknowledge)
            .unwrap()
            .next(Action::Bind)
            .unwrap();
        assert!(bound.next(Action::Dispatch).is_err());
        assert!(
            bound
                .next(Action::Complete)
                .unwrap()
                .next(Action::Dispatch)
                .is_err()
        );
    }
}
