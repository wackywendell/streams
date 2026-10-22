//! Adapter facts use two directly seeded Git revisions, not catalog workflows.
use super::*;
use crate::codec::record_to_data;
use crate::model::{Record, Stream};
use crate::operations::Actor;
use crate::storage::StorageError;
use crate::sync::replay::{Replay, replay};
use crate::yaml_store::relative_record_path;

#[path = "../../../tests/support/git.rs"]
mod fixture_git;

fn revisions() -> (tempfile::TempDir, GitCatalog, Oid, Oid, Operation, Origin) {
    let dir = tempfile::tempdir().unwrap();
    let mut catalog = GitCatalog::new(dir.path(), "host", Policy::default()).unwrap();
    fixture_git::install_hook(&mut catalog, |_, _, _, _| None);
    let git = catalog.git();
    git.run(&["init", "--initial-branch=main"], None, None, &[0])
        .unwrap();
    let record = Record::Stream(Stream::new("work", "Work", None, "host").unwrap());
    let operation = Operation::new("create", None, record, Actor::User, None, vec![]).unwrap();
    let origin = Origin::from([("session".into(), "original-session".into())]);
    let bytes = serde_json::to_string(&serde_json::json!({
        "version": 1, "revision": "fixture", "kind": "stream", "record": record_to_data(operation.after())
    })).unwrap();
    let message = encode_operation(&operation, Some(&origin)).unwrap();
    let path = relative_record_path(operation.after());
    let input = format!(
        "commit refs/heads/main\nmark :1\ncommitter Streams <streams@example.invalid> 1700000000 +0000\ndata {}\n{}\nblob\nmark :2\ndata {}\n{}\ncommit refs/heads/main\ncommitter Streams <streams@example.invalid> 1700000001 +0000\ndata {}\n{}\nfrom :1\nM 100644 :2 {}\n\ndone\n",
        BOOTSTRAP.len(),
        BOOTSTRAP,
        bytes.len(),
        bytes,
        message.len(),
        message,
        path.display()
    );
    git.run(
        &["fast-import", "--quiet", "--done"],
        Some(input.as_bytes()),
        None,
        &[0],
    )
    .unwrap();
    let old: Oid = git
        .text(&["rev-parse", "HEAD^"], None, None, &[0])
        .unwrap()
        .parse()
        .unwrap();
    let new: Oid = git.head().unwrap().parse().unwrap();
    git.run(
        &["read-tree", "--reset", "-u", old.as_str()],
        None,
        None,
        &[0],
    )
    .unwrap();
    git.run(
        &["update-ref", "refs/heads/main", old.as_str(), new.as_str()],
        None,
        None,
        &[0],
    )
    .unwrap();
    catalog
        .with_lock(|locked| {
            locked
                .state
                .write_config(&GitConfig::new(None, "main", "host").unwrap())?;
            Ok(())
        })
        .unwrap();
    (dir, catalog, old, new, operation, origin)
}

#[test]
fn partial_checkout_changes_files_and_index_without_moving_branch() {
    let (_dir, catalog, old, new, _, _) = revisions();
    let index = fs::read(catalog.root().join(".git/index")).unwrap();
    // A real ref lock makes Git's update-ref fail after read-tree succeeds.
    fs::write(catalog.root().join(".git/refs/heads/main.lock"), "locked").unwrap();
    let error = catalog
        .with_lock(|locked| locked.checkout(&old, &new))
        .unwrap_err();
    assert!(matches!(error, CatalogError::Git(_)), "{error}");
    assert_eq!(catalog.git().head().unwrap(), old.as_str());
    assert_ne!(fs::read(catalog.root().join(".git/index")).unwrap(), index);
    let files = catalog
        .store()
        .with_inspection::<_, StorageError>(|tx| tx.read())
        .unwrap();
    assert_eq!(
        files.catalog,
        *read_history(catalog.git(), Some(new.as_str()))
            .unwrap()
            .catalog()
    );
    catalog
        .with_lock(|locked| {
            assert!(
                locked.state.publication()?.is_none(),
                "checkout adapter does not own the recovery fence"
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn candidate_publication_preserves_original_intent_and_live_checkout() {
    let (dir, catalog, old, new, operation, origin) = revisions();
    let head = catalog.git().head().unwrap();
    let index = fs::read(catalog.root().join(".git/index")).unwrap();
    let local = read_history(catalog.git(), Some(new.as_str())).unwrap();
    let remote_history = read_history(catalog.git(), Some(old.as_str())).unwrap();
    let Replay::Ready { changes, .. } = replay(&local, &remote_history).unwrap() else {
        panic!("fixture has no conflicting operations")
    };
    let candidate: Oid = catalog
        .with_lock(|locked| locked.candidate(&local, &remote_history, changes))
        .unwrap()
        .parse()
        .unwrap();
    let remote = dir.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    fixture_git::test_command(
        &Git::new(&remote, 10.0),
        &["init", "--bare", "--initial-branch=main"],
        None,
        None,
    )
    .unwrap();
    catalog
        .git()
        .run(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            None,
            None,
            &[0],
        )
        .unwrap();
    catalog.git().push_candidate(&candidate, "main").unwrap();
    assert_eq!(
        catalog.git().fetch_tip("main").unwrap().as_deref(),
        Some(candidate.as_str())
    );
    let history = read_history(catalog.git(), Some(candidate.as_str())).unwrap();
    let entry = history.operation("create").unwrap();
    assert_eq!(entry.operation, operation);
    assert_eq!(entry.origin, origin);
    assert_eq!(catalog.git().head().unwrap(), head);
    assert_eq!(fs::read(catalog.root().join(".git/index")).unwrap(), index);
    assert!(
        !catalog
            .root()
            .join(catalog.store().record_path(operation.after()))
            .exists()
    );
}
