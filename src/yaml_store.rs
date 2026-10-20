//! Versioned YAML storage with process serialization and atomic single-record saves.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

use saneyaml as serde_yaml;
use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::codec::{object_fields, record_from_data, record_to_data};
use crate::model::{Catalog, Record, RecordKey, RecordKind, ValidationError, check_text};
use crate::storage::{Snapshot, StorageError, Store, Transaction};
use crate::sync::state::FileJournal;

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
pub fn relative_record_path(key: &RecordKey) -> PathBuf {
    let name = format!("{}.yaml", encode_component(key.id()));
    match key {
        RecordKey::Session { host_id, .. } => PathBuf::from("sessions")
            .join(encode_component(host_id))
            .join(name),
        RecordKey::Stream { .. } => PathBuf::from("streams").join(name),
        RecordKey::Host { .. } => PathBuf::from("hosts").join(name),
    }
}

/// Flush directory entries; this does not flush the contents of files named there.
pub fn sync_directory(path: &Path) -> Result<(), StorageError> {
    let file = File::open(path)
        .map_err(|e| StorageError::new(format!("cannot open directory {path:?}: {e}")))?;
    file.sync_all()
        .map_err(|e| StorageError::new(format!("cannot fsync directory {path:?}: {e}")))
}

pub(crate) fn atomic_write(path: &Path, bytes: &[u8], staging: &Path) -> Result<(), StorageError> {
    let parent = path.parent().expect("atomic destination has a parent");
    ensure_directory(parent, true)?;
    ensure_directory(staging, true)?;
    let io_error = |e| StorageError::new(format!("cannot replace {}: {e}", path.display()));
    let mut tmp = tempfile::NamedTempFile::new_in(staging).map_err(io_error)?;
    tmp.write_all(bytes).map_err(io_error)?;
    tmp.as_file().sync_all().map_err(io_error)?;
    tmp.persist(path).map_err(|e| io_error(e.error))?;
    let result = sync_directory(parent).and_then(|()| {
        if staging != parent {
            sync_directory(staging)
        } else {
            Ok(())
        }
    });
    result.map_err(StorageError::after_write)
}

pub(crate) fn ensure_directory(path: &Path, create: bool) -> Result<(), StorageError> {
    if path.is_symlink() {
        return Err(StorageError::new(format!(
            "symlinked catalog directory: {}",
            path.display()
        )));
    }
    if create && !path.exists() {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        ensure_directory(parent, true)?;
        match fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure_directory(path, false)?;
            }
            Err(e) => {
                return Err(StorageError::new(format!(
                    "cannot create directory {}: {e}",
                    path.display()
                )));
            }
        }
        // The name must survive in its parent before any durable receipt relies on it.
        sync_directory(path)?;
        sync_directory(parent)?;
    }
    if path.exists() && !path.is_dir() {
        return Err(StorageError::new(format!(
            "expected catalog directory: {}",
            path.display()
        )));
    }
    Ok(())
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

    /// Returns a path relative to this store, not an absolute filesystem path.
    pub fn record_path(&self, key: &RecordKey) -> PathBuf {
        relative_record_path(key)
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
        let envelope = object_fields(&json_val, &["version", "revision", "kind", "record"])
            .map_err(map_err)?;
        match envelope.get("version") {
            Some(Value::Number(n)) if n.as_i64() == Some(1) => {}
            _ => {
                return Err(map_err(ValidationError::new(
                    "unsupported catalog encoding version",
                )));
            }
        }
        let Some(Value::String(rev)) = envelope.get("revision") else {
            return Err(map_err(ValidationError::new(
                "storage revision must be a nonempty string",
            )));
        };
        check_text(rev, "storage revision", false).map_err(map_err)?;
        let Some(Value::String(kind)) = envelope.get("kind") else {
            return Err(map_err(ValidationError::new("unknown record kind")));
        };
        let record_val = envelope.get("record").unwrap_or(&Value::Null);
        let kind = kind
            .parse::<RecordKind>()
            .map_err(|_| map_err(ValidationError::new(format!("unknown record kind: {kind}"))))?;
        let record = record_from_data(kind, record_val).map_err(map_err)?;
        let expected_path = relative_record_path(&record.key())
            .to_string_lossy()
            .replace('\\', "/");
        if path != expected_path {
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
    /// use streams_temper::yaml_store::YamlStore;
    /// let store = YamlStore::new("catalog");
    /// let escaped = store.with_transaction(|tx| Ok(tx));
    /// ```
    pub fn with_transaction<T, F>(&self, f: F) -> Result<T, StorageError>
    where
        F: FnOnce(&mut YamlTransaction) -> Result<T, StorageError>,
    {
        ensure_directory(&self.root, true)?;
        // Never unlink the lock file: another opener must lock this same inode.
        let lock_path = self.root.join(".catalog.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&lock_path)
            .map_err(|e| StorageError::new(format!("cannot open catalog lock: {e}")))?;
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            return Err(StorageError::new(format!("cannot lock catalog: {err}")));
        }
        let mut tx = YamlTransaction {
            root: self.root.clone(),
            _lock: file,
        };
        f(&mut tx)
    }
}

impl Store for YamlStore {
    type Transaction<'a> = YamlTransaction;
    type Journal = FileJournal;

    fn with_transaction<T>(
        &self,
        f: impl FnOnce(&mut Self::Transaction<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        YamlStore::with_transaction(self, f)
    }

    fn journal(&self) -> Self::Journal {
        FileJournal {
            root: self.root.join(".catalog-sync"),
        }
    }

    fn check_local(&self) -> Result<(), StorageError> {
        let allowed: BTreeSet<&str> = [
            ".catalog.lock",
            ".catalog-sync",
            ".catalog-staging",
            "streams",
            "sessions",
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
}

impl Transaction for YamlTransaction {
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
    _lock: std::fs::File,
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
        let mut out = Vec::new();
        for name in ["streams", "sessions", "hosts"] {
            let dir = self.root.join(name);
            ensure_directory(&dir, false)?;
            if !dir.exists() {
                continue;
            }
            let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
                .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?
                .into_iter()
                .map(|e| e.path())
                .collect();
            entries.sort();
            for path in entries {
                if name == "sessions" {
                    ensure_directory(&path, false)?;
                    let mut sub: Vec<PathBuf> = fs::read_dir(&path)
                        .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|e| StorageError::new(format!("cannot read catalog: {e}")))?
                        .into_iter()
                        .map(|e| e.path())
                        .collect();
                    sub.sort();
                    for item in sub {
                        out.push(Self::check_record_file(&item)?);
                    }
                } else {
                    out.push(Self::check_record_file(&path)?);
                }
            }
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
        let mut envelope = serde_json::Map::new();
        envelope.insert("version".into(), Value::Number(1.into()));
        envelope.insert("revision".into(), Value::String(Uuid::new_v4().to_string()));
        envelope.insert("kind".into(), Value::String(key.kind().to_string()));
        envelope.insert("record".into(), record_to_data(record));
        // JSON is a YAML subset. Quoting every string preserves YAML 1.1 readers
        // (including PyYAML) where bare yes/on/timestamp-looking values change type.
        let yaml_text = serde_json::to_string_pretty(&Value::Object(envelope))
            .map_err(|e| StorageError::new(format!("cannot save catalog record: {e}")))?;
        let path = self.root.join(relative_record_path(&key));
        atomic_write(
            &path,
            yaml_text.as_bytes(),
            &self.root.join(".catalog-staging"),
        )?;
        // A failed read after replacement cannot establish that the save failed.
        self.read().map_err(StorageError::after_write)
    }
}
