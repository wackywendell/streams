//! Raw outputs share the catalog lock and host partition with their Session metadata.

use super::{FileRecordStore, Reader, RecordIoEvent, check_parents, invalid};
use crate::{
    artifacts::{
        ArtifactKey, ArtifactReader, ArtifactSave, ArtifactStore, ContentChange, validate_content,
    },
    file_io::{atomic_write, ensure_directory, sync_directory},
    file_store::encode_component,
    model::{Record, RecordKind},
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordReader},
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) fn directory(host: &str, stream: &str) -> PathBuf {
    PathBuf::from("hosts")
        .join(encode_component(host))
        .join("streams")
        .join(encode_component(stream))
        .join("artifacts")
}

pub fn relative_artifact_path(key: &ArtifactKey) -> Result<PathBuf, StorageError> {
    key.validate().map_err(invalid)?;
    Ok(directory(&key.host, &key.stream)
        .join(encode_component(&key.session))
        .join(&key.name))
}

pub fn artifact_key_from_path(path: &Path) -> Result<ArtifactKey, StorageError> {
    let parts = super::inventory::components(path)?;
    let ["hosts", host, "streams", stream, "artifacts", session, name] = parts.as_slice() else {
        return Err(invalid("unexpected artifact path"));
    };
    let key = ArtifactKey {
        host: super::inventory::component(host)?,
        stream: super::inventory::component(stream)?,
        session: super::inventory::component(session)?,
        name: name.to_string(),
    };
    if relative_artifact_path(&key)?.as_os_str() != path.as_os_str() {
        return Err(invalid("non-canonical artifact path"));
    }
    Ok(key)
}

pub fn decode_artifact_content(bytes: &[u8]) -> Result<String, StorageError> {
    let content = std::str::from_utf8(bytes).map_err(invalid)?;
    validate_content(content).map_err(invalid)?;
    Ok(content.into())
}

impl ArtifactReader for Reader<'_> {
    fn read_content(&self, key: &ArtifactKey) -> Result<Option<String>, StorageError> {
        self.store
            .read_named(&relative_artifact_path(key)?)?
            .map(|bytes| decode_artifact_content(&bytes))
            .transpose()
    }

    fn stream_producers(&self, host: &str, stream: &str) -> Result<Vec<String>, StorageError> {
        crate::model::check_text(host, "artifact host", false).map_err(invalid)?;
        crate::model::check_text(stream, "artifact Stream", false).map_err(invalid)?;
        let relative = directory(host, stream);
        check_parents(&self.store.root, &relative.join("entry"))?;
        let path = self.store.root.join(relative);
        ensure_directory(&path, false)?;
        self.store.observe(RecordIoEvent::List(path.clone()));
        let entries = match fs::read_dir(path) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(invalid(e)),
        };
        let mut sessions = Vec::new();
        for entry in entries {
            let entry = entry.map_err(invalid)?;
            if !entry.file_type().map_err(invalid)?.is_dir() {
                return Err(invalid("artifact producer must be a directory"));
            }
            let name = entry.file_name();
            sessions.push(super::inventory::component(
                name.to_str()
                    .ok_or_else(|| invalid("artifact producer must be UTF-8"))?,
            )?);
        }
        sessions.sort();
        Ok(sessions)
    }
}

impl ArtifactStore for FileRecordStore {
    fn with_artifacts<T, E: From<StorageError>>(
        &self,
        read: impl FnOnce(&dyn ArtifactReader) -> Result<T, E>,
    ) -> Result<T, E> {
        read(&self.scope(false)?)
    }

    fn save_artifact<T, E: From<StorageError> + std::fmt::Display>(
        &self,
        target: &RecordAddress,
        content: ContentChange,
        apply: impl FnOnce(
            &dyn RecordReader,
            Option<&Record>,
        ) -> Result<(RecordChange<T>, ArtifactKey), E>,
    ) -> Result<ArtifactSave<T>, E> {
        if target.host() != self.host || target.kind() != RecordKind::Session {
            return Err(invalid("artifact write must target a local Session").into());
        }
        if let ContentChange::Write(text) = &content {
            validate_content(text).map_err(invalid)?;
        }
        let scope = self.scope(true)?;
        let mut path = None;
        let mut content_saved = false;
        let saved = self.save_scoped(&scope, target, |reader, before| {
            let (change, key) = apply(reader, before)?;
            if *target != RecordAddress::new(&key.host, RecordKind::Session, &key.session)? {
                return Err(invalid("artifact key must match its producing Session").into());
            }
            let relative = relative_artifact_path(&key)?;
            check_parents(&self.root, &relative)?;
            let content_path = self.root.join(relative);
            path = Some(content_path.clone());
            let path = &content_path;
            if let ContentChange::Write(text) = &content {
                // A raw output is never read to obtain its name or description.
                if path.is_symlink() || (path.exists() && !path.is_file()) {
                    return Err(invalid("artifact content must be a regular file").into());
                }
                let result =
                    atomic_write(path, text.as_bytes(), &self.root.join(".catalog-staging"));
                self.observe_replacement(path.clone(), text.len(), &result);
                result?;
                content_saved = true;
            }
            Ok(change)
        });
        let save = saved.map_err(|error: E| {
            if content_saved {
                invalid(format!(
                    "artifact content was saved, but its Session metadata save failed: {}",
                    error
                ))
                .into()
            } else {
                error
            }
        })?;
        let mut content_written = content_saved;
        let mut warnings = Vec::new();
        // Deleting again after the reference is gone still removes leftover content.
        if matches!(content, ContentChange::Delete) {
            let path = path.expect("a save derives its content path");
            let deleted = match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    fs::remove_file(&path)
                        .map_err(invalid)
                        .and_then(|()| {
                            sync_directory(path.parent().expect("artifact parent"))
                                .map_err(StorageError::after_write)
                        })
                        .map(|()| true)
                }
                Ok(_) => Err(invalid("artifact content is not a regular file")),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(invalid(e)),
            };
            match deleted {
                Ok(removed) => content_written |= removed,
                Err(error) => warnings.push(format!(
                    "Session reference removed; unreferenced artifact content may remain: {error}"
                )),
            }
        }
        Ok(ArtifactSave {
            save,
            content_written,
            warnings,
        })
    }
}
