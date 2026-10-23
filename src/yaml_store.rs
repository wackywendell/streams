//! Versioned YAML storage with process serialization and atomic single-record saves.

pub mod records;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use saneyaml as serde_yaml;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::codec::{record_from_data, record_to_data};
use crate::file_io::{atomic_write, ensure_directory, lock};
use crate::model::{Catalog, Record, RecordKind, ValidationError, check_text};
use crate::storage::{Journal, Snapshot, StorageError, Store, Transaction};

/// Encode UTF-8 bytes for a single filename component, including traversal-only dot names.
pub fn encode_component(id: &str) -> String {
    const SAFE: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-_.~";
    let mut encoded = String::new();
    for &byte in id.as_bytes() {
        if SAFE.contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    if encoded == "." || encoded == ".." {
        encoded.replace('.', "%2E")
    } else {
        encoded
    }
}

/// Map an owned identity to its canonical catalog-relative location.
pub fn relative_record_path(record: &Record) -> PathBuf {
    let name = format!("{}.yaml", encode_component(record.key().id()));
    match record {
        Record::Session(session) => PathBuf::from("conversations")
            .join(encode_component(&session.host_id))
            .join(name),
        Record::Stream(_) => PathBuf::from("streams").join(name),
        Record::Host(_) => PathBuf::from("hosts").join(name),
    }
}

/// Canonical identity-based path followed by the legacy Stream-partitioned path.
/// Legacy files remain readable and are relocated atomically on a membership move.
pub(crate) fn record_paths(record: &Record) -> Vec<PathBuf> {
    let mut paths = vec![relative_record_path(record)];
    if let Record::Session(session) = record {
        paths.push(
            PathBuf::from("sessions")
                .join(encode_component(&session.stream_id))
                .join(encode_component(&session.host_id))
                .join(format!("{}.yaml", encode_component(&session.id))),
        );
    }
    paths
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordEnvelope {
    version: u64,
    revision: String,
    kind: RecordKind,
    record: Value,
}

fn yaml_to_json(yaml: serde_yaml::Value) -> Result<Value, ValidationError> {
    match yaml {
        serde_yaml::Value::Null => Ok(Value::Null),
        serde_yaml::Value::Bool(b) => Ok(Value::Bool(b)),
        serde_yaml::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Number(i.into()))
            } else if let Some(u) = n.as_u64() {
                Ok(Value::Number(u.into()))
            } else if let Some(f) = n.as_f64() {
                serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .ok_or_else(|| ValidationError::new("non-finite YAML number"))
            } else {
                Err(ValidationError::new("invalid YAML number"))
            }
        }
        serde_yaml::Value::String(s) => Ok(Value::String(s)),
        serde_yaml::Value::Sequence(seq) => {
            let mut out = Vec::with_capacity(seq.len());
            for item in seq {
                out.push(yaml_to_json(item)?);
            }
            Ok(Value::Array(out))
        }
        serde_yaml::Value::Mapping(map) => {
            let mut obj = serde_json::Map::new();
            let mut seen = BTreeSet::new();
            for (k, v) in map {
                let Value::String(key_str) = yaml_to_json(k)? else {
                    return Err(ValidationError::new(
                        "YAML object keys must be unique strings",
                    ));
                };
                if !seen.insert(key_str.clone()) {
                    return Err(ValidationError::new(
                        "YAML object keys must be unique strings",
                    ));
                }
                obj.insert(key_str, yaml_to_json(v)?);
            }
            Ok(Value::Object(obj))
        }
        serde_yaml::Value::Tagged(tagged) => {
            let core = if tagged.tag.handle == "!!" {
                Some(tagged.tag.suffix.as_str())
            } else if tagged.tag.handle == "!" {
                tagged
                    .tag
                    .suffix
                    .strip_prefix("tag:yaml.org,2002:")
                    .or_else(|| tagged.tag.suffix.is_empty().then_some("str"))
            } else {
                None
            };
            match core {
                Some("str" | "bool" | "int" | "float" | "null" | "seq" | "map") => {
                    yaml_to_json(tagged.value)
                }
                _ => Err(ValidationError::new("unsupported YAML tag")),
            }
        }
    }
}

/// Versioned record files; constructing a handle does not create or inspect the root.
#[derive(Debug, Clone)]
pub struct YamlStore {
    pub root: PathBuf,
}

impl YamlStore {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        let abs = if root.is_absolute() {
            root.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(root)
        };
        Self { root: abs }
    }

    /// Returns the existing physical location, or the canonical path for a new record.
    /// The caller's snapshot validates files before any storage effects.
    pub fn record_path(&self, record: &Record) -> PathBuf {
        record_paths(record)
            .into_iter()
            .find(|path| self.root.join(path).exists())
            .unwrap_or_else(|| relative_record_path(record))
    }

    /// Check schema, identity and canonical relative location before accepting file bytes.
    /// Legacy ambiguous YAML scalars may require manual quoting during migration.
    pub fn decode_record(path: &str, contents: &[u8]) -> Result<Record, StorageError> {
        let map_err = |e: ValidationError| StorageError::new(format!("cannot read catalog: {e}"));
        let text_str = std::str::from_utf8(contents)
            .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?;
        let raw_yaml: serde_yaml::Value = serde_yaml::LoadOptions::yaml_1_1()
            .from_str(text_str)
            .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?;
        let json_val = yaml_to_json(raw_yaml).map_err(map_err)?;
        if !json_val.is_object() {
            return Err(map_err(ValidationError::new(
                "expected an object with string keys",
            )));
        }
        let envelope: RecordEnvelope = serde_json::from_value(json_val)
            .map_err(|e| map_err(ValidationError::new(e.to_string())))?;
        if envelope.version != 1 {
            return Err(map_err(ValidationError::new(
                "unsupported catalog encoding version",
            )));
        }
        check_text(&envelope.revision, "storage revision", false).map_err(map_err)?;
        if !envelope.record.is_object() {
            return Err(map_err(ValidationError::new(
                "record must be an object with string keys",
            )));
        }
        let record = record_from_data(envelope.kind, &envelope.record).map_err(map_err)?;
        if !record_paths(&record)
            .iter()
            .any(|candidate| path == candidate.to_string_lossy().replace('\\', "/"))
        {
            return Err(map_err(ValidationError::new(format!(
                "record identity does not match its physical location: {path}"
            ))));
        }
        Ok(record)
    }

    /// Decode files as one catalog and retain their exact byte digests for stale-save checks.
    pub fn decode_snapshot(files: &BTreeMap<String, Vec<u8>>) -> Result<Snapshot, StorageError> {
        let mut streams = Vec::new();
        let mut sessions = Vec::new();
        let mut hosts = Vec::new();
        let mut revisions = BTreeMap::new();
        for (name, contents) in files {
            let record = Self::decode_record(name, contents)?;
            let mut hasher = Sha256::new();
            hasher.update(contents);
            let digest = format!("{:x}", hasher.finalize());
            revisions.insert(record.key(), digest);
            match record {
                Record::Stream(s) => streams.push(s),
                Record::Session(s) => sessions.push(s),
                Record::Host(h) => hosts.push(h),
            }
        }
        let catalog = Catalog::new(streams, sessions, hosts)
            .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?;
        Ok(Snapshot { catalog, revisions })
    }

    /// Hold the exclusive catalog lock for the entire closure, releasing it even on unwind.
    ///
    /// The transaction is borrowed, so it cannot be retained after the lock scope:
    /// ```compile_fail
    /// use streams_temper::{storage::StorageError, yaml_store::YamlStore};
    /// let store = YamlStore::new("catalog");
    /// let escaped = store.with_transaction::<_, StorageError>(|tx| Ok(tx));
    /// ```
    ///
    /// Its journal is borrowed from the same transaction and cannot escape either:
    /// ```compile_fail
    /// use streams_temper::{storage::{StorageError, Transaction}, yaml_store::YamlStore};
    /// let store = YamlStore::new("catalog");
    /// let escaped = store.with_transaction::<_, StorageError>(|tx| Ok(tx.journal()));
    /// ```
    ///
    /// Journal operations and catalog operations share the held lock. This example
    /// checks the accepted borrowing scope; storage conformance tests run its assertions.
    /// ```no_run
    /// use streams_temper::{storage::{StorageError, Transaction}, yaml_store::YamlStore};
    /// # fn main() -> Result<(), StorageError> {
    /// let directory = tempfile::tempdir().unwrap();
    /// let store = YamlStore::new(directory.path());
    /// store.with_transaction::<_, StorageError>(|tx| {
    ///     let journal = tx.journal();
    ///     let snapshot = tx.read()?;
    ///     assert!(snapshot.catalog.records().is_empty());
    ///     assert!(journal.names()?.is_empty());
    ///     Ok(())
    /// })?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn with_transaction<T, E: From<StorageError>>(
        &self,
        f: impl FnOnce(&mut YamlTransaction) -> Result<T, E>,
    ) -> Result<T, E> {
        self.with_access(false, f)
    }

    /// Shares the existing catalog lock and forbids every persistence entry point.
    pub fn with_inspection<T, E: From<StorageError>>(
        &self,
        f: impl FnOnce(&mut YamlTransaction) -> Result<T, E>,
    ) -> Result<T, E> {
        self.with_access(true, f)
    }

    fn with_access<T, E: From<StorageError>>(
        &self,
        read_only: bool,
        f: impl FnOnce(&mut YamlTransaction) -> Result<T, E>,
    ) -> Result<T, E> {
        ensure_directory(&self.root, !read_only)?;
        let path = self.root.join(".catalog.lock");
        let file = if read_only {
            crate::file_io::inspection_lock(&path)?
        } else {
            lock(&path)?
        };
        let mut tx = YamlTransaction {
            root: self.root.clone(),
            journal: FileJournal {
                root: self.root.join(".catalog-sync"),
                read_only,
            },
            _lock: file,
        };
        f(&mut tx)
    }
}

impl Store for YamlStore {
    type Transaction<'a> = YamlTransaction;

    fn with_transaction<T, E: From<StorageError>>(
        &self,
        f: impl FnOnce(&mut Self::Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        YamlStore::with_transaction(self, f)
    }

    fn with_inspection<T, E: From<StorageError>>(
        &self,
        f: impl FnOnce(&mut Self::Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        YamlStore::with_inspection(self, f)
    }
}

impl Transaction for YamlTransaction {
    fn journal(&self) -> &dyn Journal {
        &self.journal
    }

    fn check_local(&self) -> Result<(), StorageError> {
        let allowed: BTreeSet<&str> = [
            ".catalog.lock",
            ".catalog-sync",
            ".catalog-staging",
            "streams",
            "sessions",
            "conversations",
            "hosts",
        ]
        .into();
        for entry in fs::read_dir(&self.root)
            .map_err(|e| StorageError::new(format!("cannot read catalog root: {e}")))?
        {
            let entry = entry.map_err(|e| StorageError::new(e.to_string()))?;
            if !allowed.contains(entry.file_name().to_string_lossy().as_ref()) {
                return Err(StorageError::new(
                    "unexpected contents in local catalog; inspect before continuing",
                ));
            }
        }
        Ok(())
    }

    fn read(&self) -> Result<Snapshot, StorageError> {
        YamlTransaction::read(self)
    }

    fn save(&self, record: &Record, expected: &Snapshot) -> Result<Snapshot, StorageError> {
        YamlTransaction::save(self, record, expected)
    }
}

/// A catalog lock held by ownership; its borrowed handle cannot escape the transaction closure.
pub struct YamlTransaction {
    root: PathBuf,
    journal: FileJournal,
    _lock: std::fs::File,
}

/// Object persistence owned exclusively by a locked YAML transaction.
struct FileJournal {
    root: PathBuf,
    read_only: bool,
}

impl FileJournal {
    fn writable(&self) -> Result<(), StorageError> {
        if self.read_only {
            return Err(StorageError::new("read-only catalog inspection"));
        }
        Ok(())
    }

    fn ensure_directory(&self, create: bool) -> Result<(), StorageError> {
        if self.root.is_symlink() {
            return Err(StorageError::new("symlinked transport state"));
        }
        crate::file_io::ensure_directory(&self.root, create)
    }
}

impl Journal for FileJournal {
    fn read(&self, name: &str) -> Result<Option<Map<String, Value>>, StorageError> {
        self.ensure_directory(!self.read_only)?;
        crate::file_io::read_json(&self.root.join(name))
    }

    fn write(&self, name: &str, data: Map<String, Value>) -> Result<(), StorageError> {
        self.writable()?;
        self.ensure_directory(true)?;
        crate::file_io::write_json(&self.root.join(name), &data)
    }

    fn clear(&self, name: &str) -> Result<(), StorageError> {
        self.writable()?;
        self.ensure_directory(true)?;
        let path = self.root.join(name);
        let removed = path.exists();
        if removed {
            fs::remove_file(&path)
                .map_err(|e| StorageError::new(format!("cannot clear {name}: {e}")))?;
        }
        crate::file_io::sync_directory(&self.root)
            .map_err(|e| if removed { e.after_write() } else { e })
    }

    fn names(&self) -> Result<Vec<String>, StorageError> {
        self.ensure_directory(!self.read_only)?;
        if !self.root.exists() {
            return Ok(Vec::new());
        }
        fs::read_dir(&self.root)
            .map_err(|e| StorageError::new(format!("cannot read {}: {e}", self.root.display())))?
            .map(|entry| {
                entry
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .map_err(|e| StorageError::new(e.to_string()))
            })
            .collect()
    }

    fn describe(&self, name: &str) -> String {
        self.root.join(name).display().to_string()
    }
}

impl YamlTransaction {
    fn check_record_file(path: &Path) -> Result<PathBuf, StorageError> {
        if path.is_symlink()
            || !path.is_file()
            || path.extension().and_then(|e| e.to_str()) != Some("yaml")
        {
            return Err(StorageError::new(format!(
                "unexpected catalog file: {}",
                path.display()
            )));
        }
        Ok(path.to_path_buf())
    }

    fn collect_files(&self) -> Result<Vec<PathBuf>, StorageError> {
        fn collect(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> Result<(), StorageError> {
            ensure_directory(dir, false)?;
            if !dir.exists() {
                return Ok(());
            }
            let mut entries = fs::read_dir(dir)
                .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?;
            entries.sort();
            for path in entries {
                if depth == 0 {
                    out.push(YamlTransaction::check_record_file(&path)?);
                } else {
                    collect(&path, depth - 1, out)?;
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        for (name, depth) in [
            ("streams", 0),
            ("sessions", 2),
            ("conversations", 1),
            ("hosts", 0),
        ] {
            collect(&self.root.join(name), depth, &mut out)?;
        }
        Ok(out)
    }

    /// Read a complete snapshot while this transaction owns the catalog lock.
    pub fn read(&self) -> Result<Snapshot, StorageError> {
        let mut files = BTreeMap::new();
        for path in self.collect_files()? {
            let rel = path
                .strip_prefix(&self.root)
                .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path)
                .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?;
            files.insert(rel, bytes);
        }
        YamlStore::decode_snapshot(&files)
    }

    /// Replace one record only if the entire captured revision set is still current.
    /// Failures after replacement are uncertain, even if a receipt has not been written.
    pub fn save(&self, record: &Record, expected: &Snapshot) -> Result<Snapshot, StorageError> {
        self.journal.writable()?;
        let current = self.read()?;
        if current.revisions != expected.revisions {
            return Err(StorageError::revision_conflict(
                "catalog changed since the captured snapshot",
            ));
        }
        current
            .catalog
            .replacing(record.clone())
            .map_err(|e| StorageError::new(format!("cannot save catalog record: {e}")))?;
        let key = record.key();
        if current.catalog.get(&key).as_ref() == Some(record) {
            return Ok(current);
        }
        let envelope = RecordEnvelope {
            version: 1,
            revision: Uuid::new_v4().to_string(),
            kind: key.kind(),
            record: record_to_data(record),
        };
        // JSON is a YAML subset. Quoting every string preserves YAML 1.1 readers
        // (including PyYAML) where bare yes/on/timestamp-looking values change type.
        let envelope = serde_json::to_value(envelope).expect("record envelope is JSON-compatible");
        let yaml_text = serde_json::to_string_pretty(&envelope)
            .map_err(|e| StorageError::new(format!("cannot save catalog record: {e}")))?;
        let store = YamlStore::new(&self.root);
        let path = self.root.join(store.record_path(record));
        let mut relocated = false;
        if let Some(before) = expected.catalog.get(&key) {
            let previous = self.root.join(store.record_path(&before));
            if previous != path {
                // Move the original bytes before replacement so a crash never leaves
                // competing copies of the same stable Session identity.
                ensure_directory(path.parent().expect("record parent"), true)?;
                fs::rename(&previous, &path).map_err(|e| {
                    StorageError::new(format!("cannot relocate session record: {e}"))
                })?;
                relocated = true;
                crate::file_io::sync_directory(path.parent().expect("record parent"))
                    .and_then(|()| {
                        crate::file_io::sync_directory(previous.parent().expect("record parent"))
                    })
                    .map_err(StorageError::after_write)?;
            }
        }
        atomic_write(
            &path,
            yaml_text.as_bytes(),
            &self.root.join(".catalog-staging"),
        )
        .map_err(|error| {
            if relocated {
                error.after_write()
            } else {
                error
            }
        })?;
        // A failed read after replacement cannot establish that the save failed.
        self.read().map_err(StorageError::after_write)
    }
}
