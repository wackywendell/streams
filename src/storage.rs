//! Record storage contracts and explicit uncertainty; no transport requirements.

pub mod records;

/// A storage failure, including whether an effect may already have taken place.
///
/// An uncertain failure is not proof that a write was lost: inspect persisted state
/// before retrying. A definite failure does not by itself describe other operations.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct StorageError {
    pub message: String,
    kind: StorageErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageErrorKind {
    Definite,
    Uncertain,
    /// A definite failure to decode one record, which a listing names instead of failing.
    Unreadable(Box<records::UnreadableRecord>),
}

impl From<records::UnreadableRecord> for StorageError {
    fn from(record: records::UnreadableRecord) -> Self {
        Self {
            message: record.to_string(),
            kind: StorageErrorKind::Unreadable(Box::new(record)),
        }
    }
}

impl StorageError {
    /// Constructs a definite failure; this does not roll back earlier coordinator effects.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: StorageErrorKind::Definite,
        }
    }

    /// Constructs a failure whose effect must be inspected before retrying.
    pub fn uncertain(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: StorageErrorKind::Uncertain,
        }
    }

    pub fn is_uncertain(&self) -> bool {
        self.kind == StorageErrorKind::Uncertain
    }

    /// The record this error names when it is a decode failure; otherwise the error itself.
    pub fn into_unreadable(self) -> Result<records::UnreadableRecord, Self> {
        match self.kind {
            StorageErrorKind::Unreadable(record) => Ok(*record),
            _ => Err(self),
        }
    }

    // Once replacement occurred, any subsequent failure must preserve that ambiguity.
    pub(crate) fn after_write(mut self) -> Self {
        self.kind = StorageErrorKind::Uncertain;
        self
    }
}
