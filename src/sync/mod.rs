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
use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumString};

/// How recently the remote view was fetched; it does not describe publication.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, EnumString, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Freshness {
    LocalOnly,
    Cached,
    Refreshed,
    Failed,
}

/// A projected read view alongside the actual local snapshot and known remote revision.
/// Projection does not persist remote records or acknowledge outgoing operations.
#[derive(Debug, Clone, PartialEq)]
pub struct ReadResult {
    pub catalog: Catalog,
    pub local: Snapshot,
    pub confirmed: Option<Snapshot>,
    pub freshness: Freshness,
    pub fetched_at: Option<f64>,
    pub conflicts: Vec<replay::ReplayConflict>,
    pub detail: String,
}
