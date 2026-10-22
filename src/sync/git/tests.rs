//! Seeded Git histories keep adapter facts separate from catalog policy workflows.
use super::*;
use crate::codec::record_to_data;
use crate::model::{Record, Stream};
use crate::operations::Actor;
use crate::storage::StorageError;
use crate::yaml_store::relative_record_path;
use std::collections::BTreeSet;

#[path = "../../../tests/support/git.rs"]
mod fixture_git;

mod membership;

fn revisions() -> (tempfile::TempDir, GitCatalog, Oid, Oid, Operation) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("publisher");
    fs::create_dir(&root).unwrap();
    let mut catalog = GitCatalog::new(&root, "host", Policy::default()).unwrap();
    fixture_git::install_hook(&mut catalog, |_, _, _, _| None);
    let git = catalog.git();
    git.run(&["init", "--initial-branch=main"], None, None, &[0])
        .unwrap();
    let record = Record::Stream(Stream::new("work", "Work", None, "host").unwrap());
    let operation = Operation::new("create", None, record, Actor::User, None, vec![]).unwrap();
    let bytes = serde_json::to_string(&serde_json::json!({
        "version": 1, "revision": "fixture", "kind": "stream", "record": record_to_data(operation.after())
    })).unwrap();
    let message = encode_operation(&operation, None).unwrap();
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
    (dir, catalog, old, new, operation)
}

#[test]
fn partial_checkout_changes_files_and_index_without_moving_branch() {
    let (_dir, catalog, old, new, _) = revisions();
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
fn retained_histories_survive_successful_checkout() {
    let (_dir, catalog, old, new, operation) = revisions();
    catalog
        .with_lock(|locked| {
            locked.retain(old.as_str())?;
            locked.retain(new.as_str())?;
            locked.checkout(&old, &new)
        })
        .unwrap();
    let refs = catalog
        .git()
        .text(
            &[
                "for-each-ref",
                "--format=%(objectname)",
                "refs/catalog/retained/",
            ],
            None,
            None,
            &[0],
        )
        .unwrap();
    assert_eq!(
        refs.lines().collect::<BTreeSet<_>>(),
        BTreeSet::from([old.as_str(), new.as_str()])
    );
    assert_eq!(catalog.git().head().unwrap(), new.as_str());
    let records = catalog
        .store()
        .with_inspection::<_, StorageError>(|tx| tx.read())
        .unwrap();
    assert_eq!(
        records.catalog.get(&operation.after().key()).as_ref(),
        Some(operation.after())
    );
}
