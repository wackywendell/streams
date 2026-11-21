//! Host-partitioned addressed records; no legacy locations or catalog-wide snapshots.
//! Opening this store never imports, scans or falls back to that legacy layout.

mod artifacts;
mod inventory;
pub use artifacts::{artifact_key_from_path, decode_artifact_content, relative_artifact_path};
pub use inventory::{address_from_relative_path, catalog_path_host, decode_record_tree};

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{
    codec::{RESET_HINT, load_json, record_from_data, record_to_data},
    file_io::{
        atomic_write, decode_json, ensure_directory, existing_write_lock, inspection_lock, lock,
        read_bytes, write_json,
    },
    model::{Record, RecordKind, Stamps, check_token},
    storage::{
        StorageError,
        records::{
            RecordAddress, RecordChange, RecordReader, RecordSave, RecordStore, UnreadableRecord,
        },
    },
};

const FORMAT: &str = ".catalog-local/records.json";
/// Bumped whenever record files change shape. Older catalogs are reset, never read.
const FORMAT_VERSION: u32 = 5;

/// Canonical relative paths for format 5; callers never probe legacy locations.
pub fn relative_record_path(address: &RecordAddress) -> Result<PathBuf, StorageError> {
    address.validate()?;
    Ok(record_path(address))
}

/// The path an address names. Every component satisfies `check_token`.
fn record_path(address: &RecordAddress) -> PathBuf {
    let partition = PathBuf::from("hosts").join(address.host());
    match address.kind() {
        RecordKind::Host => partition.join("host.json"),
        RecordKind::Stream => partition
            .join("streams")
            .join(format!("{}.json", address.id())),
        RecordKind::Session => partition
            .join("sessions")
            .join(address.harness().expect("Session address carries harness"))
            .join(address.id())
            .join("session.json"),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Format {
    version: u32,
    host: String,
}

fn invalid(error: impl std::fmt::Display) -> StorageError {
    StorageError::new(error.to_string())
}

/// Actual adapter I/O attempts, with absolute paths, for fixture read budgets.
/// Lock opens are separate from content reads; metadata checks are not listings.
/// Byte events count returned contents and installed record replacements, not initialization
/// or transport writes. Failed partial I/O and filesystem metadata are not byte transfers here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordIoEvent {
    Read(PathBuf),
    Lock(PathBuf),
    List(PathBuf),
    BytesRead(usize),
    Written(PathBuf, usize),
}

impl RecordIoEvent {
    pub fn is_access(&self) -> bool {
        matches!(self, Self::Read(_) | Self::Lock(_) | Self::List(_))
    }

    pub fn bytes_read(&self) -> usize {
        match self {
            Self::BytesRead(bytes) => *bytes,
            _ => 0,
        }
    }

    pub fn written(&self) -> Option<(&Path, usize)> {
        match self {
            Self::Written(path, bytes) => Some((path, *bytes)),
            _ => None,
        }
    }
}

/// The producer is fixed on this handle and checked against its local format marker.
pub struct FileRecordStore {
    root: PathBuf,
    host: String,
    observer: Option<Box<dyn Fn(RecordIoEvent) + Send + Sync>>,
}

impl FileRecordStore {
    pub fn new(root: impl AsRef<Path>, host: impl Into<String>) -> Result<Self, StorageError> {
        let host = host.into();
        check_token(&host, "record-store host").map_err(invalid)?;
        Ok(Self {
            root: std::path::absolute(root).map_err(invalid)?,
            host,
            observer: None,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub fn with_io_observer(
        mut self,
        observe: impl Fn(RecordIoEvent) + Send + Sync + 'static,
    ) -> Self {
        self.observer = Some(Box::new(observe));
        self
    }

    fn observe(&self, event: RecordIoEvent) {
        if let Some(observe) = &self.observer {
            observe(event);
        }
    }

    fn observe_replacement(&self, path: PathBuf, bytes: usize, result: &Result<(), StorageError>) {
        if result.is_ok()
            || result
                .as_ref()
                .err()
                .is_some_and(StorageError::is_uncertain)
        {
            self.observe(RecordIoEvent::Written(path, bytes));
        }
    }

    /// Explicit initialization only. A populated catalog is never adopted.
    pub fn initialize(&self) -> Result<(), StorageError> {
        ensure_directory(&self.root, true)?;
        let path = self.root.join(".catalog.lock");
        self.observe(RecordIoEvent::Lock(path.clone()));
        let _lock = lock(&path)?;
        for entry in self.list_directory()? {
            if entry.map_err(invalid)?.file_name() != ".catalog.lock" {
                return Err(StorageError::new(format!(
                    "catalog initialization requires an empty directory; {RESET_HINT}"
                )));
            }
        }
        write_json(
            &self.root.join(FORMAT),
            &Format {
                version: FORMAT_VERSION,
                host: self.host.clone(),
            },
        )
    }

    fn scope(&self, write: bool) -> Result<Reader<'_>, StorageError> {
        ensure_directory(&self.root, false)?;
        let path = self.root.join(".catalog.lock");
        self.observe(RecordIoEvent::Lock(path.clone()));
        let lock = if write {
            existing_write_lock(&path)?
        } else {
            inspection_lock(&path)?
        };
        let bytes = self
            .read_named(Path::new(FORMAT))?
            .ok_or_else(|| StorageError::new(format!("catalog has no {FORMAT}; {RESET_HINT}")))?;
        let format: Format = decode_json(&bytes)?;
        if format.version != FORMAT_VERSION {
            return Err(StorageError::new(format!(
                "catalog records use format {} from another Streams version, not {FORMAT_VERSION} \
                 ({FORMAT}); {RESET_HINT}",
                format.version
            )));
        }
        if format.host != self.host {
            return Err(StorageError::new(format!(
                "catalog belongs to host {}, not {} ({FORMAT})",
                format.host, self.host
            )));
        }
        Ok(Reader {
            store: self,
            _lock: lock,
        })
    }

    fn list_directory(&self) -> Result<std::fs::ReadDir, StorageError> {
        self.observe(RecordIoEvent::List(self.root.clone()));
        std::fs::read_dir(&self.root).map_err(invalid)
    }

    fn read_named(&self, relative: &Path) -> Result<Option<Vec<u8>>, StorageError> {
        check_parents(&self.root, relative)?;
        let path = self.root.join(relative);
        self.observe(RecordIoEvent::Read(path.clone()));
        let bytes = read_bytes(&path)?;
        if let Some(bytes) = &bytes {
            self.observe(RecordIoEvent::BytesRead(bytes.len()));
        }
        Ok(bytes)
    }
}

/// Check each owned ancestor, including missing ones, without enumerating directories.
pub(crate) fn check_parents(root: &Path, relative: &Path) -> Result<(), StorageError> {
    let mut parent = root.to_path_buf();
    for part in relative
        .parent()
        .expect("relative record parent")
        .components()
    {
        parent.push(part);
        ensure_directory(&parent, false)?;
    }
    Ok(())
}

struct Reader<'a> {
    store: &'a FileRecordStore,
    _lock: crate::file_io::FileLock,
}

/// Decode already-observed bytes with exactly the same strict checks as a store read.
/// Watchers can establish the Stream watch before requesting context, without another read.
/// Every reader reports an undecodable record by kind and catalog path, with the cause.
pub fn decode_record(address: &RecordAddress, bytes: &[u8]) -> Result<Record, UnreadableRecord> {
    decode_record_bytes(address, bytes)
        .map_err(|error| UnreadableRecord::new(address, record_path(address), error.to_string()))
}

fn decode_record_bytes(address: &RecordAddress, bytes: &[u8]) -> Result<Record, StorageError> {
    let text = std::str::from_utf8(bytes).map_err(invalid)?;
    let record =
        record_from_data(address.kind(), &load_json(text).map_err(invalid)?).map_err(invalid)?;
    record.validate().map_err(invalid)?;
    if RecordAddress::for_record(&record) != *address {
        return Err(StorageError::new(
            "record identity does not match its physical location",
        ));
    }
    Ok(record)
}

impl RecordReader for Reader<'_> {
    fn read(&self, address: &RecordAddress) -> Result<Option<Record>, StorageError> {
        let Some(bytes) = self.store.read_named(&relative_record_path(address)?)? else {
            return Ok(None);
        };
        Ok(Some(decode_record(address, &bytes)?))
    }
}

impl RecordStore for FileRecordStore {
    fn with_reader<T, E: From<StorageError>>(
        &self,
        read: impl FnOnce(&dyn RecordReader) -> Result<T, E>,
    ) -> Result<T, E> {
        read(&self.scope(false)?)
    }

    fn save<T, E: From<StorageError>>(
        &self,
        target: &RecordAddress,
        apply: impl FnOnce(&dyn RecordReader, Option<&Record>) -> Result<RecordChange<T>, E>,
    ) -> Result<RecordSave<T>, E> {
        if target.host() != self.host {
            return Err(StorageError::new("cannot write another host's partition").into());
        }
        let scope = self.scope(true)?;
        self.save_scoped(&scope, target, apply)
    }
}

impl FileRecordStore {
    /// Sets a record's revision and update time on every save. The only other place is
    /// `discard_unreadable`, which starts a replacement at revision 1.
    fn save_scoped<T, E: From<StorageError>>(
        &self,
        scope: &Reader<'_>,
        target: &RecordAddress,
        apply: impl FnOnce(&dyn RecordReader, Option<&Record>) -> Result<RecordChange<T>, E>,
    ) -> Result<RecordSave<T>, E> {
        let previous = scope.read(target)?;
        let RecordChange {
            mut record,
            outcome,
        } = apply(scope, previous.as_ref())?;
        record.validate().map_err(invalid)?;
        if RecordAddress::for_record(&record) != *target {
            return Err(StorageError::new("save cannot change target identity or host").into());
        }
        // Stamps are the store's; compare the content the callback chose.
        let stamps = previous
            .as_ref()
            .and_then(|record| record.stamps())
            .unwrap_or_default();
        record.set_stamps(stamps);
        if let Some(current) = previous
            && current == record
        {
            return Ok(RecordSave {
                current,
                outcome,
                written: false,
            });
        }
        record.set_stamps(Stamps {
            revision: stamps.revision + 1,
            updated_at_ms: now_ms(),
        });
        self.write_record(target, &record)?;
        Ok(RecordSave {
            current: record,
            outcome,
            written: true,
        })
    }

    fn write_record(&self, target: &RecordAddress, record: &Record) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(&record_to_data(record)).map_err(invalid)?;
        let relative = relative_record_path(target)?;
        check_parents(&self.root, &relative)?;
        let path = self.root.join(relative);
        let saved = atomic_write(&path, &bytes, &self.root.join(".catalog-staging"));
        self.observe_replacement(path, bytes.len(), &saved);
        saved
    }

    /// Discard the target when its file exists but cannot be decoded: install `replacement`
    /// as a first revision, or remove the file when there is none.
    /// A readable or missing target is left alone. Returns the decode error it discarded.
    pub fn discard_unreadable(
        &self,
        target: &RecordAddress,
        replacement: Option<Record>,
    ) -> Result<Option<String>, StorageError> {
        if target.host() != self.host {
            return Err(StorageError::new("cannot write another host's partition"));
        }
        let _scope = self.scope(true)?;
        let relative = relative_record_path(target)?;
        let Some(bytes) = self.read_named(&relative)? else {
            return Ok(None);
        };
        let Err(unreadable) = decode_record(target, &bytes) else {
            return Ok(None);
        };
        match replacement {
            Some(mut record) => {
                record.validate().map_err(invalid)?;
                if RecordAddress::for_record(&record) != *target {
                    return Err(StorageError::new(
                        "a replacement must keep the target's identity",
                    ));
                }
                record.set_stamps(Stamps {
                    revision: 1,
                    updated_at_ms: now_ms(),
                });
                self.write_record(target, &record)?;
            }
            None => std::fs::remove_file(self.root.join(relative)).map_err(invalid)?,
        }
        Ok(Some(unreadable.to_string()))
    }
}

fn now_ms() -> u64 {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests;
