//! Explicit discovery and transport validation, never called by a named record read.

use super::{
    FileRecordStore, RESET_HINT, RecordIoEvent, decode_record, invalid, relative_record_path,
};
use crate::{
    file_io::ensure_directory,
    model::{Record, check_token},
    storage::{
        StorageError,
        records::{RecordAddress, RecordInventory, RecordListing, UnreadableRecord},
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
    let address = match parts.as_slice() {
        ["hosts", host, "host.json"] => RecordAddress::host_record(*host)?,
        ["hosts", host, "streams", file] => {
            RecordAddress::stream(*host, stream_filename(path, file)?)?
        }
        ["hosts", host, "sessions", harness, id, "session.json"] => {
            RecordAddress::session(*host, *harness, *id)?
        }
        ["hosts", _, "host.yaml"] | ["hosts", _, "conversations", _] => {
            return Err(old_format(path));
        }
        _ => return Err(StorageError::new("unexpected catalog record path")),
    };
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

pub(super) fn token_component(value: &str, name: &str) -> Result<String, StorageError> {
    check_token(value, name).map_err(invalid)?;
    Ok(value.to_owned())
}

/// Validate either portable file kind without making artifacts work-card records.
pub fn catalog_path_host(path: &Path) -> Result<String, StorageError> {
    if let Ok(key) = super::artifacts::artifact_key_from_path(path) {
        Ok(key.host)
    } else {
        Ok(address_from_relative_path(path)?.host().to_owned())
    }
}

fn stream_filename(path: &Path, value: &str) -> Result<String, StorageError> {
    match value.strip_suffix(".json") {
        Some(id) => token_component(id, "stream ID"),
        None if value.ends_with(".yaml") => Err(old_format(path)),
        None => Err(StorageError::new("record filename must end in .json")),
    }
}

/// Records from before format 5 used `conversations/` or YAML-named envelopes.
fn old_format(path: &Path) -> StorageError {
    StorageError::new(format!(
        "record {} is from an older catalog format; {RESET_HINT}",
        path.display()
    ))
}

fn relationships(records: &BTreeMap<RecordAddress, Record>) -> Result<(), StorageError> {
    for record in records.values() {
        if let Record::Session(session) = record {
            let stream = RecordAddress::stream(&session.host_id, &session.stream_id)?;
            if !records.contains_key(&stream) {
                return Err(StorageError::new("Session's same-host Stream is missing"));
            }
        }
    }
    Ok(())
}

/// Decode a complete tracked tree without I/O, including physical identity and live relationships.
pub fn decode_record_tree(files: &BTreeMap<PathBuf, Vec<u8>>) -> Result<Vec<Record>, StorageError> {
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
    for record in records.values() {
        let Record::Session(session) = record else {
            continue;
        };
        for summary in crate::artifacts::summaries(session) {
            let path = super::artifacts::relative_artifact_path(&summary.key)?;
            if !files.contains_key(&path) {
                return Err(StorageError::new("attached artifact content is missing"));
            }
        }
    }
    Ok(records.into_values().collect())
}

impl FileRecordStore {
    /// The caller holds the catalog lock; discovery and sync share one physical tree policy.
    pub(crate) fn read_locked_tree(&self) -> Result<BTreeMap<PathBuf, Vec<u8>>, StorageError> {
        self.read_locked(true)
    }

    /// Reads every discovered file, or only the record files when `artifacts` is false.
    fn read_locked(&self, artifacts: bool) -> Result<BTreeMap<PathBuf, Vec<u8>>, StorageError> {
        let mut paths = Vec::new();
        self.discover(&self.root.join("hosts"), &mut paths)?;
        paths
            .into_iter()
            .filter(|path| artifacts || super::artifacts::artifact_key_from_path(path).is_err())
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
                    ["hosts", host] | ["hosts", host, "streams" | "sessions"] => {
                        token_component(host, "host")?;
                    }
                    ["hosts", _, "conversations"] => return Err(old_format(relative)),
                    ["hosts", host, "sessions", harness] => {
                        token_component(host, "host")?;
                        token_component(harness, "harness")?;
                    }
                    ["hosts", host, "sessions", harness, id]
                    | ["hosts", host, "sessions", harness, id, "artifacts"] => {
                        token_component(host, "host")?;
                        token_component(harness, "harness")?;
                        token_component(id, "Session ID")?;
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

impl FileRecordStore {
    pub(super) fn read_host_sessions(&self, host: &str) -> Result<RecordListing, StorageError> {
        check_token(host, "record host").map_err(invalid)?;
        let relative = PathBuf::from("hosts").join(host).join("sessions");
        super::check_parents(&self.root, &relative.join("entry"))?;
        let sessions_dir = self.root.join(&relative);
        ensure_directory(&sessions_dir, false)?;
        self.observe(RecordIoEvent::List(sessions_dir.clone()));
        let harnesses = match fs::read_dir(&sessions_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RecordListing::default());
            }
            Err(error) => return Err(invalid(error)),
        };
        let mut listing = RecordListing::default();
        for harness_entry in harnesses {
            let harness_entry = harness_entry.map_err(invalid)?;
            if !harness_entry.file_type().map_err(invalid)?.is_dir() {
                return Err(invalid("Session harness entry must be a directory"));
            }
            let harness_name = harness_entry
                .file_name()
                .into_string()
                .map_err(|_| invalid("record path must be UTF-8"))?;
            token_component(&harness_name, "harness")?;
            let harness_dir = harness_entry.path();
            self.observe(RecordIoEvent::List(harness_dir.clone()));
            for session_entry in fs::read_dir(&harness_dir).map_err(invalid)? {
                let session_entry = session_entry.map_err(invalid)?;
                if !session_entry.file_type().map_err(invalid)?.is_dir() {
                    return Err(invalid("Session directory entry must be a directory"));
                }
                let id_name = session_entry
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("record path must be UTF-8"))?;
                token_component(&id_name, "Session ID")?;
                let record_relative = relative
                    .join(&harness_name)
                    .join(&id_name)
                    .join("session.json");
                let address = RecordAddress::session(host, &harness_name, &id_name)?;
                if let Some(bytes) = self.read_named(&record_relative)? {
                    list(&mut listing, decode_record(&address, &bytes));
                }
            }
        }
        Ok(sorted(listing))
    }
}

impl RecordInventory for FileRecordStore {
    fn host_sessions(&self, host: &str) -> Result<RecordListing, StorageError> {
        let _scope = self.scope(false)?;
        self.read_host_sessions(host)
    }

    fn host_streams(&self, host: &str) -> Result<RecordListing, StorageError> {
        let _scope = self.scope(false)?;
        check_token(host, "record host").map_err(invalid)?;
        let relative = PathBuf::from("hosts").join(host).join("streams");
        super::check_parents(&self.root, &relative.join("entry"))?;
        let directory = self.root.join(&relative);
        ensure_directory(&directory, false)?;
        self.observe(RecordIoEvent::List(directory.clone()));
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(RecordListing::default());
            }
            Err(error) => return Err(invalid(error)),
        };
        let mut listing = RecordListing::default();
        for entry in entries {
            let entry = entry.map_err(invalid)?;
            if !entry.file_type().map_err(invalid)?.is_file() {
                return Err(invalid("Stream inventory entry must be a regular record"));
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(&self.root)
                .expect("partition is under catalog");
            let address = address_from_relative_path(relative)?;
            let bytes = self
                .read_named(relative)?
                .ok_or_else(|| invalid("Stream disappeared during inspection"))?;
            list(&mut listing, decode_record(&address, &bytes));
        }
        Ok(sorted(listing))
    }

    fn inventory(&self) -> Result<Vec<Record>, StorageError> {
        let _scope = self.scope(false)?;
        decode_record_tree(&self.read_locked_tree()?)
    }

    fn listing(&self) -> Result<RecordListing, StorageError> {
        let _scope = self.scope(false)?;
        let mut listing = RecordListing::default();
        for (path, bytes) in self.read_locked(false)? {
            let address = address_from_relative_path(&path)?;
            list(&mut listing, decode_record(&address, &bytes));
        }
        Ok(sorted(listing))
    }
}

/// A decode failure names its record in the listing; any other failure fails the listing
/// before it reaches here.
fn list(listing: &mut RecordListing, decoded: Result<Record, UnreadableRecord>) {
    match decoded {
        Ok(record) => listing.records.push(record),
        Err(unreadable) => listing.unreadable.push(unreadable),
    }
}

fn sorted(mut listing: RecordListing) -> RecordListing {
    listing
        .records
        .sort_by_cached_key(RecordAddress::for_record);
    listing
        .unreadable
        .sort_by(|left, right| left.path.cmp(&right.path));
    listing
}

#[cfg(test)]
mod tests;
