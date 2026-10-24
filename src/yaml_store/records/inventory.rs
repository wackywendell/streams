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

fn components(path: &Path) -> Result<Vec<&str>, StorageError> {
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
fn component(value: &str) -> Result<String, StorageError> {
    let decoded = percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .map_err(invalid)?
        .into_owned();
    if super::super::encode_component(&decoded) != value {
        return Err(StorageError::new("non-canonical record identity encoding"));
    }
    Ok(decoded)
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
        let address = address_from_relative_path(path)?;
        records.insert(address.clone(), decode_record(&address, bytes)?);
    }
    relationships(&records)?;
    Ok(records.into_values().collect())
}

impl YamlRecordStore {
    /// The caller holds the catalog lock; discovery and sync share one physical tree policy.
    pub(crate) fn read_locked_tree(&self) -> Result<BTreeMap<PathBuf, Vec<u8>>, StorageError> {
        let mut addresses = Vec::new();
        self.discover(&self.root.join("hosts"), &mut addresses)?;
        addresses
            .into_iter()
            .map(|address| {
                let path = relative_record_path(&address)?;
                let bytes = self.read_named(&path)?.ok_or_else(|| {
                    StorageError::new("record disappeared during locked discovery")
                })?;
                Ok((path, bytes))
            })
            .collect()
    }

    fn discover(
        &self,
        directory: &Path,
        addresses: &mut Vec<RecordAddress>,
    ) -> Result<(), StorageError> {
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
                    _ => return Err(StorageError::new("unexpected catalog directory")),
                }
                self.discover(&path, addresses)?;
            } else {
                // Reader rejects symlinks, FIFOs and other special files before opening them.
                addresses.push(address_from_relative_path(relative)?);
            }
        }
        Ok(())
    }
}

impl RecordInventory for YamlRecordStore {
    fn inventory(&self) -> Result<Vec<RecordRead>, StorageError> {
        let _scope = self.scope(false)?;
        decode_record_tree(&self.read_locked_tree()?)
    }
}

#[cfg(test)]
mod tests;
