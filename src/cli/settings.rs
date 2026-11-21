use streams::{
    config::CardMode,
    runtime::card_settings::{CardSettings, ConversationId},
};

use super::*;

impl App {
    pub(super) fn conversation_card_settings(
        &self,
        records: &application::OpenedRecords,
        conversation: &ConversationId,
    ) -> Result<CardSettings> {
        Ok(CardSettings::new(
            &self.config,
            records.root(),
            records.host().as_str(),
            conversation,
        )?)
    }

    pub(super) fn card_settings(&self, conversation: &ConversationId) -> Result<Output> {
        let records = application::open_records(&self.config)?;
        let session = self
            .conversation_card_settings(&records, conversation)?
            .mode()?;
        let global = records.settings().herdr.cards_auto_open;
        let effective = session.resolve(global);
        Ok(Output::plain(
            json!({ "global": global, "session": session, "effective": effective }),
            format!("session={session} global={global} effective={effective}"),
        ))
    }

    pub(super) fn set_card_mode(
        &self,
        conversation: &ConversationId,
        mode: CardMode,
    ) -> Result<Output> {
        let records = application::open_records(&self.config)?;
        self.conversation_card_settings(&records, conversation)?
            .set(mode)?;
        let mut output = self.card_settings(conversation)?;
        output.accepted_save = true;
        output.wrote = true;
        Ok(output)
    }
}
