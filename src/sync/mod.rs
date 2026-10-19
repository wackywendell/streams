//! Catalog backends keep local persistence and publication outcomes independent.

pub mod baseline;
pub mod envelope;
pub mod git_io;
pub mod history;
pub mod local;
pub mod outcomes;
pub mod policy;
pub mod protocol;
pub mod replay;
pub mod state;

use crate::{model::Catalog, storage::Snapshot};

#[derive(Debug, Clone, PartialEq)]
pub struct ReadResult {
    pub catalog: Catalog,
    pub local: Snapshot,
    pub confirmed: Option<Snapshot>,
    pub freshness: String,
    pub fetched_at: Option<f64>,
    pub conflicts: Vec<serde_json::Value>,
    pub detail: String,
}
