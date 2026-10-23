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
    pub binding: Option<String>,
}
/// A captured owner lifetime is a restriction on later effects, never authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeOwnerContext {
    pub socket: PathBuf,
    pub pane_id: String,
    pub terminal_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    #[serde(deserialize_with = "deserialize_binding")]
    pub binding: Option<String>,
}
fn deserialize_binding<'de, D: serde::Deserializer<'de>>(
    value: D,
) -> std::result::Result<Option<String>, D::Error> {
    serde::Deserialize::deserialize(value)
}
pub struct NativeOwnerView {
    pub card: CardSnapshot,
    pub context: NativeOwnerContext,
}
impl NativeCardEngine {
    /// Observe the attested Pi owner without retiring markers or preparing state.
    pub fn observe_owner(&mut self) -> Result<NativeOwnerView> {
        self.begin_observation();
        self.observe_owner_current()
    }
    fn observe_owner_current(&mut self) -> Result<NativeOwnerView> {
        let v = self.inspect(false)?;
        if v.caller != v.owner || self.launched.is_some() {
            return Err(self.refuse("only the attested Pi owner may drive lifecycle effects"));
        }
        Ok(NativeOwnerView {
            context: NativeOwnerContext {
                socket: self.client.endpoint().socket.clone(),
                pane_id: v.owner.pane_id,
                terminal_id: v.owner.terminal_id,
                workspace_id: v.owner.workspace_id,
                tab_id: v.owner.tab_id,
                binding: v
                    .snapshot
                    .workspace_tokens(&v.caller.workspace_id)
                    .and_then(|tokens| tokens.get("strm_binding"))
                    .cloned(),
            },
            card: v.saved.card,
        })
    }
    /// A private child retains its already-attested caller terminal across requests.
    pub fn require_caller_terminal(&self, terminal: &str) -> Result<()> {
        if self.caller.terminal_id != terminal {
            return Err(super::super::RuntimeError::NativeContextChanged(
                "Owner pane terminal identity changed".into(),
            ));
        }
        Ok(())
    }

    /// Re-attest the caller and pin the CLI process before any lifecycle mutation.
    pub fn pin_owner(&mut self, scope: &Scope, expected: &NativeOwnerContext) -> Result<()> {
        // Pin construction against its bootstrap snapshot without spending the
        // first command's observation. Later pinning must still refresh.
        if !self.bootstrap {
            self.observation = None;
        }
        let observed = self.observe_owner_current()?;
        let mut context = observed.context;
        if expected.binding.is_none() {
            context.binding = None;
        }
        if observed.card.scope() != scope || context != *expected {
            return Err(super::super::RuntimeError::NativeContextChanged(
                "captured native owner scope, endpoint, tuple or binding changed".into(),
            ));
        }
        Ok(())
    }
    /// Read-only: does not acquire/create the workspace lock, repair evidence,
    /// retire predecessors, register plugins or initialize a native binding.
    pub fn status(&mut self) -> Result<NativeCardStatus> {
        self.begin_observation();
        let v = self.guard()?;
        let evidence = self.state.read(&v.key)?;
        if !matches!(v.pair()?, PairState::Empty)
            || evidence
                .as_ref()
                .is_some_and(|record| matches!(record.phase(), NativePhase::Closed(_)))
        {
            v.bound()?;
        }
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
        let binding = v.workspace_tokens().get("strm_binding").cloned();
        Ok(NativeCardStatus {
            condition,
            binding,
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
