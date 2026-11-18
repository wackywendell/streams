//! Pure membership changes. Native observation and external effects belong to the caller.

use serde::Serialize;

use crate::model::{Session, Stream, ValidationError, check_text};

/// A destination for one verified native conversation, never a model-generated update.
/// The native adapter verifies the live destination before calling this domain boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MoveIntent {
    host_id: String,
    harness: String,
    native_id: String,
    destination_stream_id: String,
}

impl MoveIntent {
    pub fn new(
        host_id: impl Into<String>,
        harness: impl Into<String>,
        native_id: impl Into<String>,
        destination_stream_id: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let intent = Self {
            host_id: host_id.into(),
            harness: harness.into(),
            native_id: native_id.into(),
            destination_stream_id: destination_stream_id.into(),
        };
        for (name, text) in [
            ("move host", &intent.host_id),
            ("move harness", &intent.harness),
            ("move native ID", &intent.native_id),
            ("move destination", &intent.destination_stream_id),
        ] {
            check_text(text, name, false)?;
        }
        Ok(intent)
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }
    pub fn destination_stream_id(&self) -> &str {
        &self.destination_stream_id
    }
    pub fn session_id(&self) -> String {
        Session::id_for(&self.host_id, &self.harness, &self.native_id)
    }
}

/// Read Session + old Stream + destination, then replace only the returned Session.
/// `None` means membership already matches; no new membership period is created.
/// Destination creation and native relationship/evidence writes are separate operations.
pub fn apply_move(
    session: &Session,
    source: &Stream,
    destination: &Stream,
    intent: &MoveIntent,
) -> Result<Option<Session>, ValidationError> {
    session.validate_portable()?;
    source.validate()?;
    destination.validate()?;
    if session.host_id != intent.host_id
        || source.owner_host_id != intent.host_id
        || destination.owner_host_id != intent.host_id
    {
        return Err(ValidationError::new(
            "membership moves require one local writing host",
        ));
    }
    if session.id != intent.session_id()
        || session.harness != intent.harness
        || session.harness_session_id != intent.native_id
    {
        return Err(ValidationError::new(
            "move does not identify the current conversation",
        ));
    }
    if session.stream_id != source.id {
        return Err(ValidationError::new(
            "move source does not match current membership",
        ));
    }
    if destination.id != intent.destination_stream_id {
        return Err(ValidationError::new(
            "move destination does not match verified intent",
        ));
    }
    if session.stream_id == destination.id {
        return Ok(None);
    }
    let mut moved = session.clone();
    moved.membership_epoch += 1;
    moved.stream_id = destination.id.clone();
    moved.validate_portable()?;
    Ok(Some(moved))
}

#[cfg(test)]
mod tests;
