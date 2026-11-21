//! Record-local persistence: exact addresses and one target per save.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::model::{Record, RecordKind, Session, Stream, check_token};

use super::StorageError;

/// Host is part of every portable record's identity; Sessions also include harness.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RecordAddress {
    host: String,
    kind: RecordKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<String>,
    id: String,
}

impl RecordAddress {
    pub fn host_record(host: impl Into<String>) -> Result<Self, StorageError> {
        let host = host.into();
        let address = Self {
            id: host.clone(),
            host,
            kind: RecordKind::Host,
            harness: None,
        };
        address.validate()?;
        Ok(address)
    }

    pub fn stream(host: impl Into<String>, id: impl Into<String>) -> Result<Self, StorageError> {
        let address = Self {
            host: host.into(),
            kind: RecordKind::Stream,
            harness: None,
            id: id.into(),
        };
        address.validate()?;
        Ok(address)
    }

    pub fn session(
        host: impl Into<String>,
        harness: impl Into<String>,
        id: impl Into<String>,
    ) -> Result<Self, StorageError> {
        let address = Self {
            host: host.into(),
            kind: RecordKind::Session,
            harness: Some(harness.into()),
            id: id.into(),
        };
        address.validate()?;
        Ok(address)
    }

    pub fn for_stream(stream: &Stream) -> Self {
        Self {
            host: stream.host_id.clone(),
            kind: RecordKind::Stream,
            harness: None,
            id: stream.id.clone(),
        }
    }

    pub fn for_session(session: &Session) -> Self {
        Self {
            host: session.host_id.clone(),
            kind: RecordKind::Session,
            harness: Some(session.harness.clone()),
            id: session.id.clone(),
        }
    }

    pub fn for_record(record: &Record) -> Self {
        match record {
            Record::Stream(record) => Self::for_stream(record),
            Record::Session(record) => Self::for_session(record),
            Record::Host(record) => Self {
                host: record.id.clone(),
                kind: RecordKind::Host,
                harness: None,
                id: record.id.clone(),
            },
        }
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn kind(&self) -> RecordKind {
        self.kind
    }

    pub fn harness(&self) -> Option<&str> {
        self.harness.as_deref()
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// Full record key (`HOST/ID` for Stream, `HOST/HARNESS/ID` for Session, `HOST` for Host).
    pub fn full_key(&self) -> String {
        match self.kind {
            RecordKind::Host => self.host.clone(),
            RecordKind::Stream => format!("{}/{}", self.host, self.id),
            RecordKind::Session => format!(
                "{}/{}/{}",
                self.host,
                self.harness.as_deref().unwrap_or(""),
                self.id
            ),
        }
    }

    pub fn validate(&self) -> Result<(), StorageError> {
        check_token(&self.host, "record host").map_err(|e| StorageError::new(e.to_string()))?;
        check_token(&self.id, "record ID").map_err(|e| StorageError::new(e.to_string()))?;
        match (self.kind, &self.harness) {
            (RecordKind::Session, Some(harness)) => {
                check_token(harness, "record harness")
                    .map_err(|e| StorageError::new(e.to_string()))?;
            }
            (RecordKind::Session, None) => {
                return Err(StorageError::new("Session address requires a harness"));
            }
            (_, Some(_)) => {
                return Err(StorageError::new("only Session addresses carry a harness"));
            }
            (RecordKind::Host, None) if self.id != self.host => {
                return Err(StorageError::new("host record ID must match its partition"));
            }
            _ => {}
        }
        Ok(())
    }
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
            "{kind} record {} is unreadable: {} ({})",
            self.path.display(),
            self.cause,
            self.repair
        )
    }
}

/// A listing holds every record it could decode and names each one it could not, so one
/// unreadable file never hides the rest. The catalog layout itself must still be readable.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecordListing {
    pub records: Vec<Record>,
    pub unreadable: Vec<UnreadableRecord>,
}

impl RecordListing {
    /// For callers that act on every record: the first unreadable record fails the call.
    pub fn complete(self) -> Result<Vec<Record>, StorageError> {
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
    pub current: Record,
    pub outcome: T,
    /// False when the record was already as requested, so nothing was written.
    pub written: bool,
}

impl<T> RecordSave<T> {
    pub fn target(&self) -> String {
        self.current.key().to_string()
    }

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
    fn read(&self, address: &RecordAddress) -> Result<Option<Record>, StorageError>;

    /// The Stream `host` owns with this ID.
    fn read_stream(&self, host: &str, id: &str) -> Result<Option<Stream>, StorageError> {
        match self.read(&RecordAddress::stream(host, id)?)? {
            None => Ok(None),
            Some(Record::Stream(stream)) => Ok(Some(stream)),
            Some(_) => Err(StorageError::new("a Stream address holds another record")),
        }
    }

    /// The Session `host` holds for `(harness, id)`.
    fn read_session(
        &self,
        host: &str,
        harness: &str,
        id: &str,
    ) -> Result<Option<Session>, StorageError> {
        match self.read(&RecordAddress::session(host, harness, id)?)? {
            None => Ok(None),
            Some(Record::Session(session)) => Ok(Some(session)),
            Some(_) => Err(StorageError::new("a Session address holds another record")),
        }
    }
}

/// Explicit human discovery, inspection and sync, separate from a point reader's capability.
pub trait RecordInventory {
    fn inventory(&self) -> Result<Vec<Record>, StorageError>;
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
    /// and update time. Only the store sets them: here, and when a repair replaces an
    /// unreadable file with a first revision.
    fn save<T, E: From<StorageError>>(
        &self,
        target: &RecordAddress,
        apply: impl FnOnce(&dyn RecordReader, Option<&Record>) -> Result<RecordChange<T>, E>,
    ) -> Result<RecordSave<T>, E>;
}
