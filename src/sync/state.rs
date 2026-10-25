//! Explicit refusal shared by transport checks and complete model projections.

use crate::storage::StorageError;

/// State cannot safely support further effects without inspection or repair.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RepairRequired(pub String);

impl RepairRequired {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl From<StorageError> for RepairRequired {
    fn from(error: StorageError) -> Self {
        Self::new(error.to_string())
    }
}
