//! Storage contracts and snapshot types; no YAML, paths, or Git requirements.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{Catalog, RecordKey};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct StorageError {
    pub message: String,
    pub uncertain: bool,
    pub is_revision_conflict: bool,
}

impl StorageError {
    pub fn new(message: impl Into<String>, uncertain: bool) -> Self {
        Self {
            message: message.into(),
            uncertain,
            is_revision_conflict: false,
        }
    }

    pub fn revision_conflict(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            uncertain: false,
            is_revision_conflict: true,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub catalog: Catalog,
    pub revisions: BTreeMap<RecordKey, String>,
}
