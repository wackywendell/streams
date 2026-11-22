//! Publication transports bytes; local record application is independent.

pub mod git_io;
pub mod outcomes;
pub mod records;
pub mod state;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    config::Strategy,
    file_store::records::FileRecordStore,
    storage::{StorageError, records::RecordInventory},
};

/// Deadline for one Git command or diagnostic native request.
pub const COMMAND_TIMEOUT_SECONDS: f64 = 30.0;

/// How `configure` should update a Git transport's configured remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteUpdate {
    Keep,
    Set(String),
    Clear,
}

/// Recorded outcome of the most recent sync attempt for a catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LastSyncState {
    pub attempted_at_ms: u64,
    pub succeeded_at_ms: Option<u64>,
    pub local: outcomes::LocalStatus,
    pub sharing: outcomes::SharingStatus,
    pub needs_attention: bool,
    pub detail: String,
}

/// Interchangeable catalog sync backend behind `.catalog-local/transport.json`.
pub trait SyncBackend {
    fn strategy(&self) -> Strategy;
    fn sync(&self) -> outcomes::Result;
    /// Non-blocking sync for background triggers; returns `None` if a sync is already in flight.
    fn try_sync(&self) -> Option<outcomes::Result>;
}

/// Local-only backend that validates records without Git or network access.
pub struct NoneBackend {
    root: PathBuf,
    host: String,
}

impl NoneBackend {
    pub fn new(root: impl Into<PathBuf>, host: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            host: host.into(),
        }
    }
}

impl SyncBackend for NoneBackend {
    fn strategy(&self) -> Strategy {
        Strategy::None
    }

    fn sync(&self) -> outcomes::Result {
        match FileRecordStore::new(&self.root, &self.host).and_then(|store| store.inventory()) {
            Ok(_) => outcomes::Result::new(
                outcomes::SharingStatus::LocalOnly,
                outcomes::LocalStatus::Unchanged,
            ),
            Err(error) => outcomes::Result::new(
                outcomes::SharingStatus::Blocked,
                outcomes::LocalStatus::RepairRequired,
            )
            .with_detail(error.to_string()),
        }
    }

    fn try_sync(&self) -> Option<outcomes::Result> {
        Some(self.sync())
    }
}

/// Open the catalog's configured sync backend behind [`SyncBackend`].
pub fn open_backend(root: &Path, host: &str) -> Result<Box<dyn SyncBackend>, StorageError> {
    let transport = records::RecordSync::open(root, host)?;
    Ok(match transport.strategy() {
        Strategy::None => Box::new(NoneBackend::new(root, host)),
        Strategy::Git => Box::new(transport),
    })
}

/// Read the last recorded sync outcome (if any) from `.catalog-local/last-sync.json`.
pub fn read_last_sync(root: &Path) -> Result<Option<LastSyncState>, StorageError> {
    records::read_last_sync(root)
}

/// Return true when automatic post-write sync is enabled, the catalog uses a non-`none`
/// backend, and at least `interval_minutes` have elapsed since the last sync attempt.
pub fn is_auto_sync_due(
    root: &Path,
    interval_minutes: u32,
    now_ms: u64,
) -> Result<bool, StorageError> {
    records::is_auto_sync_due(root, interval_minutes, now_ms)
}
