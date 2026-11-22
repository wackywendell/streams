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
    atomic_write_with_sync(path, bytes, staging, sync_directory)
}

fn atomic_write_with_sync(
    path: &Path,
    bytes: &[u8],
    staging: &Path,
    sync: impl Fn(&Path) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let parent = path.parent().expect("atomic destination has a parent");
    ensure_directory(parent, true)?;
    ensure_directory(staging, true)?;
    let io_error = |e| StorageError::new(format!("cannot replace {}: {e}", path.display()));
    let mut tmp = tempfile::NamedTempFile::new_in(staging).map_err(io_error)?;
    tmp.write_all(bytes).map_err(io_error)?;
    tmp.as_file().sync_all().map_err(io_error)?;
    tmp.persist(path).map_err(|e| io_error(e.error))?;
    let result = sync(parent).and_then(|()| {
        if staging != parent {
            sync(staging)
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

/// Scope ownership must release flock even if a concurrent fork inherited the descriptor.
pub(crate) struct FileLock {
    file: File,
    owner: u32,
}
impl FileLock {
    pub(crate) fn from_locked(file: File) -> Self {
        Self {
            file,
            owner: std::process::id(),
        }
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        // An inherited copy is not the owning process's scope and must not unlock it.
        if std::process::id() == self.owner {
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

/// Open without following symlinks or waiting on special files, then serialize writers.
pub(crate) fn lock(path: &Path) -> Result<FileLock, StorageError> {
    open_lock(path, false, true, false)
}

pub(crate) fn inspection_lock(path: &Path) -> Result<FileLock, StorageError> {
    open_lock(path, true, false, false)
}

pub(crate) fn existing_write_lock(path: &Path) -> Result<FileLock, StorageError> {
    open_lock(path, false, false, false)
}

/// Non-blocking exclusive lock; returns `Ok(None)` if another process holds it.
pub(crate) fn try_lock(path: &Path) -> Result<Option<FileLock>, StorageError> {
    match open_lock_raw(path, false, true, true) {
        Ok(lock) => Ok(Some(lock)),
        Err(LockError::WouldBlock) => Ok(None),
        Err(LockError::Storage(error)) => Err(error),
    }
}

enum LockError {
    WouldBlock,
    Storage(StorageError),
}

fn open_lock(
    path: &Path,
    read_only: bool,
    create: bool,
    nonblocking: bool,
) -> Result<FileLock, StorageError> {
    match open_lock_raw(path, read_only, create, nonblocking) {
        Ok(lock) => Ok(lock),
        Err(LockError::WouldBlock) => Err(StorageError::new(format!(
            "cannot lock {}: would block",
            path.display()
        ))),
        Err(LockError::Storage(error)) => Err(error),
    }
}

fn open_lock_raw(
    path: &Path,
    read_only: bool,
    create: bool,
    nonblocking: bool,
) -> Result<FileLock, LockError> {
    ensure_directory(path.parent().expect("lock parent"), create).map_err(LockError::Storage)?;
    let file = OpenOptions::new()
        .create(create)
        .truncate(false)
        .read(true)
        .write(!read_only)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| {
            LockError::Storage(StorageError::new(format!(
                "cannot open lock {}: {e}",
                path.display()
            )))
        })?;
    if !file
        .metadata()
        .map_err(|e| LockError::Storage(StorageError::new(e.to_string())))?
        .is_file()
    {
        return Err(LockError::Storage(StorageError::new(format!(
            "lock must be a regular file: {}",
            path.display()
        ))));
    }
    // Never unlink the lock: other writers must keep locking this same inode.
    let operation = if read_only {
        libc::LOCK_SH
    } else {
        libc::LOCK_EX
    } | if nonblocking { libc::LOCK_NB } else { 0 };
    if unsafe { libc::flock(file.as_raw_fd(), operation) } != 0 {
        let error = std::io::Error::last_os_error();
        if nonblocking && error.kind() == std::io::ErrorKind::WouldBlock {
            return Err(LockError::WouldBlock);
        }
        return Err(LockError::Storage(StorageError::new(format!(
            "cannot lock {}: {error}",
            path.display()
        ))));
    }
    Ok(FileLock::from_locked(file))
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
    read_bytes(path)?
        .map(|bytes| decode_json(&bytes))
        .transpose()
}

pub(crate) fn read_bytes(path: &Path) -> Result<Option<Vec<u8>>, StorageError> {
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
    Ok(Some(bytes))
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

pub(crate) fn write_text(path: &Path, text: &str) -> Result<(), StorageError> {
    if path.is_symlink() {
        return Err(StorageError::new(format!(
            "refusing symlinked state file: {}",
            path.display()
        )));
    }
    atomic_write(path, text.as_bytes(), path.parent().expect("file parent"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn post_replacement_flush_failures_are_uncertain_and_keep_complete_bytes() {
        for fail_on in ["records", "staging"] {
            let root = tempfile::tempdir().unwrap();
            let parent = root.path().join("records");
            let staging = root.path().join("staging");
            let path = parent.join("target.json");
            atomic_write(&path, b"old bytes", &staging).unwrap();
            let bytes = br#"{"record":"new","receipts":{"accepted":true}}"#;
            let error = atomic_write_with_sync(&path, bytes, &staging, |directory| {
                if directory.file_name().unwrap() == fail_on {
                    Err(StorageError::new("injected directory flush failure"))
                } else {
                    sync_directory(directory)
                }
            })
            .unwrap_err();
            assert!(error.is_uncertain());
            assert_eq!(fs::read(path).unwrap(), bytes);
            assert_eq!(fs::read_dir(staging).unwrap().count(), 0);
        }
    }

    #[test]
    fn scope_unwind_releases_lock_even_when_an_inherited_descriptor_stays_open() {
        for shared in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("catalog.lock");
            drop(lock(&path).unwrap());
            let held = if shared {
                inspection_lock(&path)
            } else {
                existing_write_lock(&path)
            }
            .unwrap();
            // dup and fork retain the same locked open-file description. No child or timing needed.
            let inherited = held.file.try_clone().unwrap();
            drop(FileLock {
                file: held.file.try_clone().unwrap(),
                owner: held.owner.wrapping_add(1),
            });
            let contender = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)
                .unwrap();
            assert_ne!(
                unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            assert_eq!(
                std::io::Error::last_os_error().kind(),
                std::io::ErrorKind::WouldBlock
            );
            assert!(
                std::panic::catch_unwind(|| {
                    let _scope = held;
                    panic!("interrupted lock scope");
                })
                .is_err()
            );
            assert_eq!(
                unsafe { libc::flock(contender.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
                0
            );
            drop(inherited);
        }
    }

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
