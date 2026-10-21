use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayCondition {
    Unbound,
    Absent,
    Ready,
    Closed,
    Pending,
    Separated,
}
#[derive(Debug, Clone, Serialize)]
pub struct NativeCardStatus {
    pub condition: DisplayCondition,
    pub workspace_id: String,
    pub owner_pane_id: String,
    pub owner_terminal_id: String,
    pub session_id: String,
    pub native_session_id: String,
    pub key: String,
    pub evidence: Option<NativeAttempt>,
}
impl NativeCardEngine {
    /// Read-only: does not acquire/create the workspace lock, repair evidence,
    /// retire predecessors, register plugins or initialize a native binding.
    pub fn status(&mut self) -> Result<NativeCardStatus> {
        let v = self.guard()?;
        let evidence = self.state.read(&v.key)?;
        let pending = evidence.as_ref().is_some_and(|record| {
            matches!(
                record.phase(),
                NativePhase::Unlaunched | NativePhase::Reserved(_)
            )
        });
        let condition = if pending {
            DisplayCondition::Pending
        } else {
            self.evidence(&v)?;
            match v.pair()? {
                PairState::Separated { .. } => DisplayCondition::Separated,
                PairState::Paired { .. } => DisplayCondition::Ready,
                _ if evidence
                    .as_ref()
                    .is_some_and(|record| matches!(record.phase(), NativePhase::Closed(_))) =>
                {
                    DisplayCondition::Closed
                }
                _ if v.bound().is_ok() => DisplayCondition::Absent,
                _ => DisplayCondition::Unbound,
            }
        };
        Ok(NativeCardStatus {
            condition,
            workspace_id: v.owner.workspace_id,
            owner_pane_id: v.owner.pane_id,
            owner_terminal_id: v.owner.terminal_id,
            session_id: v.saved.card.scope().session_id.clone(),
            native_session_id: v.saved.card.scope().native_session_id.clone(),
            key: v.key,
            evidence,
        })
    }
}
