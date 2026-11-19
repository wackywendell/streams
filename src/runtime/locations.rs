//! Per-stream or per-session directory intent; reads never register or repair selections.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

use crate::codec::{object, required_nullable};

use super::checkouts::{CheckoutRegistry, CheckoutSnapshot, canonical_directory, relative_path};
use super::{Result, digest, invalid, lock, read_json, text, write_json};

/// A resolved local cwd, optionally anchored to a movable checkout ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryBinding {
    pub cwd: PathBuf,
    pub checkout: Option<BoundCheckout>,
    pub transcript_file: Option<PathBuf>,
}

/// Checkout-relative intent is present as one unit, never as partial metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundCheckout {
    pub id: String,
    pub relative_cwd: String,
    pub repository_url: Option<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    #[serde(deserialize_with = "required_nullable")]
    transcript_file: Option<PathBuf>,
}

/// Identifies either a Stream-wide default binding or a Session-specific binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingKey<'a> {
    Stream(&'a str),
    Session(&'a str),
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
    pub fn path(&self, key: BindingKey<'_>) -> PathBuf {
        match key {
            BindingKey::Session(session_id) => self
                .state_dir
                .join("session-directories")
                .join(format!("{}.json", digest(session_id.as_bytes()))),
            BindingKey::Stream(stream_id) => self.stream_path(stream_id),
        }
    }

    fn stream_path(&self, stream_id: &str) -> PathBuf {
        let key = format!("[{}, null]", crate::codec::ascii_json(stream_id));
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
            (4, Some(stream), None) if selection.transcript_file.is_none() => text(stream)?,
            (4, None, Some(session)) => text(session)?,
            _ => return Err(invalid("unsupported directory binding identity or version")),
        }
        if let Some(path) = &selection.transcript_file {
            absolute(path, "transcript file")?;
        }
        match &selection.location {
            Location::Directory { cwd } => absolute(cwd, "cwd")?,
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

    fn selection(&self, key: BindingKey<'_>) -> Result<Option<Current>> {
        match key {
            BindingKey::Stream(id) | BindingKey::Session(id) => text(id)?,
        }
        let path = self.path(key);
        let Some(selection) = read_json::<Current>(&path)? else {
            return Ok(None);
        };
        Self::validate(&selection)?;
        let matches = match key {
            BindingKey::Stream(stream_id) => {
                selection.stream_id.as_deref() == Some(stream_id) && selection.session_id.is_none()
            }
            BindingKey::Session(session_id) => {
                selection.stream_id.is_none() && selection.session_id.as_deref() == Some(session_id)
            }
        };
        if !matches {
            return Err(invalid(format!(
                "directory binding identity or format mismatch; {}",
                crate::codec::RESET_HINT
            )));
        }
        Ok(Some(selection))
    }

    fn resolve(&self, selection: Current) -> Result<DirectoryBinding> {
        Self::resolve_with(selection, |id, within| self.checkouts.directory(id, within))
    }

    fn resolve_with(
        selection: Current,
        directory: impl FnOnce(&str, &str) -> Result<PathBuf>,
    ) -> Result<DirectoryBinding> {
        let transcript_file = selection.transcript_file;
        match selection.location {
            Location::Directory { cwd } => Ok(DirectoryBinding {
                cwd: canonical_directory(&cwd)?,
                checkout: None,
                transcript_file,
            }),
            Location::Checkout {
                id,
                within,
                repository_url,
            } => Ok(DirectoryBinding {
                cwd: directory(&id, &within)?,
                checkout: Some(BoundCheckout {
                    id,
                    relative_cwd: within,
                    repository_url,
                }),
                transcript_file,
            }),
        }
    }

    /// Resolve current intent, revalidating moved checkout roots and relative cwd containment.
    /// Missing/corrupt targets are errors, never permission to pick another checkout.
    pub fn read(&self, key: BindingKey<'_>) -> Result<Option<DirectoryBinding>> {
        self.selection(key)?
            .map(|selection| self.resolve(selection))
            .transpose()
    }

    /// Resolve display hints using the same operation-local checkout observations as its documents.
    pub fn read_with_checkouts(
        &self,
        key: BindingKey<'_>,
        checkouts: &mut CheckoutSnapshot<'_>,
    ) -> Result<Option<DirectoryBinding>> {
        self.selection(key)?
            .map(|selection| {
                Self::resolve_with(selection, |id, within| checkouts.directory(id, within))
            })
            .transpose()
    }

    /// Read only the saved locator; checking a running Session needs no cwd or registry access.
    pub fn transcript_file(&self, session_id: &str) -> Result<Option<PathBuf>> {
        Ok(self
            .selection(BindingKey::Session(session_id))?
            .and_then(|current| current.transcript_file))
    }

    /// Explicitly select a cwd. `if_missing` preserves and revalidates existing intent.
    /// A normal replacement may repair a missing target, but never corrupt saved intent.
    pub fn remember(
        &self,
        key: BindingKey<'_>,
        cwd: &Path,
        if_missing: bool,
        repository_url: Option<&str>,
        checkout_id: Option<&str>,
    ) -> Result<DirectoryBinding> {
        match key {
            BindingKey::Stream(id) | BindingKey::Session(id) => text(id)?,
        }
        if let Some(url) = repository_url {
            text(url)?;
        }
        let directory = canonical_directory(cwd)?;
        let path = self.path(key);
        let _lock = lock(&path.with_extension("lock"))?;
        let previous = self.selection(key)?;
        if if_missing && let Some(previous) = &previous {
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
        let (stream_id, session_id) = match key {
            BindingKey::Stream(id) => (Some(id.to_owned()), None),
            BindingKey::Session(id) => (None, Some(id.to_owned())),
        };
        let current = Current {
            version: 4,
            stream_id,
            session_id,
            location,
            transcript_file: previous.and_then(|previous| previous.transcript_file),
        };
        // Validate the selection before writing, retaining uncertainty only for the durable write itself.
        let binding = self.resolve(current.clone())?;
        write_json(&path, &current)?;
        Ok(binding)
    }

    /// Save the caller's verified locator without changing directory intent or recording liveness.
    pub fn remember_transcript(&self, session_id: &str, transcript_file: &Path) -> Result<()> {
        if !self.remember_transcript_if_bound(session_id, transcript_file)? {
            return Err(invalid(
                "select the Session directory before saving its transcript locator",
            ));
        }
        Ok(())
    }

    /// Save the caller's verified locator if the Session already has a directory binding.
    pub fn remember_transcript_if_bound(
        &self,
        session_id: &str,
        transcript_file: &Path,
    ) -> Result<bool> {
        text(session_id)?;
        absolute(transcript_file, "transcript file")?;
        let key = BindingKey::Session(session_id);
        let path = self.path(key);
        let _lock = lock(&path.with_extension("lock"))?;
        let Some(mut current) = self.selection(key)? else {
            return Ok(false);
        };
        if current.transcript_file.as_deref() == Some(transcript_file) {
            return Ok(true);
        }
        current.transcript_file = Some(transcript_file.to_owned());
        write_json(&path, &current)?;
        Ok(true)
    }
}

fn absolute(path: &Path, field: &str) -> Result<()> {
    text(
        path.to_str()
            .ok_or_else(|| invalid(format!("{field} must be UTF-8")))?,
    )?;
    if !path.is_absolute() {
        return Err(invalid(format!(
            "directory binding {field} must be absolute"
        )));
    }
    Ok(())
}
