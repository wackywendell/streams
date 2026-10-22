//! Per-stream or per-session directory intent; reads never register or repair selections.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::codec::{object, required_nullable};

use super::checkouts::{CheckoutRegistry, canonical_directory, relative_path};
use super::{Result, digest, invalid, lock, read_json, text, write_json};

/// A resolved local cwd, optionally anchored to a movable checkout ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryBinding {
    pub cwd: PathBuf,
    pub checkout: Option<BoundCheckout>,
}

/// Checkout-relative intent is present as one unit, never as partial metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundCheckout {
    pub id: String,
    pub relative_cwd: String,
    pub repository_url: Option<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Location {
    Directory {
        cwd: PathBuf,
    },
    Checkout {
        id: String,
        within: String,
        #[serde(deserialize_with = "required_nullable")]
        repository_url: Option<String>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Current {
    version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stream_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    session_id: Option<String>,
    #[serde(deserialize_with = "object")]
    location: Location,
}

/// Saved cwd intent scoped by Session identity, or by Stream for Stream-only defaults.
#[derive(Debug, Clone)]
pub struct DirectoryBindings {
    state_dir: PathBuf,
    checkouts: CheckoutRegistry,
}

impl DirectoryBindings {
    /// Construct a local binding store; no state is read or created.
    pub fn new(state_dir: impl AsRef<Path>, checkouts: CheckoutRegistry) -> Self {
        Self {
            state_dir: state_dir.as_ref().to_path_buf(),
            checkouts,
        }
    }

    /// Return the current hashed binding path, without creating a directory.
    pub fn path(&self, stream_id: &str, session_id: Option<&str>) -> PathBuf {
        if let Some(session_id) = session_id {
            return self
                .state_dir
                .join("session-directories")
                .join(format!("{}.json", digest(session_id.as_bytes())));
        }
        self.legacy_path(stream_id, None)
    }

    fn legacy_path(&self, stream_id: &str, session_id: Option<&str>) -> PathBuf {
        let key = format!(
            "[{}, {}]",
            crate::codec::ascii_json(stream_id),
            session_id
                .map(crate::codec::ascii_json)
                .unwrap_or_else(|| "null".into())
        );
        self.state_dir
            .join("directories")
            .join(format!("{}.json", digest(key.as_bytes())))
    }

    fn validate(selection: &Current) -> Result<()> {
        match (
            selection.version,
            &selection.stream_id,
            &selection.session_id,
        ) {
            (2, Some(stream), session) => {
                text(stream)?;
                if let Some(session) = session {
                    text(session)?;
                }
            }
            (3, None, Some(session)) => text(session)?,
            _ => return Err(invalid("unsupported directory binding identity or version")),
        }
        match &selection.location {
            Location::Directory { cwd } => absolute(cwd)?,
            Location::Checkout {
                id,
                within,
                repository_url,
            } => {
                text(id)?;
                relative_path(within, true)?;
                if let Some(url) = repository_url {
                    text(url)?;
                }
            }
        }
        Ok(())
    }

    fn selection(
        &self,
        stream_id: &str,
        session_id: Option<&str>,
    ) -> Result<Option<(Current, PathBuf)>> {
        text(stream_id)?;
        if let Some(id) = session_id {
            text(id)?;
        }
        let path = self.path(stream_id, session_id);
        let read_current = || -> Result<_> {
            let Some(selection) = read_json::<Current>(&path)? else {
                return Ok(None);
            };
            Self::validate(&selection)?;
            // A renamed v2 Session selection is also readable after interrupted promotion.
            if selection.session_id.as_deref() != session_id
                || (session_id.is_none() && selection.stream_id.as_deref() != Some(stream_id))
            {
                return Err(invalid("directory binding identity mismatch"));
            }
            Ok(Some((selection, path.clone())))
        };
        if let Some(selection) = read_current()? {
            return Ok(Some(selection));
        }
        let Some(session_id) = session_id else {
            return Ok(None);
        };
        let directory = self.state_dir.join("directories");
        crate::file_io::ensure_directory(&directory, false)?;
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return read_current(),
            Err(error) => return Err(error.into()),
        };
        let exact = self.legacy_path(stream_id, Some(session_id));
        let mut found = None;
        for entry in entries {
            let path = entry?.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some(hash) = name.strip_suffix(".json") else {
                continue;
            };
            if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            // An unreadable legacy record could belong to this Session before its move.
            // Do not mistake unknown identity for proof that no selection exists.
            let Some(selection) = read_json::<Current>(&path)? else {
                continue;
            };
            if selection.session_id.as_deref() != Some(session_id) {
                if path == exact {
                    return Err(invalid("directory binding identity mismatch"));
                }
                continue;
            }
            Self::validate(&selection)?;
            if selection.version != 2
                || selection
                    .stream_id
                    .as_deref()
                    .is_none_or(|stream| self.legacy_path(stream, Some(session_id)) != path)
            {
                return Err(invalid("legacy directory binding identity mismatch"));
            }
            if found.is_some() {
                return Err(invalid("ambiguous legacy directory selections for Session"));
            }
            found = Some((selection, path));
        }
        // Promotion can move the selection between the first probe and legacy discovery.
        match found {
            Some(selection) => Ok(Some(selection)),
            None => read_current(),
        }
    }

    fn resolve(&self, selection: Current) -> Result<DirectoryBinding> {
        match selection.location {
            Location::Directory { cwd } => Ok(DirectoryBinding {
                cwd: canonical_directory(&cwd)?,
                checkout: None,
            }),
            Location::Checkout {
                id,
                within,
                repository_url,
            } => Ok(DirectoryBinding {
                cwd: self.checkouts.directory(&id, &within)?,
                checkout: Some(BoundCheckout {
                    id,
                    relative_cwd: within,
                    repository_url,
                }),
            }),
        }
    }

    /// Resolve current intent, revalidating moved checkout roots and relative cwd containment.
    /// Missing/corrupt targets are errors, never permission to pick another checkout.
    pub fn read(
        &self,
        stream_id: &str,
        session_id: Option<&str>,
    ) -> Result<Option<DirectoryBinding>> {
        self.selection(stream_id, session_id)?
            .map(|(selection, _)| self.resolve(selection))
            .transpose()
    }

    /// Explicitly select a cwd. `if_missing` preserves and revalidates existing intent.
    /// A normal replacement may repair a missing target, but never corrupt saved intent.
    pub fn remember(
        &self,
        stream_id: &str,
        cwd: &Path,
        session_id: Option<&str>,
        if_missing: bool,
        repository_url: Option<&str>,
        checkout_id: Option<&str>,
    ) -> Result<DirectoryBinding> {
        text(stream_id)?;
        if let Some(id) = session_id {
            text(id)?;
        }
        if let Some(url) = repository_url {
            text(url)?;
        }
        let directory = canonical_directory(cwd)?;
        let path = self.path(stream_id, session_id);
        let _lock = lock(&path.with_extension("lock"))?;
        let previous = self.selection(stream_id, session_id)?;
        if if_missing && let Some((previous, _)) = &previous {
            return self.resolve(previous.clone());
        }
        let found = self.checkouts.inspect(&directory)?;
        if let Some(found) = &found {
            relative_path(&found.relative_cwd, true)?;
        }
        let selected = if let Some(id) = checkout_id {
            let selected = self.checkouts.get(id)?;
            if found
                .as_ref()
                .is_none_or(|found| found.root != selected.directory)
            {
                return Err(invalid("selected checkout does not contain requested cwd"));
            }
            Some(selected)
        } else if let Some(found) = &found {
            Some(self.checkouts.register(&found.root, repository_url)?)
        } else {
            None
        };
        let location = if let Some(selected) = selected {
            if let Some(url) = repository_url
                && !selected.repository_urls.iter().any(|bound| bound == url)
            {
                return Err(invalid(
                    "selected checkout lacks explicit repository binding",
                ));
            }
            Location::Checkout {
                id: selected.id,
                within: found.expect("selected checkout was inspected").relative_cwd,
                repository_url: repository_url.map(str::to_owned),
            }
        } else {
            if repository_url.is_some() {
                return Err(invalid("repository binding requires a Git or jj checkout"));
            }
            Location::Directory { cwd: directory }
        };
        let current = Current {
            version: if session_id.is_some() { 3 } else { 2 },
            stream_id: session_id.is_none().then(|| stream_id.to_owned()),
            session_id: session_id.map(str::to_owned),
            location,
        };
        // Validate the selection before writing, retaining uncertainty only for the durable write itself.
        let binding = self.resolve(current.clone())?;
        if let Some((_, legacy)) = previous
            && legacy != path
        {
            // Preserve the original bytes at one readable path even if replacement is interrupted.
            fs::rename(&legacy, &path)?;
            crate::file_io::sync_directory(path.parent().expect("selection parent"))
                .and_then(|()| {
                    crate::file_io::sync_directory(legacy.parent().expect("legacy parent"))
                })
                .map_err(crate::storage::StorageError::after_write)?;
        }
        write_json(&path, &current)?;
        Ok(binding)
    }
}

fn absolute(path: &Path) -> Result<()> {
    text(path.to_str().ok_or_else(|| invalid("cwd must be UTF-8"))?)?;
    if !path.is_absolute() {
        return Err(invalid("directory binding cwd must be absolute"));
    }
    Ok(())
}
