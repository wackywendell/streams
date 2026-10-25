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

/// A stale-revision rejection never writes, so it cannot also be uncertain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageErrorKind {
    Definite,
    Uncertain,
    RevisionConflict,
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

    /// Constructs a definite rejection of a save based on stale revisions.
    pub fn revision_conflict(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind: StorageErrorKind::RevisionConflict,
        }
    }

    pub fn kind(&self) -> StorageErrorKind {
        self.kind
    }
    pub fn is_uncertain(&self) -> bool {
        self.kind == StorageErrorKind::Uncertain
    }
    pub fn is_revision_conflict(&self) -> bool {
        self.kind == StorageErrorKind::RevisionConflict
    }

    // Once replacement occurred, any subsequent failure must preserve that ambiguity.
    pub(crate) fn after_write(mut self) -> Self {
        self.kind = StorageErrorKind::Uncertain;
        self
    }
}
