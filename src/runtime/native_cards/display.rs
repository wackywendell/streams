use super::*;
use crate::tui::NativeOwnerObservation;

#[derive(Clone)]
pub struct CardPlacement {
    pub owner: NativeOwnerObservation,
    pub card: NativeOwnerObservation,
}

/// No native history: a usable snapshot either still contains this Pi or it does not.
pub struct DisplayLifetime {
    context: LaunchedCardContext,
    card_terminal: Option<String>,
}
impl DisplayLifetime {
    pub fn new(context: LaunchedCardContext) -> Self {
        Self {
            context,
            card_terminal: None,
        }
    }
    pub fn context(&self) -> &LaunchedCardContext {
        &self.context
    }
    pub fn observe_native(
        &mut self,
        client: &HerdrClient,
        snapshot: &HerdrNativeSnapshot,
    ) -> Result<Option<CardPlacement>> {
        let Some((owner, _)) = self.context.owner.observe(snapshot)? else {
            return Ok(None);
        };
        let card = match &self.card_terminal {
            Some(terminal) => match snapshot
                .topology()
                .panes
                .iter()
                .find(|pane| &pane.terminal_id == terminal)
            {
                Some(card) => card,
                None => return Ok(None),
            },
            None => {
                let inherited = client.current_pane_from(snapshot.raw())?;
                snapshot
                    .topology()
                    .panes
                    .iter()
                    .find(|pane| pane.pane.id == inherited.id)
                    .ok_or_else(|| invalid("card pane is absent from the snapshot"))?
            }
        };
        self.card_terminal = Some(card.terminal_id.clone());
        Ok(Some(CardPlacement {
            owner: owner::location(owner),
            card: owner::location(card),
        }))
    }
    pub fn admit(&self, frame: &CardSnapshot, placement: &CardPlacement) -> Result<()> {
        if !self.context.scope.same_conversation(frame.scope()) || frame.owner() != &placement.owner
        {
            return Err(invalid("card observation belongs to another conversation"));
        }
        Ok(())
    }
    pub fn caller_terminal(&self) -> Option<&str> {
        self.card_terminal.as_deref()
    }
}
