//! Catalog backends keep local persistence and publication outcomes independent.

pub mod adoption;
pub mod baseline;
mod coordinator;
pub mod envelope;
pub mod git;
pub mod git_io;
pub mod history;
pub mod local;
pub mod outcomes;
pub mod policy;
pub mod protocol;
pub mod records;
pub mod replay;
pub mod scalars;
pub mod state;

pub use scalars::{Oid, Seconds};

use crate::{
    model::{Catalog, ValidationError},
    operations::{Authority, Operation},
    storage::{Snapshot, Store},
};
use serde::Serialize;
use strum::{AsRefStr, Display, EnumString, VariantNames};

/// How recently the remote view was fetched; it does not describe publication.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Freshness {
    LocalOnly,
    Cached,
    Refreshed,
    Failed,
}

crate::codec::deserialize_string_enum!(Freshness);

/// Durable records and effective authority captured in one read-only scope.
#[derive(Debug, Clone)]
pub struct Inspection {
    pub snapshot: Snapshot,
    pub authorities: std::collections::BTreeMap<String, Authority>,
}

/// Application-facing catalog operations, independent of storage and transport choice.
///
/// Implementations preserve operation IDs and captured preconditions across retries.
/// Identical intent is idempotent; reuse of an ID with different intent fails closed.
/// Local durability and sharing are separate facts, including after uncertain effects.
/// This interface does not require Git history, branch adoption, or network access.
/// See the architecture ADR for common and backend-specific conformance laws.
pub trait CatalogBackend {
    /// Returns actual locally persisted records and their storage preconditions.
    fn snapshot(&self) -> Result<Snapshot, state::RepairRequired>;

    /// Never repairs receipts, refreshes projected state, or creates local machinery.
    fn inspect(&self) -> Result<Inspection, state::RepairRequired> {
        Err(state::RepairRequired::new(
            "backend does not support read-only inspection",
        ))
    }

    /// Returns effective ownership evidence, never authority inferred from a proposal.
    fn authority(&self, stream_id: &str) -> Result<Option<Authority>, state::RepairRequired>;

    /// Applies original captured intent; a repair receipt must not be blindly retried.
    fn save(
        &self,
        operation: &Operation,
        origin: Option<&envelope::Origin>,
    ) -> Result<outcomes::Result, ValidationError>;

    /// Optionally refreshes a view without publishing outgoing operations.
    fn read(&self, refresh: bool) -> Result<ReadResult, state::RepairRequired>;

    /// Attempts synchronization under the event's timing policy, reporting its evidence.
    fn sync(&self, event: policy::SyncEvent) -> outcomes::Result;
}

impl<S: Store> CatalogBackend for local::LocalCatalog<S> {
    fn inspect(&self) -> Result<Inspection, state::RepairRequired> {
        local::LocalCatalog::inspect(self)
    }

    fn snapshot(&self) -> Result<Snapshot, state::RepairRequired> {
        local::LocalCatalog::snapshot(self)
    }

    fn authority(&self, stream_id: &str) -> Result<Option<Authority>, state::RepairRequired> {
        local::LocalCatalog::authority(self, stream_id)
    }

    fn save(
        &self,
        operation: &Operation,
        origin: Option<&envelope::Origin>,
    ) -> Result<outcomes::Result, ValidationError> {
        local::LocalCatalog::save(self, operation, origin)
    }

    fn read(&self, refresh: bool) -> Result<ReadResult, state::RepairRequired> {
        local::LocalCatalog::read(self, refresh)
    }

    fn sync(&self, event: policy::SyncEvent) -> outcomes::Result {
        local::LocalCatalog::sync(self, event)
    }
}

impl CatalogBackend for git::GitCatalog {
    fn inspect(&self) -> Result<Inspection, state::RepairRequired> {
        git::GitCatalog::inspect(self)
    }

    fn snapshot(&self) -> Result<Snapshot, state::RepairRequired> {
        git::GitCatalog::snapshot(self)
    }

    fn authority(&self, stream_id: &str) -> Result<Option<Authority>, state::RepairRequired> {
        git::GitCatalog::authority(self, stream_id)
    }

    fn save(
        &self,
        operation: &Operation,
        origin: Option<&envelope::Origin>,
    ) -> Result<outcomes::Result, ValidationError> {
        git::GitCatalog::save(self, operation, origin)
    }

    fn read(&self, refresh: bool) -> Result<ReadResult, state::RepairRequired> {
        git::GitCatalog::read(self, refresh)
    }

    fn sync(&self, event: policy::SyncEvent) -> outcomes::Result {
        git::GitCatalog::sync(self, event)
    }
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
