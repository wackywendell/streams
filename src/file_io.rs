//! Filesystem durability and strict host-local JSON primitives.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;

use serde::{Serialize, de::DeserializeOwned};

use crate::codec::load_json;
use crate::storage::StorageError;

/// Flush directory entries; this does not flush the contents of files named there.
pub(crate) fn sync_directory(path: &Path) -> Result<(), StorageError> {
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

/// Open without following symlinks or waiting on special files, then serialize writers.
pub(crate) fn lock(path: &Path) -> Result<File, StorageError> {
    open_lock(path, false)
}

pub(crate) fn inspection_lock(path: &Path) -> Result<File, StorageError> {
    open_lock(path, true)
}

fn open_lock(path: &Path, read_only: bool) -> Result<File, StorageError> {
    ensure_directory(path.parent().expect("lock parent"), !read_only)?;
    let file = OpenOptions::new()
        .create(!read_only)
        .truncate(false)
        .read(true)
        .write(!read_only)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| StorageError::new(format!("cannot open lock {}: {e}", path.display())))?;
    if !file
        .metadata()
        .map_err(|e| StorageError::new(e.to_string()))?
        .is_file()
    {
        return Err(StorageError::new(format!(
            "lock must be a regular file: {}",
            path.display()
        )));
    }
    // Never unlink the lock: other writers must keep locking this same inode.
    let operation = if read_only {
        libc::LOCK_SH
    } else {
        libc::LOCK_EX
    };
    if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
        return Err(StorageError::new(format!(
            "cannot lock {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(file)
}

pub(crate) fn decode_json<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, StorageError> {
    let text = std::str::from_utf8(bytes).map_err(|e| StorageError::new(e.to_string()))?;
    let value = load_json(text).map_err(|e| StorageError::new(e.to_string()))?;
    if !value.is_object() {
        return Err(StorageError::new("expected a JSON object"));
    }
    serde_json::from_value(value).map_err(|e| StorageError::new(e.to_string()))
}

/// Missing files stay missing; even the parent directory is checked without creation.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StorageError> {
    ensure_directory(path.parent().expect("file parent"), false)?;
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => {
            return Ok(None);
        }
        Err(e) => {
            return Err(StorageError::new(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    if !file
        .metadata()
        .map_err(|e| StorageError::new(e.to_string()))?
        .is_file()
    {
        return Err(StorageError::new(format!(
            "state must be a regular file: {}",
            path.display()
        )));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|e| StorageError::new(format!("cannot read {}: {e}", path.display())))?;
    decode_json(&bytes).map(Some)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), StorageError> {
    if path.is_symlink() {
        return Err(StorageError::new(format!(
            "refusing symlinked state file: {}",
            path.display()
        )));
    }
    let value = serde_json::to_value(value).map_err(|e| StorageError::new(e.to_string()))?;
    if !value.is_object() {
        return Err(StorageError::new("expected a JSON object"));
    }
    let mut bytes =
        serde_json::to_vec_pretty(&value).map_err(|e| StorageError::new(e.to_string()))?;
    bytes.push(b'\n');
    atomic_write(path, &bytes, path.parent().expect("file parent"))
}

pub(crate) fn check_catalog_root(root: &Path) -> Result<(), StorageError> {
    ensure_directory(root, false)?;
    for name in [".catalog-sync", ".catalog.lock", ".catalog-staging"] {
        if root.join(name).is_symlink() {
            return Err(StorageError::new("symlinked catalog machinery"));
        }
    }
    ensure_directory(&root.join(".catalog-sync"), false)
}

/// Only top-level policy is shared: callers separately validate bookkeeping contents.
pub(crate) fn check_empty_catalog_root(root: &Path) -> Result<(), StorageError> {
    for entry in fs::read_dir(root).map_err(|e| StorageError::new(e.to_string()))? {
        let name = entry
            .map_err(|e| StorageError::new(e.to_string()))?
            .file_name();
        if name != ".catalog.lock" && name != ".catalog-sync" {
            return Err(StorageError::new(
                "initialization requires a new empty catalog directory",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_lock_excludes_independent_opens_in_other_threads() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.lock");
        let held = lock(&path).unwrap();
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let contender = OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open(&path)
                        .unwrap();
                    assert_ne!(
                        unsafe {
                            libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB)
                        },
                        0
                    );
                    assert_eq!(
                        std::io::Error::last_os_error().kind(),
                        std::io::ErrorKind::WouldBlock
                    );
                })
                .join()
                .unwrap();
            drop(held);
            scope.spawn(|| drop(lock(&path).unwrap())).join().unwrap();
        });
    }
}
