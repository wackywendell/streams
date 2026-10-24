//! Legacy directory selection translation is an offline boundary, never a read fallback.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub(crate) struct PreparedDirectoryCutover {
    state_dir: PathBuf,
    sources: BTreeMap<PathBuf, Vec<u8>>,
    writes: BTreeMap<PathBuf, Current>,
    _locks: Vec<crate::file_io::FileLock>,
}
impl DirectoryBindings {
    pub(crate) fn prepare_cutover(
        &self,
        sessions: &BTreeMap<String, String>,
        streams: &BTreeSet<String>,
    ) -> Result<PreparedDirectoryCutover> {
        let mut sources = BTreeMap::new();
        let mut writes: BTreeMap<PathBuf, Current> = BTreeMap::new();
        crate::file_io::ensure_directory(&self.state_dir, false)?;
        for collection in ["directories", "session-directories"] {
            let directory = self.state_dir.join(collection);
            crate::file_io::ensure_directory(&directory, false)?;
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let path = entry?.path();
                if path.extension().is_none_or(|extension| extension != "json") {
                    continue;
                }
                let bytes = crate::file_io::read_bytes(&path)?.ok_or_else(|| {
                    invalid("directory selection disappeared during offline cutover")
                })?;
                let mut current: Current = crate::file_io::decode_json(&bytes)?;
                Self::validate(&current)?;
                let expected = match collection {
                    "directories" if current.version == 2 => self.stream_path(
                        current.stream_id.as_deref().expect("v2 validated"),
                        current.session_id.as_deref(),
                    ),
                    "session-directories" if current.session_id.is_some() => {
                        self.path("", current.session_id.as_deref())
                    }
                    _ => return Err(invalid("legacy directory binding layout mismatch")),
                };
                if path != expected {
                    return Err(invalid("legacy directory binding filename mismatch"));
                }
                sources.insert(path, bytes);
                let target = if let Some(old) = &current.session_id {
                    let Some(id) = sessions.get(old) else {
                        continue;
                    };
                    current.version = 3;
                    current.stream_id = None;
                    current.session_id = Some(id.clone());
                    self.path("", Some(id))
                } else {
                    let stream = current
                        .stream_id
                        .as_deref()
                        .expect("Stream default validated");
                    if !streams.contains(stream) {
                        continue;
                    }
                    self.path(stream, None)
                };
                if let Some(existing) = writes.get(&target) {
                    if existing.location != current.location {
                        return Err(invalid(
                            "conflicting directory selections; explicitly clear and reselect before cutover",
                        ));
                    }
                } else {
                    writes.insert(target, current);
                }
            }
        }
        let locks: BTreeSet<_> = sources
            .keys()
            .chain(writes.keys())
            .map(|path| path.with_extension("lock"))
            .collect();
        let guards = locks
            .iter()
            .map(|path| lock(path))
            .collect::<Result<Vec<_>>>()?;
        for (path, before) in &sources {
            if crate::file_io::read_bytes(path)?.as_deref() != Some(before.as_slice()) {
                return Err(invalid(
                    "directory selection changed while acquiring cutover locks; stop all writers before cutover",
                ));
            }
        }
        Ok(PreparedDirectoryCutover {
            state_dir: self.state_dir.clone(),
            sources,
            writes,
            _locks: guards,
        })
    }
}
impl PreparedDirectoryCutover {
    pub(crate) fn install(&self, archive: &Path) -> Result<()> {
        for (path, bytes) in &self.sources {
            let relative = path
                .strip_prefix(&self.state_dir)
                .expect("selection is under runtime root");
            let target = archive.join("runtime").join(relative);
            crate::file_io::atomic_write(
                &target,
                bytes,
                target.parent().expect("archived selection parent"),
            )?;
        }
        for (path, selection) in &self.writes {
            write_json(path, selection)?;
        }
        for path in self
            .sources
            .keys()
            .filter(|path| !self.writes.contains_key(*path))
        {
            fs::remove_file(path)?;
            crate::file_io::sync_directory(path.parent().expect("selection parent"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
