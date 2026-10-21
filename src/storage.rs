//! Storage contracts and snapshot types; no YAML, paths, or Git requirements.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::model::{Catalog, Record, RecordKey};

/// Catalog persistence and bookkeeping sharing one serialization scope.
///
/// Journal access is borrowed from a transaction and cannot outlive its lock scope.
pub trait Store {
    type Transaction<'a>: Transaction
    where
        Self: 'a;

    /// Holds serialization for the closure; its borrowed transaction cannot escape.
    /// Errors do not roll back earlier catalog or journal effects.
    fn with_transaction<T, E: From<StorageError>>(
        &self,
        f: impl FnOnce(&mut Self::Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E>;

    /// Read existing state without creating locks, directories, or bookkeeping.
    /// Adapters must opt in; ordinary transactions are not a read-only fallback.
    fn with_inspection<T, E: From<StorageError>>(
        &self,
        _f: impl FnOnce(&mut Self::Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        Err(StorageError::new("storage adapter does not support read-only inspection").into())
    }
}

/// Catalog access borrowed within a store's serialization scope.
pub trait Transaction {
    fn journal(&self) -> &dyn Journal;

    /// Rejects adapter-specific layouts incompatible with local-only operation.
    fn check_local(&self) -> Result<(), StorageError>;

    fn read(&self) -> Result<Snapshot, StorageError>;

    /// Saves one validated record only if the whole captured revision set is current.
    /// A stale rejection has no effects; a domain no-op preserves every revision.
    /// Failures after a possible replacement must be uncertain, not definite.
    fn save(&self, record: &Record, expected: &Snapshot) -> Result<Snapshot, StorageError>;
}

/// Durable versioned objects, serialized by the owning store's transaction.
/// Names are trusted bookkeeping keys, not paths. Objects include their version;
/// typed schema and version validation belong to the coordinator's state layer.
/// Write or clear failures may have taken effect; no rollback is promised.
pub trait Journal {
    fn read(&self, name: &str) -> Result<Option<Map<String, Value>>, StorageError>;
    fn write(&self, name: &str, data: Map<String, Value>) -> Result<(), StorageError>;
    fn clear(&self, name: &str) -> Result<(), StorageError>;
    fn names(&self) -> Result<Vec<String>, StorageError>;

    /// Describes a bookkeeping key in diagnostics without exposing path assumptions.
    fn describe(&self, name: &str) -> String {
        name.to_owned()
    }
}

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

/// Decoded records and opaque, store-bound revision tokens for optimistic saves.
///
/// Tokens are not domain values and must not be interpreted or moved between stores.
/// Equal catalogs can have different revisions; compare `catalog` for domain equality
/// and the whole `revisions` map to detect stale captured input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub catalog: Catalog,
    pub revisions: BTreeMap<RecordKey, String>,
}
