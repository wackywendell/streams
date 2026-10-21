//! Host-local checkout selections and verified saved-conversation identities.
//!
//! These files are execution-host state, never shared catalog records. Callers must
//! not publish their paths or use them to infer conversation membership.

pub mod arrangements;
pub mod checkouts;
pub mod conversations;
pub mod launch;
pub mod locations;
pub mod native;
pub mod native_state;
pub mod open_protocol;
pub mod revisions;
pub mod state;
pub mod workspace;

pub use checkouts::{CheckoutLocation, CheckoutRegistry, RepositoryInspection};
pub use locations::{BoundCheckout, DirectoryBinding, DirectoryBindings};

use std::fs::File;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::file_io;
use crate::storage::StorageError;

/// Local state failures distinguish absence/ambiguity from corruption and uncertain writes.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("invalid host-local state: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{message}; candidates: {candidates:?}")]
    Ambiguous {
        message: String,
        candidates: Vec<String>,
    },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Storage(#[from] StorageError),
    #[error("repository inspection failed: {0}")]
    Inspection(String),
    #[error("{0}")]
    Frontend(#[from] crate::frontend::FrontendError),
    #[error(
        "native open may be partial; {0}. Inspect native state before retrying; no rollback or automatic relaunch"
    )]
    OpenIncomplete(String),
}

impl RuntimeError {
    /// A failure after atomic replacement requires inspection before retrying.
    pub fn is_uncertain(&self) -> bool {
        match self {
            Self::Storage(error) => error.is_uncertain(),
            Self::OpenIncomplete(_) => true,
            Self::Frontend(error) => error
                .commands
                .iter()
                .any(|c| c.outcome == crate::frontend::CommandOutcome::Uncertain),
            _ => false,
        }
    }
}

pub(crate) type Result<T> = std::result::Result<T, RuntimeError>;

pub(crate) fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Invalid(message.into())
}

pub(crate) fn text(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(|c| c < ' ' || c == '\u{7f}') {
        return Err(invalid("expected nonempty text without control characters"));
    }
    Ok(())
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(file_io::decode_json(bytes)?)
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    Ok(file_io::read_json(path)?)
}

pub(crate) fn lock(path: &Path) -> Result<File> {
    Ok(file_io::lock(path)?)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    Ok(file_io::write_json(path, value)?)
}

pub(crate) fn digest(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value))
}
