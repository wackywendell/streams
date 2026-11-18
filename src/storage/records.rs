//! Record-local persistence: exact addresses and one target per save.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Record, RecordKind, check_text};

use super::StorageError;

/// Host is part of every portable record's identity, including Streams.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RecordAddress {
    host: String,
    kind: RecordKind,
    id: String,
}

impl RecordAddress {
    pub fn new(
        host: impl Into<String>,
        kind: RecordKind,
        id: impl Into<String>,
    ) -> Result<Self, StorageError> {
        let address = Self {
            host: host.into(),
            kind,
            id: id.into(),
        };
        address.validate()?;
        Ok(address)
    }

    pub fn for_record(record: &Record) -> Self {
        let host = match record {
            Record::Stream(record) => &record.owner_host_id,
            Record::Session(record) => &record.host_id,
            Record::Host(record) => &record.id,
        };
        Self {
            host: host.clone(),
            kind: record.kind(),
            id: record.key().id().into(),
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }
    pub fn kind(&self) -> RecordKind {
        self.kind
    }
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn validate(&self) -> Result<(), StorageError> {
        for (name, text) in [("record host", &self.host), ("record ID", &self.id)] {
            check_text(text, name, false).map_err(|e| StorageError::new(e.to_string()))?;
        }
        if self.kind == RecordKind::Host && self.id != self.host {
            return Err(StorageError::new("host record ID must match its partition"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRead {
    pub record: Record,
}

/// A record file whose bytes cannot be decoded, named by kind and catalog path, with the cause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnreadableRecord {
    pub kind: RecordKind,
    /// The host whose partition holds the file; `/streams reset` repairs only local records.
    pub host: String,
    pub path: PathBuf,
    pub cause: String,
    /// How to repair it. Frontends display this text; they do not compose their own.
    pub repair: String,
}

impl UnreadableRecord {
    pub fn new(address: &RecordAddress, path: PathBuf, cause: String) -> Self {
        let repair = match address.kind() {
            RecordKind::Stream => {
                "Run /streams reset in a conversation on that Stream, or delete or fix the file."
            }
            RecordKind::Session => {
                "Run /streams reset in that conversation, or delete or fix the file."
            }
            RecordKind::Host => "Delete or fix the file.",
        };
        Self {
            kind: address.kind(),
            host: address.host().to_owned(),
            path,
            cause,
            repair: repair.to_owned(),
        }
    }
}

impl std::fmt::Display for UnreadableRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            RecordKind::Host => "Host",
            RecordKind::Stream => "Stream",
            RecordKind::Session => "Session",
        };
        write!(
            f,
            "{kind} record {} is unreadable: {}",
            self.path.display(),
            self.cause
        )
    }
}

/// A listing holds every record it could decode and names each one it could not, so one
/// unreadable file never hides the rest. The catalog layout itself must still be readable.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordListing {
    pub records: Vec<RecordRead>,
    pub unreadable: Vec<UnreadableRecord>,
}

impl RecordListing {
    /// For callers that act on every record: the first unreadable record fails the call.
    pub fn complete(self) -> Result<Vec<RecordRead>, StorageError> {
        match self.unreadable.into_iter().next() {
            Some(unreadable) => Err(unreadable.into()),
            None => Ok(self.records),
        }
    }
}

/// The record a save callback wants stored, with its typed domain outcome.
/// Returning the current record unchanged skips the write.
pub struct RecordChange<T> {
    pub record: Record,
    pub outcome: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordSave<T> {
    pub current: RecordRead,
    pub outcome: T,
    /// False when the record was already as requested, so nothing was written.
    pub written: bool,
}

impl<T> RecordSave<T> {
    /// Names the saved record in a response that reports several saves.
    pub fn target(&self) -> String {
        format!(
            "{}:{}",
            self.current.record.kind().as_ref(),
            self.current.record.key().id()
        )
    }

    /// The `local` save status every caller reports.
    pub fn local(&self) -> &'static str {
        if self.written {
            "persisted"
        } else {
            "unchanged"
        }
    }
}

/// The shell may read named dependencies; it has no enumeration or write capability.
pub trait RecordReader {
    fn read(&self, address: &RecordAddress) -> Result<Option<RecordRead>, StorageError>;
}

/// Explicit human discovery, inspection and sync, separate from a point reader's capability.
pub trait RecordInventory {
    fn inventory(&self) -> Result<Vec<RecordRead>, StorageError>;
    /// Every host's records, without the relationship checks `inventory` applies.
    fn listing(&self) -> Result<RecordListing, StorageError>;
    fn host_sessions(&self, host: &str) -> Result<RecordListing, StorageError>;
    fn host_streams(&self, host: &str) -> Result<RecordListing, StorageError>;
}

/// Serialization belongs to the adapter. No action manages a multi-file transaction.
pub trait RecordStore {
    fn with_reader<T, E: From<StorageError>>(
        &self,
        read: impl FnOnce(&dyn RecordReader) -> Result<T, E>,
    ) -> Result<T, E>;

    /// Read the target once, then admit and atomically replace it. The callback is the
    /// shell: read explicit dependencies, then call pure apply. A change that leaves the
    /// record as it is writes nothing; otherwise the store advances the record's revision
    /// and update time, which no other code sets.
    fn save<T, E: From<StorageError>>(
        &self,
        target: &RecordAddress,
        apply: impl FnOnce(&dyn RecordReader, Option<&RecordRead>) -> Result<RecordChange<T>, E>,
    ) -> Result<RecordSave<T>, E>;
}
