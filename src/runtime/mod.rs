//! Host-local checkout selections and verified saved-conversation identities.
//!
//! These files are execution-host state, never shared catalog records. Callers must
//! not publish their paths or use them to infer conversation membership.

pub mod checkouts;
pub mod conversations;
pub mod locations;

pub use checkouts::{CheckoutLocation, CheckoutRegistry, RepositoryInspection};
pub use locations::{BoundCheckout, DirectoryBinding, DirectoryBindings};

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::codec::load_json;
use crate::storage::StorageError;
use crate::yaml_store::{atomic_write, ensure_directory};

/// Local state failures distinguish absence/ambiguity from corruption and uncertain writes.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("invalid host-local state: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("{message}; candidates: {candidates:?}")]
    Ambiguous {
        message: String,
        candidates: Vec<String>,
    },
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Storage(#[from] StorageError),
    #[error("repository inspection failed: {0}")]
    Inspection(String),
}

impl RuntimeError {
    /// A failure after atomic replacement requires inspection before retrying.
    pub fn is_uncertain(&self) -> bool {
        matches!(self, Self::Storage(error) if error.is_uncertain())
    }
}

pub(crate) type Result<T> = std::result::Result<T, RuntimeError>;

pub(crate) fn invalid(message: impl Into<String>) -> RuntimeError {
    RuntimeError::Invalid(message.into())
}

pub(crate) fn text(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(|c| c < ' ' || c == '\u{7f}') {
        return Err(invalid("expected nonempty text without control characters"));
    }
    Ok(())
}

pub(crate) fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    let json = std::str::from_utf8(bytes).map_err(|error| invalid(error.to_string()))?;
    let value = load_json(json).map_err(|error| invalid(error.to_string()))?;
    if !value.is_object() {
        return Err(invalid("expected a JSON object"));
    }
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}

pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(invalid("state must be a regular file"));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    decode(&bytes).map(Some)
}

pub(crate) fn lock(path: &Path) -> Result<File> {
    ensure_directory(
        path.parent()
            .ok_or_else(|| invalid("lock needs a parent"))?,
        true,
    )?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(invalid("lock must be a regular file"));
    }
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(file)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    if path.is_symlink() {
        return Err(invalid("symlinked state file"));
    }
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| invalid(e.to_string()))?;
    bytes.push(b'\n');
    atomic_write(
        path,
        &bytes,
        path.parent()
            .ok_or_else(|| invalid("state needs a parent"))?,
    )?;
    Ok(())
}

pub(crate) fn digest(value: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value))
}
