//! Per-stream or per-session directory intent; reads never register or repair selections.

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
    stream_id: String,
    #[serde(deserialize_with = "required_nullable")]
    session_id: Option<String>,
    #[serde(deserialize_with = "object")]
    location: Location,
}

/// Saved cwd intent scoped by the exact `(stream_id, session_id)` pair.
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

    fn selection(&self, stream_id: &str, session_id: Option<&str>) -> Result<Option<Current>> {
        text(stream_id)?;
        if let Some(id) = session_id {
            text(id)?;
        }
        let Some(selection) = read_json::<Current>(&self.path(stream_id, session_id))? else {
            return Ok(None);
        };
        if selection.version != 2 {
            return Err(invalid("unsupported directory binding version"));
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
        if selection.stream_id != stream_id || selection.session_id.as_deref() != session_id {
            return Err(invalid("directory binding identity mismatch"));
        }
        Ok(Some(selection))
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
            .map(|selection| self.resolve(selection))
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
        if if_missing && let Some(previous) = previous {
            return self.resolve(previous);
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
            version: 2,
            stream_id: stream_id.into(),
            session_id: session_id.map(str::to_owned),
            location,
        };
        // Validate the selection before writing, retaining uncertainty only for the durable write itself.
        let binding = self.resolve(current.clone())?;
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
