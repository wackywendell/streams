//! Host-bound admission and record-local catalog actions.

pub mod artifacts;
mod create;
pub mod documents;
pub mod integration;
pub mod lifecycle;
pub use create::create_record;
mod enrollment;
pub use enrollment::{EnrollmentSave, enroll_session, enroll_session_observed, enroll_stream};
mod record_update;
mod records;
pub use record_update::{save_record_update, save_user_edit, save_user_references};
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

/// Same-pass data, never serialized into the retained decision.
pub struct MoveSave {
    pub save: crate::storage::records::RecordSave,
    /// Fresh decisions supply their resulting Stream. Retried decisions read no dependencies.
    pub stream: Option<crate::model::Stream>,
}

/// Reuse the Stream read during apply when constructing the resulting native context.
/// A retained retry leaves the caller's already-observed current Stream in use.
pub fn move_session_observed(
    store: &impl crate::storage::records::RecordStore,
    operation_id: &str,
    intent: &crate::operations::membership::MoveIntent,
) -> Result<MoveSave> {
    use crate::{
        model::{Record, RecordKind},
        operations::{DomainOutcome, membership::apply_move},
        storage::records::{RecordAddress, RecordChange, RecordRequest},
    };
    let address = RecordAddress::new(intent.host_id(), RecordKind::Session, intent.session_id())?;
    let request = RecordRequest::new(address, operation_id, &serde_json::json!(intent))?;
    let mut stream = None;
    let save = store.save::<ApplicationError>(&request, |reader, before| {
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
        let receipt = serde_json::json!({
            "outcome": outcome,
            "session_id": after.id,
            "from_stream_id": session.stream_id,
            "stream_id": after.stream_id,
            "membership_period": after.previous_stream_ids.len(),
        });
        Ok(RecordChange {
            record: Record::Session(after),
            outcome: receipt.as_object().expect("receipt object").clone(),
        })
    })?;
    Ok(MoveSave { save, stream })
}
