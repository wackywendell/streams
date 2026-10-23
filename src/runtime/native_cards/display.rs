use super::*;
use crate::tui::NativeOwnerObservation;

/// Read-only placement, not a capability for native effects or catalog writes.
#[derive(Clone)]
pub struct CardPlacement {
    pub owner: NativeOwnerObservation,
    pub card: NativeOwnerObservation,
}

/// A card observes its conversation and exact launch, independently of pair
/// publication. Only the Pi child writes native evidence and markers.
pub struct DisplayLifetime {
    context: LaunchedCardContext,
    state: NativeState,
    socket: PathBuf,
    card_terminal: Option<String>,
}
impl DisplayLifetime {
    pub fn new(context: LaunchedCardContext, socket: PathBuf) -> Result<Self> {
        Ok(Self {
            socket,
            state: NativeState::new(context.state_directory())?,
            context,
            card_terminal: None,
        })
    }
    // The native format cutover changes this single reader together with its
    // producer and migration. Display never probes or mixes evidence formats.
    pub fn read_evidence(&self) -> Result<Option<NativeAttempt>> {
        let owner = NativeTuple {
            pane_id: self.context.owner().pane_id.clone(),
            tab_id: self.context.owner().tab_id.clone(),
            workspace_id: self.context.owner().workspace_id.clone(),
            terminal_id: self.context.owner_terminal().into(),
        };
        let key = attempt_key(&self.socket, self.context.scope(), &owner)?;
        Ok(self
            .state
            .read(&key)?
            .filter(|record| record.nonce() == self.context.nonce()))
    }
    pub fn context(&self) -> &LaunchedCardContext {
        &self.context
    }
    pub fn observe_native(
        &mut self,
        client: &HerdrClient,
        snapshot: &HerdrNativeSnapshot,
        evidence: &NativeAttempt,
    ) -> Result<Option<CardPlacement>> {
        let Some(owner) = snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| pane.terminal_id == self.context.owner_terminal())
        else {
            return Ok(None);
        };
        if client.endpoint().socket != self.socket {
            return Err(invalid("card endpoint changed"));
        }
        if evidence.nonce() != self.context.nonce() {
            return Err(invalid("card launch lifetime does not match its evidence"));
        }
        let card = match &self.card_terminal {
            Some(terminal) => snapshot.locate_terminal(terminal)?,
            None => {
                let inherited = client.current_pane_from(snapshot.raw())?;
                snapshot
                    .topology()
                    .panes
                    .iter()
                    .find(|pane| pane.pane == inherited)
                    .ok_or_else(|| invalid("card caller is missing from the native snapshot"))?
            }
        };
        if evidence
            .resources()
            .is_some_and(|resources| resources.terminal_id() != card.terminal_id)
        {
            return Err(invalid("card terminal does not match its launch evidence"));
        }
        self.card_terminal = Some(card.terminal_id.clone());
        let observed = |pane: &HerdrInventoryPane| NativeOwnerObservation {
            pane_id: pane.pane.id.clone(),
            tab_id: pane.pane.tab_id.clone(),
            workspace_id: pane.pane.workspace_id.clone(),
        };
        Ok(Some(CardPlacement {
            owner: observed(owner),
            card: observed(card),
        }))
    }
    pub fn admit(&self, frame: &CardSnapshot, placement: &CardPlacement) -> Result<()> {
        if !self.context.scope().same_conversation(frame.scope())
            || frame.owner() != &placement.owner
        {
            return Err(invalid(
                "card observation belongs to another conversation or owner",
            ));
        }
        Ok(())
    }
    pub fn caller_terminal(&self) -> Option<&str> {
        self.card_terminal.as_deref()
    }
}
