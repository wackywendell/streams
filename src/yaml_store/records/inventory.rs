//! Explicit discovery and transport validation, never called by a named record read.

use super::{RecordIoEvent, YamlRecordStore, decode_record, invalid, relative_record_path};
use crate::{
    file_io::ensure_directory,
    model::{Record, RecordKind},
    storage::{
        StorageError,
        records::{RecordAddress, RecordInventory, RecordRead},
    },
};
use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

/// Reject non-canonical spellings instead of creating aliases for physical record identities.
pub fn address_from_relative_path(path: &Path) -> Result<RecordAddress, StorageError> {
    let parts = components(path)?;
    let (host, kind, id) = match parts.as_slice() {
        ["hosts", host, "host.yaml"] => {
            let host = component(host)?;
            (host.clone(), RecordKind::Host, host)
        }
        ["hosts", host, "streams", file] => (component(host)?, RecordKind::Stream, filename(file)?),
        ["hosts", host, "conversations", file] => {
            (component(host)?, RecordKind::Session, filename(file)?)
        }
        _ => return Err(StorageError::new("unexpected catalog record path")),
    };
    let address = RecordAddress::new(host, kind, id)?;
    if relative_record_path(&address)?.as_os_str() != path.as_os_str() {
        return Err(StorageError::new("non-canonical catalog record path"));
    }
    Ok(address)
}

pub(super) fn components(path: &Path) -> Result<Vec<&str>, StorageError> {
    path.components()
        .map(|part| match part {
            Component::Normal(value) => value
                .to_str()
                .ok_or_else(|| StorageError::new("record path must be UTF-8")),
            _ => Err(StorageError::new(
                "record path must be relative and canonical",
            )),
        })
        .collect()
}
pub(super) fn component(value: &str) -> Result<String, StorageError> {
    let decoded = percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map_err(invalid)?
        .into_owned();
    if super::super::encode_component(&decoded) != value {
        return Err(StorageError::new("non-canonical record identity encoding"));
    }
    Ok(decoded)
}
/// Validate either portable file kind without making artifacts work-card records.
pub fn catalog_path_host(path: &Path) -> Result<String, StorageError> {
    if let Ok(key) = super::artifacts::artifact_key_from_path(path) {
        Ok(key.host)
    } else {
        Ok(address_from_relative_path(path)?.host().to_owned())
    }
}

fn filename(value: &str) -> Result<String, StorageError> {
    component(
        value
            .strip_suffix(".yaml")
            .ok_or_else(|| StorageError::new("record filename must end in .yaml"))?,
    )
}

fn relationships(records: &BTreeMap<RecordAddress, RecordRead>) -> Result<(), StorageError> {
    for read in records.values() {
        if let Record::Session(session) = &read.record {
            let stream =
                RecordAddress::new(&session.host_id, RecordKind::Stream, &session.stream_id)?;
            if !records.contains_key(&stream) {
                return Err(StorageError::new("Session's same-host Stream is missing"));
            }
        }
    }
    Ok(())
}

/// Decode a complete tracked tree without I/O, including physical identity and live relationships.
pub fn decode_record_tree(
    files: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<Vec<RecordRead>, StorageError> {
    let mut records = BTreeMap::new();
    for (path, bytes) in files {
        if super::artifacts::artifact_key_from_path(path).is_ok() {
            // Interrupted saves may leave valid but unattached output text.
            super::artifacts::decode_artifact_content(bytes)?;
        } else {
            let address = address_from_relative_path(path)?;
            records.insert(address.clone(), decode_record(&address, bytes)?);
        }
    }
    relationships(&records)?;
    for read in records.values() {
        let Record::Session(session) = &read.record else {
            continue;
        };
        for stream in session.artifacts.keys() {
            let stream_key = RecordAddress::new(&session.host_id, RecordKind::Stream, stream)?;
            if !records.contains_key(&stream_key) {
                return Err(StorageError::new("artifact's producing Stream is missing"));
            }
            for summary in crate::artifacts::summaries(session, stream) {
                let path = super::artifacts::relative_artifact_path(&summary.key)?;
                if !files.contains_key(&path) {
                    return Err(StorageError::new("attached artifact content is missing"));
                }
            }
        }
    }
    Ok(records.into_values().collect())
}

impl YamlRecordStore {
    /// The caller holds the catalog lock; discovery and sync share one physical tree policy.
    pub(crate) fn read_locked_tree(&self) -> Result<BTreeMap<PathBuf, Vec<u8>>, StorageError> {
        let mut paths = Vec::new();
        self.discover(&self.root.join("hosts"), &mut paths)?;
        paths
            .into_iter()
            .map(|path| {
                let bytes = self.read_named(&path)?.ok_or_else(|| {
                    StorageError::new("record disappeared during locked discovery")
                })?;
                Ok((path, bytes))
            })
            .collect()
    }

    fn discover(&self, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<(), StorageError> {
        ensure_directory(directory, false)?;
        self.observe(RecordIoEvent::List(directory.into()));
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(invalid(error)),
        };
        for entry in entries {
            let entry = entry.map_err(invalid)?;
            let path = entry.path();
            let relative = path
                .strip_prefix(&self.root)
                .expect("entry is under catalog root");
            if entry.file_type().map_err(invalid)?.is_dir() {
                let parts = components(relative)?;
                match parts.as_slice() {
                    ["hosts", host] => {
                        component(host)?;
                    }
                    ["hosts", host, "streams" | "conversations"] => {
                        component(host)?;
                    }
                    ["hosts", host, "streams", stream]
                    | ["hosts", host, "streams", stream, "artifacts"] => {
                        component(host)?;
                        component(stream)?;
                    }
                    ["hosts", host, "streams", stream, "artifacts", session] => {
                        component(host)?;
                        component(stream)?;
                        component(session)?;
                    }
                    _ => return Err(StorageError::new("unexpected catalog directory")),
                }
                self.discover(&path, paths)?;
            } else {
                // Reader rejects symlinks, FIFOs and other special files before opening them.
                catalog_path_host(relative)?;
                paths.push(relative.to_path_buf());
            }
        }
        Ok(())
    }
}

impl RecordInventory for YamlRecordStore {
    fn host_sessions(&self, host: &str) -> Result<Vec<RecordRead>, StorageError> {
        self.host_partition(host, RecordKind::Session, "Session")
    }

    fn host_streams(&self, host: &str) -> Result<Vec<RecordRead>, StorageError> {
        self.host_partition(host, RecordKind::Stream, "Stream")
    }

    fn inventory(&self) -> Result<Vec<RecordRead>, StorageError> {
        let _scope = self.scope(false)?;
        decode_record_tree(&self.read_locked_tree()?)
    }
}

impl YamlRecordStore {
    fn host_partition(
        &self,
        host: &str,
        kind: RecordKind,
        label: &str,
    ) -> Result<Vec<RecordRead>, StorageError> {
        let _scope = self.scope(false)?;
        let sample = RecordAddress::new(host, kind, "entry")?;
        let relative = relative_record_path(&sample)?;
        super::check_parents(&self.root, &relative)?;
        let directory = self
            .root
            .join(relative.parent().expect("partition directory"));
        ensure_directory(&directory, false)?;
        self.observe(RecordIoEvent::List(directory.clone()));
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(error) => return Err(invalid(error)),
        };
        let mut records = Vec::new();
        for entry in entries {
            let entry = entry.map_err(invalid)?;
            let file_type = entry.file_type().map_err(invalid)?;
            if kind == RecordKind::Stream && file_type.is_dir() {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("record path must be UTF-8"))?;
                component(&name)?;
                continue;
            }
            if !file_type.is_file() {
                return Err(invalid(format!(
                    "{label} inventory entry must be a regular record"
                )));
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(&self.root)
                .expect("partition is under catalog");
            let address = address_from_relative_path(relative)?;
            let bytes = self
                .read_named(relative)?
                .ok_or_else(|| invalid(format!("{label} disappeared during inspection")))?;
            records.push(decode_record(&address, &bytes)?);
        }
        records.sort_by(|left, right| left.record.key().id().cmp(right.record.key().id()));
        Ok(records)
    }
}

#[cfg(test)]
mod tests;
