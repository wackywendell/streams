use super::*;
use std::{
    fs::{self, OpenOptions},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
};

/// Only the holding process is active. An empty file is never evidence of a launch.
pub struct CardLock {
    _lock: crate::file_io::FileLock,
}
impl CardLock {
    pub fn acquire(directory: &Path, scope: &Scope) -> Result<Option<Self>> {
        fs::create_dir_all(directory)?;
        let path = directory.join(format!("{}.lock", conversation_key(scope)?));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)?;
        if !file.metadata()?.is_file() {
            return Err(invalid("card lock must be a regular file"));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(error.into())
            };
        }
        Ok(Some(Self {
            _lock: crate::file_io::FileLock::from_locked(file),
        }))
    }
}
