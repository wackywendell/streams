//! Record-local persistence: exact addresses, one target and its embedded retry evidence.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

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

/// Captured intent is supplied by the domain, never reconstructed from current records.
#[derive(Debug, Clone)]
pub struct RecordRequest {
    pub(crate) target: RecordAddress,
    pub(crate) operation_id: String,
    pub(crate) fingerprint: String,
}

impl RecordRequest {
    pub fn new(
        target: RecordAddress,
        operation_id: impl Into<String>,
        intent: &Value,
    ) -> Result<Self, StorageError> {
        target.validate()?;
        let operation_id = operation_id.into();
        check_text(&operation_id, "operation ID", false)
            .map_err(|e| StorageError::new(e.to_string()))?;
        let bytes =
            serde_json::to_vec(&(&target, intent)).map_err(|e| StorageError::new(e.to_string()))?;
        Ok(Self {
            target,
            operation_id,
            fingerprint: format!("{:x}", Sha256::digest(bytes)),
        })
    }

    pub fn target(&self) -> &RecordAddress {
        &self.target
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRead {
    pub record: Record,
    pub revision: String,
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

/// Only the target can be replaced. Domain rejection returns an error instead.
pub struct RecordChange {
    pub record: Record,
    pub outcome: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordSave {
    pub current: RecordRead,
    pub outcome: Map<String, Value>,
    pub repeated: bool,
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
    fn last_updated_ms(
        &self,
        addresses: &[RecordAddress],
    ) -> Result<std::collections::BTreeMap<RecordAddress, u64>, StorageError>;
}

/// Serialization belongs to the adapter. No action manages a multi-file transaction.
pub trait RecordStore {
    fn with_reader<T, E: From<StorageError>>(
        &self,
        read: impl FnOnce(&dyn RecordReader) -> Result<T, E>,
    ) -> Result<T, E>;

    /// Read the target once, handle its receipt, then admit and atomically replace it.
    /// The callback is the shell: read explicit dependencies, then call pure apply.
    fn save<E: From<StorageError>>(
        &self,
        request: &RecordRequest,
        apply: impl FnOnce(&dyn RecordReader, Option<&RecordRead>) -> Result<RecordChange, E>,
    ) -> Result<RecordSave, E>;
}
