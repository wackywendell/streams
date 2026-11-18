//! Host-bound admission and record-local catalog actions.

pub mod artifacts;
mod create;
pub mod documents;
pub mod integration;
pub mod lifecycle;
mod listing;
pub use create::create_record;
pub use listing::{StreamFilter, StreamList, list_streams};
mod enrollment;
pub use enrollment::{EnrollmentSave, enroll_session, enroll_session_observed, enroll_stream};
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

/// Same-pass data, never serialized into the saved record.
pub struct MoveSave {
    pub save: crate::storage::records::RecordSave<crate::operations::DomainOutcome>,
    /// The destination Stream read during apply.
    pub stream: crate::model::Stream,
}

/// Reuse the Stream read during apply when constructing the resulting native context.
/// A retried move finds the Session already in its destination and changes nothing.
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
        let Record::Session(session) = &before.record else {
            return Err(ValidationError::new("membership target must be a Session").into());
        };
        let source_key =
            RecordAddress::new(&session.host_id, RecordKind::Stream, &session.stream_id)?;
        let source = reader.read(&source_key)?;
        let Some(crate::storage::records::RecordRead {
            record: Record::Stream(source),
            ..
        }) = source
        else {
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
            let Some(crate::storage::records::RecordRead {
                record: Record::Stream(destination),
                ..
            }) = reader.read(&key)?
            else {
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
