//! Host-bound admission and record-local catalog actions.

pub mod artifacts;
mod create;
pub mod documents;
pub mod edit;
pub mod fields;
pub mod lifecycle;
mod listing;
mod records;

pub use create::{CreateSave, create_record, create_session, create_stream};
pub use edit::{EditResult, edit_session, edit_stream};
pub use listing::{
    ListedSession, ListedStream, SessionFilter, SessionList, StreamFilter, StreamList,
    list_sessions, list_streams,
};
pub use records::{
    DiscardedCard, OpenedRecords, configure_records, initialize_records, open_records,
    open_records_observed,
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
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{message}")]
    Ambiguous {
        message: String,
        candidates: Vec<String>,
    },
}

impl ApplicationError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn ambiguous(message: impl Into<String>, candidates: Vec<String>) -> Self {
        Self::Ambiguous {
            message: message.into(),
            candidates,
        }
    }
}

pub type Result<T> = std::result::Result<T, ApplicationError>;
