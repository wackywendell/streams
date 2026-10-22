use super::*;
use crate::{
    storage::{Journal, StorageError, Transaction},
    yaml_store::YamlStore,
};
use serde_json::json;
use std::sync::{Arc, Mutex};

fn staged(root: &Path) -> TempDir {
    let staging = root.join(".catalog-staging");
    fs::create_dir_all(&staging).unwrap();
    let candidate = tempfile::Builder::new()
        .prefix("adoption-")
        .tempdir_in(staging)
        .unwrap();
    fs::create_dir(candidate.path().join(".git")).unwrap();
    // This phase moves opaque bytes; it does not inspect Git objects.
    fs::write(
        candidate.path().join(".git/prepared"),
        b"prepared machinery",
    )
    .unwrap();
    candidate
}

fn evidence() -> Map<String, Value> {
    json!({"adoption":{"baseline":"opaque"}})
        .as_object()
        .unwrap()
        .clone()
}

fn assert_prepared(root: &Path) {
    assert_eq!(
        fs::read(root.join(".git/prepared")).unwrap(),
        b"prepared machinery"
    );
}

fn assert_fenced(root: &Path) {
    YamlStore::new(root)
        .with_transaction::<_, StorageError>(|tx| {
            let state = State::from_journal(tx.journal());
            assert_eq!(
                state.read("save.json").unwrap().unwrap()["adoption"],
                evidence()["adoption"]
            );
            assert!(state.check_save().is_err());
            Ok(())
        })
        .unwrap();
}

struct LostWriteAck<'a>(&'a dyn Journal);

impl Journal for LostWriteAck<'_> {
    fn read(&self, name: &str) -> Result<Option<Map<String, Value>>, StorageError> {
        self.0.read(name)
    }
    fn write(&self, name: &str, object: Map<String, Value>) -> Result<(), StorageError> {
        self.0.write(name, object)?;
        Err(StorageError::uncertain("fence write acknowledgement lost"))
    }
    fn clear(&self, name: &str) -> Result<(), StorageError> {
        self.0.clear(name)
    }
    fn names(&self) -> Result<Vec<String>, StorageError> {
        self.0.names()
    }
    fn describe(&self, name: &str) -> String {
        self.0.describe(name)
    }
}

#[test]
fn physical_fence_write_error_after_effect_retains_both_fence_and_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let candidate = staged(root);
    let retained = candidate.path().to_path_buf();
    YamlStore::new(root)
        .with_transaction::<_, StorageError>(|tx| {
            let journal = LostWriteAck(tx.journal());
            let state = State::from_journal(&journal);
            assert_eq!(
                fence_and_move(&state, root, candidate, evidence())
                    .unwrap_err()
                    .to_string(),
                "fence write acknowledgement lost",
            );
            Ok(())
        })
        .unwrap();
    assert_prepared(&retained);
    assert!(!root.join(".git").exists());
    assert_fenced(root);
}

#[test]
fn failed_fence_write_retains_candidate_without_installing_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let candidate = staged(root);
    let retained = candidate.path().to_path_buf();
    let store = YamlStore::new(root);
    store
        .with_transaction::<_, StorageError>(|tx| {
            let state = State::from_journal(tx.journal()).with_hook(Arc::new(Mutex::new(
                |op: &str, name: &str| {
                    if op == "write" && name == "save.json" {
                        Err(repair("fence failure"))
                    } else {
                        Ok(())
                    }
                },
            )));
            assert_eq!(
                fence_and_move(&state, root, candidate, evidence())
                    .unwrap_err()
                    .to_string(),
                "fence failure",
            );
            assert!(state.read("save.json").unwrap().is_none());
            assert!(state.check_save().is_ok());
            Ok(())
        })
        .unwrap();
    assert_prepared(&retained);
    assert!(!root.join(".git").exists());
}

#[test]
fn failed_move_preserves_both_fence_and_uninstalled_candidate() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let candidate = staged(root);
    let retained = candidate.path().to_path_buf();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/block-install"), b"retain").unwrap();
    let store = YamlStore::new(root);
    store
        .with_transaction::<_, StorageError>(|tx| {
            let state = State::from_journal(tx.journal());
            assert!(fence_and_move(&state, root, candidate, evidence()).is_err());
            Ok(())
        })
        .unwrap();
    assert_prepared(&retained);
    assert_eq!(
        fs::read(root.join(".git/block-install")).unwrap(),
        b"retain"
    );
    let saved = fs::read(root.join(".catalog-sync/save.json")).unwrap();
    assert_fenced(root);
    assert_eq!(
        fs::read(root.join(".catalog-sync/save.json")).unwrap(),
        saved
    );
}

#[test]
fn successful_move_leaves_fence_and_retained_directory_for_completion() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let candidate = staged(root);
    let original = candidate.path().to_path_buf();
    let store = YamlStore::new(root);
    let retained = store
        .with_transaction::<_, StorageError>(|tx| {
            Ok(fence_and_move(
                &State::from_journal(tx.journal()),
                root,
                candidate,
                evidence(),
            )
            .unwrap())
        })
        .unwrap();
    assert_eq!(retained, original);
    assert!(retained.is_dir());
    assert!(!retained.join(".git").exists());
    assert_prepared(root);
    assert_fenced(root);
}
