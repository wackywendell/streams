use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

mod support {
    pub mod git;
}

use streams_temper::{
    model::{Record, Stream},
    operations::{Actor, Operation},
    sync::{
        git::GitCatalog,
        git_io::GitError,
        history::read_history,
        outcomes::LocalStatus,
        policy::{Policy, SyncEvent},
    },
};
use support::git::install_hook;

fn state<T>(
    catalog: &GitCatalog,
    f: impl FnOnce(&streams_temper::sync::state::State<'_>) -> T,
) -> T {
    use streams_temper::storage::{StorageError, Transaction};
    catalog
        .store()
        .with_transaction::<_, StorageError>(|tx| {
            Ok(f(&streams_temper::sync::state::State::from_journal(
                tx.journal(),
            )))
        })
        .unwrap()
}

// Per-subprocess author/signing overrides never change user or process-global config.
fn catalog(root: &Path, host: &str) -> GitCatalog {
    let mut catalog = GitCatalog::new(root, host, Policy::default()).unwrap();
    install_hook(&mut catalog, |_, _, _, _| None);
    catalog.set_clock(Arc::new(|| 1000.0));
    catalog
}

fn create(id: &str, stream: &str, host: &str) -> Operation {
    Operation::new(
        id,
        None,
        Record::Stream(Stream::new(stream, stream, None, host).unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}

fn accepted(catalog: &GitCatalog, operation: &Operation) {
    let result = catalog.save(operation, None).unwrap();
    assert!(result.accepted(), "{result:?}");
}

#[test]
fn physical_save_faults_keep_partial_writes_fenced_after_reopen() {
    for command in ["add", "commit-tree", "update-ref"] {
        let dir = tempfile::tempdir().unwrap();
        let mut local = catalog(&dir.path().join("catalog"), "host")
            .initialize(None, "main")
            .unwrap();
        let head = local.git().head().unwrap();
        let index = std::fs::read(local.root().join(".git/index")).unwrap();
        let failed = Arc::new(AtomicBool::new(false));
        let seen = failed.clone();
        install_hook(&mut local, move |_, args, _, _| {
            if args[0] == command {
                seen.store(true, Ordering::SeqCst);
                Some(Err(GitError::new("physical save failure", Some(1))))
            } else {
                None
            }
        });
        let result = local.save(&create("create", "work", "host"), None).unwrap();
        assert_eq!(result.local, LocalStatus::RepairRequired, "{command}");
        assert!(failed.load(Ordering::SeqCst), "{command}");
        assert_eq!(local.git().head().unwrap(), head, "{command}");
        assert_eq!(
            std::fs::read(local.root().join(".git/index")).unwrap() == index,
            command == "add",
            "{command}: only failures after add leave a changed index"
        );
        let files = local
            .store()
            .with_inspection::<_, streams_temper::storage::StorageError>(|tx| tx.read())
            .unwrap();
        assert_eq!(files.catalog.streams().len(), 1);
        assert_eq!(files.catalog.streams()[0].id, "work");
        assert!(
            !read_history(local.git(), Some(&head))
                .unwrap()
                .has_operation("create")
        );
        let marker = state(&local, |state| state.read("save.json")).unwrap();
        assert!(marker.is_some());
        let restarted = catalog(local.root(), "host");
        assert!(restarted.snapshot().is_err());
        assert!(
            restarted
                .save(&create("later", "later", "host"), None)
                .unwrap()
                .requires_repair()
        );
        assert!(restarted.sync(SyncEvent::Force).requires_repair());
        assert_eq!(
            state(&restarted, |state| state.read("save.json")).unwrap(),
            marker
        );
    }
}

#[test]
fn dirty_ignored_and_unsupported_history_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let local = catalog(&dir.path().join("catalog"), "host")
        .initialize(None, "main")
        .unwrap();
    std::fs::write(local.root().join("unexpected"), "do not publish").unwrap();
    assert!(local.snapshot().is_err());
    assert!(local.sync(SyncEvent::Explicit).requires_repair());
    std::fs::remove_file(local.root().join("unexpected")).unwrap();
    std::fs::create_dir_all(local.root().join(".git/info")).unwrap();
    std::fs::write(local.root().join(".git/info/exclude"), "private\n").unwrap();
    std::fs::write(local.root().join("private"), "ignored notes").unwrap();
    assert!(local.snapshot().is_err());
    std::fs::remove_file(local.root().join("private")).unwrap();
    let tree = local.git().text(&["write-tree"], None, None, &[0]).unwrap();
    let bad = local
        .git()
        .commit(
            &tree,
            "Unsupported commit\n",
            Some(&local.git().head().unwrap()),
        )
        .unwrap();
    local
        .git()
        .run(&["update-ref", "refs/heads/main", &bad], None, None, &[0])
        .unwrap();
    assert!(local.snapshot().is_err());
}

#[test]
fn local_initialization_checks_identity_branch_and_no_clobber() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("catalog");
    let local = catalog(&root, "host").initialize(None, "main").unwrap();
    let operation = create("create", "work", "host");
    accepted(&local, &operation);
    assert!(catalog(&root, "different-host").snapshot().is_err());
    let reused = catalog(&root, "host").initialize(None, "main").unwrap();
    assert_eq!(
        reused.save(&operation, None).unwrap().local,
        LocalStatus::Unchanged
    );
    reused
        .git()
        .run(
            &["symbolic-ref", "HEAD", "refs/heads/other"],
            None,
            None,
            &[0],
        )
        .unwrap();
    assert!(reused.snapshot().is_err());
    let tip = local
        .git()
        .text(&["rev-parse", "refs/heads/main"], None, None, &[0])
        .unwrap();
    local
        .git()
        .run(
            &["update-ref", "--no-deref", "HEAD", &tip],
            None,
            None,
            &[0],
        )
        .unwrap();
    assert!(local.sync(SyncEvent::Explicit).requires_repair());
    let unrelated = dir.path().join("unrelated");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("notes"), "must stay untouched").unwrap();
    assert!(
        catalog(&unrelated, "host")
            .initialize(None, "main")
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(unrelated.join("notes")).unwrap(),
        "must stay untouched"
    );
}
