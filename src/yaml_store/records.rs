//! Host-partitioned addressed records; no legacy locations or catalog-wide snapshots.
//! Opening this store never imports, scans or falls back to that legacy layout.

mod artifacts;
mod inventory;
pub use artifacts::{artifact_key_from_path, decode_artifact_content, relative_artifact_path};
pub(crate) use inventory::record_mtime_ms;
pub use inventory::{address_from_relative_path, catalog_path_host, decode_record_tree};

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::{
    codec::{record_from_data, record_to_data},
    file_io::{
        atomic_write, decode_json, ensure_directory, existing_write_lock, inspection_lock, lock,
        read_bytes, write_json,
    },
    model::{Record, RecordKind, check_text},
    storage::{
        StorageError,
        records::{
            RecordAddress, RecordChange, RecordRead, RecordReader, RecordRequest, RecordSave,
            RecordStore, UnreadableRecord,
        },
    },
};

use super::{encode_component, yaml_to_json};

const FORMAT: &str = ".catalog-local/records.json";

/// Canonical relative paths for the new format; callers never probe legacy locations.
pub fn relative_record_path(address: &RecordAddress) -> Result<PathBuf, StorageError> {
    address.validate()?;
    let partition = PathBuf::from("hosts").join(encode_component(address.host()));
    let name = format!("{}.yaml", encode_component(address.id()));
    Ok(match address.kind() {
        RecordKind::Host => partition.join("host.yaml"),
        RecordKind::Stream => partition.join("streams").join(name),
        RecordKind::Session => partition.join("conversations").join(name),
    })
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Format {
    version: u32,
    host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    fingerprint: String,
    outcome: Map<String, Value>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    revision: String,
    kind: RecordKind,
    record: Value,
    #[serde(deserialize_with = "crate::codec::object_map")]
    receipts: BTreeMap<String, Receipt>,
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, StorageError> {
    let text = std::str::from_utf8(bytes).map_err(invalid)?;
    let yaml = saneyaml::LoadOptions::yaml_1_1()
        .from_str(text)
        .map_err(invalid)?;
    let json = yaml_to_json(yaml).map_err(invalid)?;
    if !json.is_object() {
        return Err(StorageError::new("expected a record object"));
    }
    serde_json::from_value(json).map_err(invalid)
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
pub struct YamlRecordStore {
    root: PathBuf,
    host: String,
    observer: Option<Box<dyn Fn(RecordIoEvent) + Send + Sync>>,
}

impl YamlRecordStore {
    pub fn new(root: impl AsRef<Path>, host: impl Into<String>) -> Result<Self, StorageError> {
        let host = host.into();
        check_text(&host, "record-store host", false).map_err(invalid)?;
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

    /// Explicit initialization only. A populated old catalog requires offline cutover.
    pub fn initialize(&self) -> Result<(), StorageError> {
        ensure_directory(&self.root, true)?;
        let path = self.root.join(".catalog.lock");
        self.observe(RecordIoEvent::Lock(path.clone()));
        let _lock = lock(&path)?;
        for entry in self.list_directory()? {
            if entry.map_err(invalid)?.file_name() != ".catalog.lock" {
                return Err(StorageError::new(
                    "new-format initialization requires an empty catalog; run offline cutover",
                ));
            }
        }
        write_json(
            &self.root.join(FORMAT),
            &Format {
                version: 2,
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
            .ok_or_else(|| StorageError::new("record format is incomplete; run offline cutover"))?;
        let format: Format = decode_json(&bytes)?;
        if format.version != 2 || format.host != self.host {
            return Err(StorageError::new(
                "record format or producer mismatch; run offline cutover",
            ));
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
    store: &'a YamlRecordStore,
    _lock: crate::file_io::FileLock,
}

impl Reader<'_> {
    fn envelope(
        &self,
        address: &RecordAddress,
    ) -> Result<Option<(Envelope, RecordRead)>, StorageError> {
        let Some(bytes) = self.store.read_named(&relative_record_path(address)?)? else {
            return Ok(None);
        };
        decode_envelope(address, &bytes).map(Some)
    }
}

/// Decode already-observed bytes with exactly the same strict checks as a store read.
/// Watchers can establish the Stream watch before requesting context, without another read.
pub fn decode_record(address: &RecordAddress, bytes: &[u8]) -> Result<RecordRead, StorageError> {
    Ok(decode_envelope(address, bytes)?.1)
}

/// Every reader reports an undecodable record by kind and catalog path, with the cause.
fn decode_envelope(
    address: &RecordAddress,
    bytes: &[u8],
) -> Result<(Envelope, RecordRead), StorageError> {
    let path = relative_record_path(address)?;
    decode_envelope_bytes(address, bytes)
        .map_err(|error| UnreadableRecord::new(address, path, error.to_string()).into())
}

fn decode_envelope_bytes(
    address: &RecordAddress,
    bytes: &[u8],
) -> Result<(Envelope, RecordRead), StorageError> {
    let envelope: Envelope = decode(bytes)?;
    if envelope.version != 2 || envelope.kind != address.kind() {
        return Err(StorageError::new("unsupported or mismatched record schema"));
    }
    check_text(&envelope.revision, "record revision", false).map_err(invalid)?;
    let record = record_from_data(envelope.kind, &envelope.record).map_err(invalid)?;
    record.validate_portable().map_err(invalid)?;
    if RecordAddress::for_record(&record) != *address {
        return Err(StorageError::new(
            "record identity does not match its physical location",
        ));
    }
    for (id, receipt) in &envelope.receipts {
        check_text(id, "receipt operation ID", false).map_err(invalid)?;
        if receipt.fingerprint.len() != 64
            || !receipt
                .fingerprint
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(StorageError::new("invalid receipt fingerprint"));
        }
    }
    let current = RecordRead {
        record,
        revision: envelope.revision.clone(),
    };
    Ok((envelope, current))
}

impl RecordReader for Reader<'_> {
    fn read(&self, address: &RecordAddress) -> Result<Option<RecordRead>, StorageError> {
        Ok(self.envelope(address)?.map(|(_, current)| current))
    }
}

impl RecordStore for YamlRecordStore {
    fn with_reader<T, E: From<StorageError>>(
        &self,
        read: impl FnOnce(&dyn RecordReader) -> Result<T, E>,
    ) -> Result<T, E> {
        read(&self.scope(false)?)
    }

    fn save<E: From<StorageError>>(
        &self,
        request: &RecordRequest,
        apply: impl FnOnce(&dyn RecordReader, Option<&RecordRead>) -> Result<RecordChange, E>,
    ) -> Result<RecordSave, E> {
        if request.target.host() != self.host {
            return Err(StorageError::new("cannot write another host's partition").into());
        }
        let scope = self.scope(true)?;
        self.save_scoped(&scope, request, apply)
    }
}

impl YamlRecordStore {
    fn save_scoped<E: From<StorageError>>(
        &self,
        scope: &Reader<'_>,
        request: &RecordRequest,
        apply: impl FnOnce(&dyn RecordReader, Option<&RecordRead>) -> Result<RecordChange, E>,
    ) -> Result<RecordSave, E> {
        let previous = scope.envelope(&request.target)?;
        if let Some((envelope, current)) = &previous
            && let Some(receipt) = envelope.receipts.get(&request.operation_id)
        {
            if receipt.fingerprint != request.fingerprint {
                return Err(StorageError::new("operation ID reused with changed intent").into());
            }
            return Ok(RecordSave {
                current: current.clone(),
                outcome: receipt.outcome.clone(),
                repeated: true,
            });
        }
        let change = apply(scope, previous.as_ref().map(|(_, current)| current))?;
        change.record.validate_portable().map_err(invalid)?;
        if RecordAddress::for_record(&change.record) != request.target {
            return Err(StorageError::new("save cannot change target identity or host").into());
        }
        let unchanged = previous
            .as_ref()
            .is_some_and(|(_, current)| current.record == change.record);
        let revision = if unchanged {
            previous
                .as_ref()
                .expect("unchanged record exists")
                .1
                .revision
                .clone()
        } else {
            Uuid::new_v4().to_string()
        };
        let mut receipts = previous
            .map(|(envelope, _)| envelope.receipts)
            .unwrap_or_default();
        receipts.insert(
            request.operation_id.clone(),
            Receipt {
                fingerprint: request.fingerprint.clone(),
                outcome: change.outcome.clone(),
            },
        );
        let envelope = Envelope {
            version: 2,
            revision: revision.clone(),
            kind: request.target.kind(),
            record: record_to_data(&change.record),
            receipts,
        };
        self.write_envelope(&request.target, &envelope)?;
        Ok(RecordSave {
            current: RecordRead {
                record: change.record,
                revision,
            },
            outcome: change.outcome,
            repeated: false,
        })
    }

    fn write_envelope(
        &self,
        target: &RecordAddress,
        envelope: &Envelope,
    ) -> Result<(), StorageError> {
        let bytes = serde_json::to_vec_pretty(envelope).map_err(invalid)?;
        let relative = relative_record_path(target)?;
        check_parents(&self.root, &relative)?;
        let path = self.root.join(relative);
        let saved = atomic_write(&path, &bytes, &self.root.join(".catalog-staging"));
        self.observe_replacement(path, bytes.len(), &saved);
        saved
    }

    /// Discard the target when its file exists but cannot be decoded: install `replacement`
    /// with a fresh revision and no receipts, or remove the file when there is none.
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
        let Err(unreadable) = decode_envelope(target, &bytes) else {
            return Ok(None);
        };
        match replacement {
            Some(record) => {
                record.validate_portable().map_err(invalid)?;
                if RecordAddress::for_record(&record) != *target {
                    return Err(StorageError::new(
                        "a replacement must keep the target's identity",
                    ));
                }
                self.write_envelope(
                    target,
                    &Envelope {
                        version: 2,
                        revision: Uuid::new_v4().to_string(),
                        kind: target.kind(),
                        record: record_to_data(&record),
                        receipts: BTreeMap::new(),
                    },
                )?;
            }
            None => std::fs::remove_file(self.root.join(relative)).map_err(invalid)?,
        }
        Ok(Some(unreadable.to_string()))
    }
}

#[cfg(test)]
mod tests;
