use std::{
    fs::{self, OpenOptions},
    os::fd::AsRawFd,
    os::unix::fs::MetadataExt,
    panic::catch_unwind,
};

use streams_temper::{
    storage::{StorageError, StorageErrorKind},
    yaml_store::YamlStore,
};

#[test]
fn storage_failures_have_one_noncontradictory_kind() {
    let definite = StorageError::new("failed before write");
    assert_eq!(definite.kind(), StorageErrorKind::Definite);
    assert!(!definite.is_uncertain());
    assert!(!definite.is_revision_conflict());
    let uncertain = StorageError::uncertain("response lost");
    assert_eq!(uncertain.kind(), StorageErrorKind::Uncertain);
    assert!(uncertain.is_uncertain());
    assert!(!uncertain.is_revision_conflict());
    let stale = StorageError::revision_conflict("stale snapshot");
    assert_eq!(stale.kind(), StorageErrorKind::RevisionConflict);
    assert!(stale.is_revision_conflict());
    assert!(!stale.is_uncertain());
}

#[test]
fn catalog_lock_rejects_directory_and_fifo_without_running_transaction() {
    for directory in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".catalog.lock");
        if directory {
            fs::create_dir(&path).unwrap();
        } else {
            let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        }
        let error = YamlStore::new(dir.path())
            .with_transaction::<(), StorageError>(|_| panic!("invalid lock admitted transaction"))
            .unwrap_err();
        assert!(!error.is_uncertain());
        assert!(!dir.path().join(".catalog-sync").exists());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn transaction_owns_lock_and_releases_it_on_unwind() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    assert!(
        catch_unwind(|| {
            store
                .with_transaction::<(), StorageError>(|_| panic!("interrupted transaction"))
                .unwrap();
        })
        .is_err()
    );
    let lock_path = dir.path().join(".catalog.lock");
    let inode = fs::metadata(&lock_path).unwrap().ino();
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    drop(file);
    store.with_transaction(|tx| tx.read()).unwrap();
    assert_eq!(fs::metadata(&lock_path).unwrap().ino(), inode);
}
