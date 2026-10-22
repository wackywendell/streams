use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

mod support {
    pub mod git;
}

use streams_temper::{
    model::{Record, Session, Stream},
    operations::{Actor, Operation},
    sync::{
        git::GitCatalog,
        git_io::{Git, GitError},
        history::read_history,
        outcomes::{LocalStatus, SharingStatus},
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
    let remote_text = remote.to_str().unwrap().to_owned();
    let a = catalog(&dir.path().join("a"), "a")
        .initialize(Some(remote_text.clone()), "main")
        .unwrap();
    assert_eq!(
        a.sync(SyncEvent::Explicit).sharing,
        SharingStatus::Confirmed
    );
    let b = catalog(&dir.path().join("b"), "b")
        .initialize(Some(remote_text), "main")
        .unwrap();
    Pair { _dir: dir, a, b }
}

fn sync(catalog: &GitCatalog) {
    let result = catalog.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Confirmed, "{result:?}");
    assert!(!result.requires_repair(), "{result:?}");
}

// Simulate the last, unpublished Session creation having been written by the
// previous Stream-partitioned writer, without changing its operation or bytes.
fn legacy_session_commit(catalog: &GitCatalog, session: &Session) {
    let record = Record::Session(session.clone());
    let canonical = catalog.store().record_path(&record);
    let legacy = PathBuf::from(format!(
        "sessions/{}/{}/{}.yaml",
        session.stream_id, session.host_id, session.id
    ));
    std::fs::create_dir_all(catalog.root().join(legacy.parent().unwrap())).unwrap();
    std::fs::rename(
        catalog.root().join(&canonical),
        catalog.root().join(&legacy),
    )
    .unwrap();
    let git = catalog.git();
    let head = git.head().unwrap();
    let parent = git.text(&["rev-parse", "HEAD^"], None, None, &[0]).unwrap();
    let message = git
        .text(&["show", "-s", "--format=%B", "HEAD"], None, None, &[0])
        .unwrap();
    git.run(
        &[
            "add",
            "--all",
            "--",
            &canonical.to_string_lossy(),
            &legacy.to_string_lossy(),
        ],
        None,
        None,
        &[0],
    )
    .unwrap();
    let tree = git.text(&["write-tree"], None, None, &[0]).unwrap();
    let replacement = git.commit(&tree, &message, Some(&parent)).unwrap();
    git.run(
        &["update-ref", "refs/heads/main", &replacement, &head],
        None,
        None,
        &[0],
    )
    .unwrap();
    assert_eq!(
        catalog.snapshot().unwrap().catalog.get(&record.key()),
        Some(record)
    );
}

#[test]
fn legacy_membership_move_is_saved_and_replayed_without_duplicate_session_files() {
    let p = pair();
    accepted(&p.a, &create("source-create", "source", "a"));
    accepted(&p.a, &create("destination-create", "destination", "a"));
    let session = Session::new("conversation", "a", "source", "pi", "native", "Work").unwrap();
    let token = p.a.authority("source").unwrap().unwrap().token;
    accepted(
        &p.a,
        &Operation::new(
            "session-create",
            None,
            Record::Session(session.clone()),
            Actor::User,
            Some(token.clone()),
            vec![],
        )
        .unwrap(),
    );
    legacy_session_commit(&p.a, &session);
    // Publish verbatim: replaying the initial creation would canonicalize the fixture.
    p.a.git()
        .run(
            &["push", "origin", "HEAD:refs/heads/main"],
            None,
            None,
            &[0],
        )
        .unwrap();
    sync(&p.b);

    // Force candidate replay over a newer remote history that still has the legacy file.
    accepted(&p.b, &create("unrelated-create", "unrelated", "b"));
    sync(&p.b);
    for catalog in [&p.a, &p.b] {
        let history = read_history(catalog.git(), Some(&catalog.git().head().unwrap())).unwrap();
        assert!(
            history
                .files()
                .contains_key("sessions/source/a/conversation.yaml")
        );
        assert!(
            !history
                .files()
                .contains_key("conversations/a/conversation.yaml")
        );
    }
    let source = p.a.snapshot().unwrap();
    let movement = Operation::reassociate(
        "native-move",
        &session,
        source.catalog.stream("source").unwrap(),
        "destination",
        token,
    )
    .unwrap();
    accepted(&p.a, &movement);
    assert!(
        !p.a.root()
            .join("sessions/source/a/conversation.yaml")
            .exists()
    );
    assert!(
        p.a.root()
            .join("conversations/a/conversation.yaml")
            .is_file()
    );
    sync(&p.a);
    sync(&p.b);
    for catalog in [&p.a, &p.b] {
        let snapshot = catalog.snapshot().unwrap();
        let moved = snapshot.catalog.session("a", "conversation").unwrap();
        assert_eq!(moved.stream_id, "destination");
        assert_eq!(moved.previous_stream_ids, ["source"]);
        assert_eq!(moved.title, "Work");
        assert_eq!(snapshot.catalog.sessions().len(), 1);
        let history = read_history(catalog.git(), Some(&catalog.git().head().unwrap())).unwrap();
        assert_eq!(
            history.operation("native-move").unwrap().operation,
            movement
        );
        assert!(
            !history
                .files()
                .contains_key("sessions/source/a/conversation.yaml")
        );
        assert!(
            history
                .files()
                .contains_key("conversations/a/conversation.yaml")
        );
    }
}

#[test]
fn history_rejects_retained_predecessors_and_noncanonical_session_relocations() {
    for case in ["retained", "modified", "reverse", "lateral"] {
        let p = pair();
        accepted(&p.a, &create("source-create", "source", "a"));
        accepted(&p.a, &create("destination-create", "destination", "a"));
        let session = Session::new("conversation", "a", "source", "pi", "native", "Work").unwrap();
        let token = p.a.authority("source").unwrap().unwrap().token;
        accepted(
            &p.a,
            &Operation::new(
                "session-create",
                None,
                Record::Session(session.clone()),
                Actor::User,
                Some(token.clone()),
                vec![],
            )
            .unwrap(),
        );
        if case != "reverse" {
            legacy_session_commit(&p.a, &session);
        }
        // Publish the fixture commit verbatim: normal sync can rematerialize it
        // at the canonical path before the second clone sees the legacy layout.
        p.a.git()
            .run(
                &["push", "origin", "HEAD:refs/heads/main"],
                None,
                None,
                &[0],
            )
            .unwrap();
        sync(&p.b);
        let parent = p.b.git().head().unwrap();
        let validated = read_history(p.b.git(), Some(&parent)).unwrap();
        let previous_path = if case == "reverse" {
            "conversations/a/conversation.yaml"
        } else {
            "sessions/source/a/conversation.yaml"
        };
        let previous_bytes = validated
            .files()
            .get(previous_path)
            .unwrap_or_else(|| {
                panic!(
                    "{case}: missing {previous_path}: {:?}",
                    validated.files().keys()
                )
            })
            .clone();
        accepted(
            &p.a,
            &Operation::reassociate(
                "native-move",
                &session,
                p.a.snapshot().unwrap().catalog.stream("source").unwrap(),
                "destination",
                token,
            )
            .unwrap(),
        );
        let canonical = p.a.root().join("conversations/a/conversation.yaml");
        match case {
            "retained" | "modified" => {
                let mut bytes = previous_bytes;
                if case == "modified" {
                    bytes.extend_from_slice(b"\n# old path was modified, not deleted\n");
                }
                let previous = p.a.root().join(previous_path);
                std::fs::create_dir_all(previous.parent().unwrap()).unwrap();
                std::fs::write(previous, bytes).unwrap();
            }
            "reverse" | "lateral" => {
                let target = p.a.root().join("sessions/destination/a/conversation.yaml");
                std::fs::create_dir_all(target.parent().unwrap()).unwrap();
                std::fs::rename(canonical, target).unwrap();
            }
            _ => unreachable!(),
        }
        let git = p.a.git();
        let message = git
            .text(&["show", "-s", "--format=%B", "HEAD"], None, None, &[0])
            .unwrap();
        git.run(&["add", "--all"], None, None, &[0]).unwrap();
        let tree = git.text(&["write-tree"], None, None, &[0]).unwrap();
        let malicious = git.commit(&tree, &message, Some(&parent)).unwrap();
        git.run(
            &["push", "origin", &format!("{malicious}:refs/heads/main")],
            None,
            None,
            &[0],
        )
        .unwrap();
        p.b.git()
            .run(&["fetch", "origin"], None, None, &[0])
            .unwrap();
        // Validate committed objects in the other clone, not the malformed worktree.
        let error = read_history(p.b.git(), Some(&malicious)).unwrap_err();
        assert!(
            error.to_string().contains("unsupported catalog history"),
            "{case}: {error}"
        );
        if case != "retained" {
            assert!(
                error.to_string().contains("commit relocation"),
                "{case}: {error}"
            );
        }
        assert!(p.b.sync(SyncEvent::Explicit).requires_repair(), "{case}");
        assert_eq!(p.b.git().head().unwrap(), parent, "{case}");
    }
}

#[test]
fn physical_save_faults_keep_partial_writes_fenced_after_reopen() {
    for command in ["add", "commit-tree", "update-ref"] {
        let dir = tempfile::tempdir().unwrap();
        let mut local = catalog(&dir.path().join("catalog"), "host");
        support::git::seed_bootstrap(&local);
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
    let local = catalog(&dir.path().join("catalog"), "host");
    support::git::seed_bootstrap(&local);
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
