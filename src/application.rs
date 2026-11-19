//! Host-bound admission and record-local catalog actions.

pub mod artifacts;
mod create;
pub mod documents;
pub mod integration;
pub mod lifecycle;
mod listing;
pub use create::create_record;
pub use listing::{ListedStream, StreamFilter, StreamList, list_streams};
mod enrollment;
pub use enrollment::{
    EnrollmentReceipt, EnrollmentSave, enroll, enroll_session, enroll_session_observed,
    enroll_stream, remember_directory,
};
mod record_update;
mod records;
pub use record_update::{
    save_record_update, save_session_edit, save_stream_edit, save_user_references,
};
pub use records::{
    DiscardedCard, OpenedRecords, initialize_records, open_records, open_records_observed,
};

use crate::config::ConfigError;
use crate::model::ValidationError;
use crate::storage::StorageError;
use crate::sync::state::RepairRequired;

#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repair(#[from] RepairRequired),
    #[error(transparent)]
    Runtime(#[from] crate::runtime::RuntimeError),
}

pub type Result<T> = std::result::Result<T, ApplicationError>;

/// Only an unenrolled conversation takes its Stream from the caller's workspace. Ask for
/// that Stream only then, and when the workspace names one, `read` `request` again with it.
pub fn with_workspace_stream<E>(
    context: crate::agent::RecordContext,
    request: crate::agent::ContextRequest<'_>,
    workspace_stream: impl FnOnce() -> Option<String>,
    read: impl FnOnce(
        crate::agent::ContextRequest<'_>,
    ) -> std::result::Result<crate::agent::RecordContext, E>,
) -> std::result::Result<crate::agent::RecordContext, E> {
    if context.session.is_some() {
        return Ok(context);
    }
    match workspace_stream() {
        Some(stream_id) => read(crate::agent::ContextRequest {
            workspace_stream_id: Some(&stream_id),
            ..request
        }),
        None => Ok(context),
    }
}

/// Same-pass data, never serialized into the saved record.
pub struct MoveSave {
    pub save: crate::storage::records::RecordSave<crate::operations::DomainOutcome>,
    /// The destination Stream read during apply.
    pub stream: crate::model::Stream,
}

/// Reuse the Stream read during apply when constructing the resulting native context.
/// A retried move finds the Session already in its destination and changes nothing.
/// The planned Session move command (CLI redesign) uses it.
pub fn move_session_observed(
    store: &impl crate::storage::records::RecordStore,
    intent: &crate::operations::membership::MoveIntent,
) -> Result<MoveSave> {
    use crate::{
        model::{Record, RecordKind},
        operations::{DomainOutcome, membership::apply_move},
        storage::records::{RecordAddress, RecordChange},
    };
    let address = RecordAddress::new(intent.host_id(), RecordKind::Session, intent.session_id())?;
    let mut stream = None;
    let save = store.save::<_, ApplicationError>(&address, |reader, before| {
        let Some(before) = before else {
            return Err(ValidationError::new("native conversation is not enrolled").into());
        };
        let Record::Session(session) = before else {
            return Err(ValidationError::new("membership target must be a Session").into());
        };
        let source_key =
            RecordAddress::new(&session.host_id, RecordKind::Stream, &session.stream_id)?;
        let Some(Record::Stream(source)) = reader.read(&source_key)? else {
            return Err(ValidationError::new("current Stream is missing or invalid").into());
        };
        let destination = if source.id == intent.destination_stream_id() {
            source.clone()
        } else {
            let key = RecordAddress::new(
                intent.host_id(),
                RecordKind::Stream,
                intent.destination_stream_id(),
            )?;
            let Some(Record::Stream(destination)) = reader.read(&key)? else {
                return Err(ValidationError::new(
                    "destination Stream is missing; create it before moving",
                )
                .into());
            };
            destination
        };
        let moved = apply_move(session, &source, &destination, intent)?;
        let outcome = if moved.is_some() {
            DomainOutcome::Changes
        } else {
            DomainOutcome::NoChange
        };
        let after = moved.unwrap_or_else(|| session.clone());
        stream = Some(destination);
        Ok(RecordChange {
            record: Record::Session(after),
            outcome,
        })
    })?;
    Ok(MoveSave {
        save,
        stream: stream.expect("a successful move read its destination"),
    })
}
