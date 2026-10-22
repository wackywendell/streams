//! Filesystem evidence for recovery fences, independent of Git and publication setup.
use super::*;
use crate::storage::Transaction;
use crate::yaml_store::YamlStore;
use std::sync::atomic::{AtomicUsize, Ordering};

fn adoption() -> AdoptingAttempt {
    let old = "a".repeat(40).parse().unwrap();
    let new = "b".repeat(40).parse().unwrap();
    let adoption = PublicationAttempt::new(
        None,
        new,
        old,
        "main",
        "origin",
        vec!["save".into()],
        Some(Seconds::try_from(2000.0).unwrap()),
    )
    .unwrap();
    adoption.adopting(adoption.local_tip().clone(), adoption.candidate().clone())
}

#[test]
fn corrupt_adoption_marker_is_rejected_without_overwriting_evidence() {
    let dir = tempfile::tempdir().unwrap();
    YamlStore::new(dir.path())
        .with_transaction::<_, StorageError>(|tx| {
            let path = dir.path().join(".catalog-sync/publication.json");
            let bytes = b"{not valid JSON";
            State::from_journal(tx.journal())
                .fence_adoption(&adoption())
                .unwrap();
            std::fs::write(&path, bytes).unwrap();
            assert!(
                State::from_journal(tx.journal())
                    .fence_adoption(&adoption())
                    .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            Ok(())
        })
        .unwrap();
}

#[test]
fn adoption_fence_restores_unlinked_marker_and_preserves_identical_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    let adoption = adoption();
    let path = dir.path().join(".catalog-sync/publication.json");
    store
        .with_transaction::<_, StorageError>(|tx| {
            let state = State::from_journal(tx.journal());
            state
                .write_publication(&PublicationMarker::Attempting(adoption.attempt().clone()))
                .unwrap();
            state.fence_adoption(&adoption).unwrap();
            let original = std::fs::read(&path).unwrap();
            let writes = Arc::new(AtomicUsize::new(0));
            let observed = writes.clone();
            let marker_path = path.clone();
            let state = state.with_hook(Arc::new(Mutex::new(move |op: &str, name: &str| {
                if name == "publication.json" {
                    if op == "clear" {
                        std::fs::remove_file(&marker_path).unwrap();
                        return Err(RepairRequired::new("failure after unlink"));
                    }
                    if op == "write" {
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                }
                Ok(())
            })));
            state.fence_adoption(&adoption).unwrap();
            assert_eq!(
                writes.load(Ordering::SeqCst),
                0,
                "identical fence must not be rewritten"
            );
            assert!(state.clear("publication.json").is_err());
            assert!(!path.exists(), "the injected error follows a real unlink");
            state.fence_adoption(&adoption).unwrap();
            assert_eq!(writes.load(Ordering::SeqCst), 1);
            assert_eq!(std::fs::read(&path).unwrap(), original);
            Ok(())
        })
        .unwrap();
    // Fresh readers must find the exact tips, operation IDs and confirmation timestamp.
    YamlStore::new(dir.path())
        .with_inspection::<_, StorageError>(|tx| {
            let state = State::from_journal(tx.journal());
            assert_eq!(
                state.publication().unwrap(),
                Some(PublicationMarker::Adopting(adoption))
            );
            Ok(())
        })
        .unwrap();
}
