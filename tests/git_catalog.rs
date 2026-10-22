use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

mod support {
    pub mod git;
}

use streams_temper::{
    model::{Record, Stream},
    operations::{Actor, Operation},
    sync::{
        Freshness, Seconds,
        envelope::Origin,
        git::GitCatalog,
        git_io::{Git, GitError},
        history::read_history,
        outcomes::{LocalStatus, SharingStatus},
        policy::{Policy, SyncEvent},
        state::{PublicationMarker, RepairRequired},
    },
};
use support::git::{install_hook, test_command};

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

struct Pair {
    _dir: tempfile::TempDir,
    a: GitCatalog,
    b: GitCatalog,
}

fn pair() -> Pair {
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remote.git");
    std::fs::create_dir(&remote).unwrap();
    Git::new(&remote, 10.0)
        .run_raw(
            &["init", "--bare", "--initial-branch=main"],
            None,
            None,
            &[0],
        )
        .unwrap();
    let remote = remote.to_str().unwrap().to_owned();
    let a = catalog(&dir.path().join("a"), "a")
        .initialize(Some(remote.clone()), "main")
        .unwrap();
    sync(&a);
    let b = catalog(&dir.path().join("b"), "b")
        .initialize(Some(remote), "main")
        .unwrap();
    Pair { _dir: dir, a, b }
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

fn retained(catalog: &GitCatalog) -> Vec<String> {
    catalog
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
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn sync(catalog: &GitCatalog) {
    let result = catalog.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Confirmed, "{result:?}");
    assert!(!result.requires_repair(), "{result:?}");
}

#[test]
fn two_clones_roundtrip_provenance_and_lost_ack_reopen() {
    let mut p = pair();
    let first = create("first", "one", "a");
    let origin = Origin::from([
        ("host".into(), "original-host".into()),
        ("session".into(), "original-session".into()),
    ]);
    assert!(p.a.save(&first, Some(&origin)).unwrap().accepted());
    let second = create("second", "two", "b");
    accepted(&p.b, &second);
    let before = p.b.git().head().unwrap();
    let live_index = std::fs::read(p.b.root().join(".git/index")).unwrap();
    sync(&p.a);
    let read = p.b.read(true).unwrap();
    assert_eq!(read.freshness, Freshness::Refreshed);
    assert_eq!(read.catalog.streams().len(), 2);
    assert_eq!(p.b.git().head().unwrap(), before);
    assert_eq!(
        std::fs::read(p.b.root().join(".git/index")).unwrap(),
        live_index
    );
    assert_eq!(p.b.snapshot().unwrap().catalog.streams().len(), 1);

    let pushed = Arc::new(AtomicBool::new(false));
    let seen = pushed.clone();
    let pushes = Arc::new(AtomicUsize::new(0));
    let count = pushes.clone();
    install_hook(&mut p.b, move |git, args, input, index| {
        if args[0] == "push" {
            count.fetch_add(1, Ordering::SeqCst);
            let result = test_command(git, args, input, index);
            seen.store(true, Ordering::SeqCst);
            Some(result.and_then(|_| Err(GitError::new("lost reply", None))))
        } else if seen.load(Ordering::SeqCst) && matches!(args[0], "ls-remote" | "fetch") {
            Some(Err(GitError::new("offline after push", None)))
        } else {
            None
        }
    });
    assert_eq!(
        p.b.sync(SyncEvent::Explicit).sharing,
        SharingStatus::Uncertain
    );
    assert_eq!(pushes.load(Ordering::SeqCst), 1);
    assert_eq!(
        p.b.git().head().unwrap(),
        before,
        "unconfirmed candidate is not adopted"
    );
    assert!(state(&p.b, |state| state.publication()).unwrap().is_some());
    accepted(&p.b, &create("third", "three", "b"));
    let mut reopened = catalog(p.b.root(), "b");
    reopened.set_clock(Arc::new(|| 2000.0));
    sync(&reopened);
    assert!(
        state(&reopened, |state| state.publication())
            .unwrap()
            .is_none()
    );
    assert_eq!(reopened.snapshot().unwrap().catalog.streams().len(), 3);
    sync(&p.a);
    assert_eq!(p.a.snapshot().unwrap().catalog.streams().len(), 3);
    let history = read_history(reopened.git(), Some(&reopened.git().head().unwrap())).unwrap();
    let entry = history.operation("first").unwrap();
    assert_eq!(entry.operation, first);
    assert_eq!(entry.origin, origin);
    assert_eq!(history.operation("second").unwrap().operation, second);
    assert!(history.has_operation("third"));
}

#[test]
fn partial_adoption_retains_both_histories_and_fences_restart() {
    let mut p = pair();
    accepted(&p.a, &create("remote", "remote", "a"));
    sync(&p.a);
    accepted(&p.b, &create("local", "local", "b"));
    let old = p.b.git().head().unwrap();
    let old_index = std::fs::read(p.b.root().join(".git/index")).unwrap();
    let old_files = p.b.snapshot().unwrap().catalog;
    let retained_before = retained(&p.b).len();
    p.b.set_clock(Arc::new(|| 2000.0));
    let checkout_ran = Arc::new(AtomicBool::new(false));
    let observed = checkout_ran.clone();
    install_hook(&mut p.b, move |git, args, input, index| {
        if index.is_none() && args.starts_with(&["read-tree", "-m", "-u"]) {
            let result = test_command(git, args, input, index);
            assert!(result.is_ok(), "checkout must actually take effect");
            observed.store(true, Ordering::SeqCst);
            Some(result)
        } else if args.starts_with(&["update-ref", "refs/heads/main"]) {
            assert!(observed.load(Ordering::SeqCst));
            Some(Err(GitError::new(
                "HEAD update failed after checkout",
                Some(1),
            )))
        } else {
            None
        }
    });
    let result = p.b.sync(SyncEvent::Explicit);
    assert_eq!(
        (result.sharing, result.local),
        (SharingStatus::Confirmed, LocalStatus::RepairRequired),
        "{result:?}"
    );
    assert!(checkout_ran.load(Ordering::SeqCst));
    assert_eq!(p.b.git().head().unwrap(), old);
    assert_ne!(
        std::fs::read(p.b.root().join(".git/index")).unwrap(),
        old_index
    );
    let marker = state(&p.b, |state| state.publication()).unwrap().unwrap();
    assert!(matches!(
        marker,
        streams_temper::sync::state::PublicationMarker::Adopting(_)
    ));
    assert_eq!(
        marker.attempt().confirmed_at(),
        Some(Seconds::try_from(2000.0).unwrap())
    );
    assert_eq!(
        marker.attempt().confirmed_at(),
        state(&p.b, |state| state.metadata()).unwrap().last_push
    );
    let refs = retained(&p.b);
    assert_eq!(refs.len(), retained_before + 3);
    assert_eq!(refs.iter().filter(|tip| *tip == &old).count(), 1);
    assert_eq!(
        refs.iter()
            .filter(|tip| *tip == marker.attempt().candidate().as_str())
            .count(),
        2
    );
    for tip in &refs {
        read_history(p.b.git(), Some(tip)).unwrap();
    }
    // Read the worktree independently of the fenced catalog API.
    let files = streams_temper::yaml_store::YamlStore::new(p.b.root())
        .with_inspection::<_, streams_temper::storage::StorageError>(|tx| tx.read())
        .unwrap();
    assert_ne!(files.catalog, old_files);
    assert_eq!(
        files.catalog,
        *read_history(p.b.git(), Some(marker.attempt().candidate().as_str()))
            .unwrap()
            .catalog()
    );
    let restarted = catalog(p.b.root(), "b");
    assert!(restarted.snapshot().is_err());
    assert!(
        restarted
            .save(&create("later", "later", "b"), None)
            .unwrap()
            .requires_repair()
    );
    assert!(restarted.sync(SyncEvent::Force).requires_repair());
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
fn physical_publication_unlink_failure_reasserts_fence_after_adoption() {
    let mut p = pair();
    accepted(&p.a, &create("remote", "remote", "a"));
    sync(&p.a);
    accepted(&p.b, &create("local", "local", "b"));
    p.b.set_clock(Arc::new(|| 2000.0));
    let old = p.b.git().head().unwrap();
    let marker_path = p.b.root().join(".catalog-sync/publication.json");
    let unlinked = Arc::new(AtomicBool::new(false));
    let seen = unlinked.clone();
    p.b.set_state_hook(Arc::new(Mutex::new(move |op: &str, name: &str| {
        if op == "clear" && name == "publication.json" {
            // The hook precedes I/O: perform the actual unlink before failing
            // as a directory-fsync failure would after removal.
            std::fs::remove_file(&marker_path).unwrap();
            assert!(!marker_path.exists());
            seen.store(true, Ordering::SeqCst);
            Err(RepairRequired::new(
                "failure after physical publication unlink",
            ))
        } else {
            Ok(())
        }
    })));
    let result = p.b.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Confirmed, "{result:?}");
    assert_eq!(result.local, LocalStatus::RepairRequired);
    assert!(unlinked.load(Ordering::SeqCst));
    let marker = state(&p.b, |state| state.publication()).unwrap().unwrap();
    assert!(matches!(marker, PublicationMarker::Adopting(_)));
    let candidate = marker.attempt().candidate().as_str();
    assert_ne!(candidate, old);
    assert_eq!(p.b.git().head().unwrap(), candidate);
    assert_eq!(
        marker.attempt().confirmed_at(),
        Some(Seconds::try_from(2000.0).unwrap())
    );
    let files =
        p.b.store()
            .with_inspection::<_, streams_temper::storage::StorageError>(|tx| tx.read())
            .unwrap();
    assert_eq!(
        files.catalog,
        *read_history(p.b.git(), Some(candidate)).unwrap().catalog()
    );
    let restarted = catalog(p.b.root(), "b");
    assert!(restarted.snapshot().is_err());
    assert!(restarted.sync(SyncEvent::Force).requires_repair());
    assert_eq!(
        state(&restarted, |state| state.publication()).unwrap(),
        Some(marker)
    );
    assert_eq!(
        state(&restarted, |state| state.metadata())
            .unwrap()
            .last_push,
        Some(Seconds::try_from(2000.0).unwrap())
    );
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
