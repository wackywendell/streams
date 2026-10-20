use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

mod support {
    pub mod git;
}

use proptest::prelude::*;
use streams_temper::{
    model::{Host, NamingMode, Record, Session, Stream},
    operations::{Actor, DomainOutcome, Operation},
    sync::{
        Freshness,
        envelope::Origin,
        git::GitCatalog,
        git_io::{Git, GitError},
        history::read_history,
        outcomes::{LocalStatus, SharingStatus},
        policy::{Policy, SyncEvent},
        state::RepairRequired,
    },
};
use support::git::{install_hook, test_command};

// Tests override author/signing per subprocess, never process-global environment
// or user config. The production constructor keeps normal signing requirements.
fn catalog(root: &Path, host: &str) -> GitCatalog {
    let mut catalog = GitCatalog::new(root, host, Policy::default()).unwrap();
    install_hook(&mut catalog, |_, _, _, _| None);
    catalog.set_clock(Arc::new(|| 1000.0));
    catalog
}

struct Pair {
    _dir: tempfile::TempDir,
    remote: PathBuf,
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
    Pair {
        _dir: dir,
        remote,
        a,
        b,
    }
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

fn edit(catalog: &GitCatalog, id: &str, stream: &str, title: &str) -> Operation {
    let before = catalog
        .snapshot()
        .unwrap()
        .catalog
        .stream(stream)
        .unwrap()
        .clone();
    let mut after = before.clone();
    after.title = title.into();
    after.title_mode = NamingMode::Manual;
    Operation::new(
        id,
        Some(Record::Stream(before)),
        Record::Stream(after),
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
fn two_clones_combine_independent_edits_and_preserve_intent_origin() {
    let p = pair();
    let first = create("first", "one", "a");
    let origin = Origin::from([
        ("host".into(), "original-host".into()),
        ("session".into(), "original-session".into()),
    ]);
    assert!(p.a.save(&first, Some(&origin)).unwrap().accepted());
    accepted(&p.b, &create("second", "two", "b"));
    let before = p.b.git().head().unwrap();
    let live_index = std::fs::read(p.b.root().join(".git/index")).unwrap();
    sync(&p.a);
    let read = p.b.read(true).unwrap();
    assert_eq!(read.freshness, Freshness::Refreshed);
    assert_eq!(read.catalog.streams.len(), 2);
    assert_eq!(p.b.git().head().unwrap(), before);
    assert_eq!(
        std::fs::read(p.b.root().join(".git/index")).unwrap(),
        live_index
    );
    assert_eq!(p.b.snapshot().unwrap().catalog.streams.len(), 1);
    sync(&p.b);
    sync(&p.a);
    assert_eq!(p.a.snapshot().unwrap().catalog.streams.len(), 2);
    let history = read_history(p.b.git(), Some(&p.b.git().head().unwrap())).unwrap();
    let entry = history.operation("first").unwrap();
    assert_eq!(entry.operation, first);
    assert_eq!(entry.origin, origin);
}

#[test]
fn whole_batch_conflict_preserves_local_view_and_allows_later_save() {
    let p = pair();
    accepted(&p.a, &create("create", "work", "a"));
    sync(&p.a);
    sync(&p.b);
    accepted(&p.a, &edit(&p.a, "remote-edit", "work", "Remote"));
    accepted(&p.b, &edit(&p.b, "local-edit", "work", "Local"));
    accepted(&p.b, &create("independent", "other", "b"));
    sync(&p.a);
    let old = p.b.git().head().unwrap();
    let remote = p.a.git().head().unwrap();
    let result = p.b.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Blocked, "{result:?}");
    assert_eq!(result.conflicts[0].operation, "local-edit");
    assert_eq!(p.b.git().head().unwrap(), old);
    assert_eq!(p.a.git().head().unwrap(), remote);
    let read = p.b.read(true).unwrap();
    assert_eq!(read.catalog.stream("work").unwrap().title, "Local");
    assert_eq!(
        read.confirmed
            .unwrap()
            .catalog
            .stream("work")
            .unwrap()
            .title,
        "Remote"
    );
    accepted(&p.b, &create("later", "later", "b"));
    assert_eq!(
        p.b.sync(SyncEvent::Explicit).sharing,
        SharingStatus::Blocked
    );
    assert!(
        p.a.inspect_remote()
            .unwrap()
            .catalog
            .stream("other")
            .is_none()
    );
}

#[test]
fn retired_operation_keeps_receipt_and_changed_intent_is_rejected() {
    let p = pair();
    accepted(&p.a, &create("create", "work", "a"));
    sync(&p.a);
    sync(&p.b);
    let local = edit(&p.b, "local-title", "work", "Same");
    accepted(&p.b, &local);
    accepted(&p.a, &edit(&p.a, "remote-title", "work", "Same"));
    sync(&p.a);
    sync(&p.b);
    let history = read_history(p.b.git(), Some(&p.b.git().head().unwrap())).unwrap();
    assert!(!history.has_operation(local.id()));
    assert!(p.b.state().seen(&local).unwrap());
    let tip = p.b.git().head().unwrap();
    assert_eq!(
        p.b.save(&local, None).unwrap().local,
        LocalStatus::Unchanged
    );
    assert_eq!(p.b.git().head().unwrap(), tip);
    let changed = edit(&p.b, local.id(), "work", "Changed intent");
    assert!(p.b.save(&changed, None).unwrap().requires_repair());
}

#[test]
fn local_creation_authority_is_immediate_but_other_producers_are_rejected() {
    let p = pair();
    let op = create("create", "work", "a");
    accepted(&p.a, &op);
    assert_eq!(p.a.authority("work").unwrap().unwrap().token, "create");
    assert!(p.b.save(&op, None).is_err());
    sync(&p.a);
    p.b.read(true).unwrap();
    assert_eq!(p.b.authority("work").unwrap().unwrap().host_id, "a");
    let before =
        p.a.snapshot()
            .unwrap()
            .catalog
            .stream("work")
            .unwrap()
            .clone();
    let mut after = before.clone();
    after.goal = Some("Generated".into());
    let automatic = Operation::new(
        "foreign",
        Some(Record::Stream(before)),
        Record::Stream(after),
        Actor::Automatic,
        Some("create".into()),
        vec![],
    )
    .unwrap();
    assert!(p.b.save(&automatic, None).is_err());
    assert!(p.b.save(&contribution(&p.a), None).is_err());
    let host = Operation::new(
        "host-checkin",
        None,
        Record::Host(
            Host::new(
                "a",
                "A",
                chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap(),
            )
            .unwrap(),
        ),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(p.b.save(&host, None).is_err());
}

#[test]
fn no_push_sync_and_read_do_not_advance_last_push_or_publication_clock() {
    let mut p = pair();
    let baseline = p.a.state().metadata().unwrap().last_push;
    p.a.set_clock(Arc::new(|| 2000.0));
    let before = p.a.state().metadata().unwrap().last_sync;
    p.a.read(true).unwrap();
    assert_eq!(p.a.state().metadata().unwrap().last_sync, before);
    sync(&p.a);
    let metadata = p.a.state().metadata().unwrap();
    assert_eq!(metadata.last_sync, Some(2000.0));
    assert_eq!(metadata.last_push, baseline);
    accepted(&p.a, &create("later", "later", "a"));
    p.a.set_clock(Arc::new(|| 2001.0));
    assert_eq!(p.a.sync(SyncEvent::Stream).sharing, SharingStatus::Pending);
    sync(&p.a); // explicit bypasses M
    accepted(&p.a, &create("shutdown", "shutdown", "a"));
    sync(&p.a); // establishes a recent baseline
    accepted(&p.a, &create("force", "force", "a"));
    assert_eq!(p.a.sync(SyncEvent::Force).sharing, SharingStatus::Confirmed);
    accepted(&p.a, &create("final", "final", "a"));
    assert_eq!(
        p.a.sync(SyncEvent::Shutdown).sharing,
        SharingStatus::Confirmed
    );
}

#[test]
fn lost_response_is_confirmed_before_adoption() {
    let mut p = pair();
    accepted(&p.a, &create("create", "work", "a"));
    let pushes = Arc::new(AtomicUsize::new(0));
    let count = pushes.clone();
    install_hook(&mut p.a, move |git, args, input, index| {
        if args[0] == "push" {
            count.fetch_add(1, Ordering::SeqCst);
            Some(
                test_command(git, args, input, index)
                    .and_then(|_| Err(GitError::new("lost reply", None))),
            )
        } else {
            None
        }
    });
    sync(&p.a);
    assert_eq!(pushes.load(Ordering::SeqCst), 1);
    assert!(p.a.state().publication().unwrap().is_none());
}

#[test]
fn lost_reply_restart_and_intervening_save_publish_both_operations() {
    let mut p = pair();
    accepted(&p.a, &create("first", "first", "a"));
    let pushed = Arc::new(AtomicBool::new(false));
    let seen = pushed.clone();
    install_hook(&mut p.a, move |git, args, input, index| {
        if args[0] == "push" {
            let result = test_command(git, args, input, index);
            seen.store(true, Ordering::SeqCst);
            Some(result.and_then(|_| Err(GitError::new("lost reply", None))))
        } else if seen.load(Ordering::SeqCst) && args[0] == "ls-remote" {
            Some(Err(GitError::new("offline after push", None)))
        } else {
            None
        }
    });
    assert_eq!(
        p.a.sync(SyncEvent::Explicit).sharing,
        SharingStatus::Uncertain
    );
    let mut restarted = catalog(p.a.root(), "a");
    accepted(&restarted, &create("second", "second", "a"));
    restarted.set_clock(Arc::new(|| 2000.0));
    sync(&restarted);
    assert_eq!(restarted.snapshot().unwrap().catalog.streams.len(), 2);
    sync(&p.b);
    assert_eq!(p.b.snapshot().unwrap().catalog.streams.len(), 2);
}

#[test]
fn unaccepted_uncertain_push_reconciles_later_remote_edit() {
    let mut p = pair();
    accepted(&p.a, &create("local", "local", "a"));
    let attempted = Arc::new(AtomicBool::new(false));
    let seen = attempted.clone();
    install_hook(&mut p.a, move |_, args, _, _| {
        if args[0] == "push" {
            seen.store(true, Ordering::SeqCst);
            Some(Err(GitError::new(
                "connection lost before acceptance",
                None,
            )))
        } else if seen.load(Ordering::SeqCst) && args[0] == "ls-remote" {
            Some(Err(GitError::new("offline", None)))
        } else {
            None
        }
    });
    let result = p.a.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Uncertain);
    assert_eq!(
        result.remote_revision, None,
        "failed confirmation must not claim a stale tip"
    );
    accepted(&p.a, &create("later", "later", "a"));
    accepted(&p.b, &create("remote", "remote", "b"));
    sync(&p.b);
    let restarted = catalog(p.a.root(), "a");
    sync(&restarted);
    assert_eq!(restarted.snapshot().unwrap().catalog.streams.len(), 3);
}

#[test]
fn remote_race_is_retried_against_new_history() {
    let mut p = pair();
    accepted(&p.a, &create("local", "local", "a"));
    accepted(&p.b, &create("remote", "remote", "b"));
    let other = p.b;
    let mut raced = false;
    install_hook(&mut p.a, move |_, args, _, _| {
        if args[0] == "push" && !raced {
            raced = true;
            sync(&other);
        }
        None
    });
    sync(&p.a);
    assert_eq!(p.a.snapshot().unwrap().catalog.streams.len(), 2);
}

#[test]
fn failed_adoption_retains_both_histories_and_fences_restart() {
    for command in ["read-tree", "update-ref"] {
        let mut p = pair();
        accepted(&p.a, &create("remote", "remote", "a"));
        sync(&p.a);
        accepted(&p.b, &create("local", "local", "b"));
        let old = p.b.git().head().unwrap();
        let retained_before = retained(&p.b).len();
        p.b.set_clock(Arc::new(|| 2000.0));
        install_hook(&mut p.b, move |_, args, _, _| {
            if (command == "read-tree" && args.starts_with(&["read-tree", "-m"]))
                || (command == "update-ref"
                    && args.first() == Some(&"update-ref")
                    && args.get(1) == Some(&"refs/heads/main"))
            {
                Some(Err(GitError::new("adoption failure", Some(1))))
            } else {
                None
            }
        });
        let result = p.b.sync(SyncEvent::Explicit);
        assert_eq!(result.sharing, SharingStatus::Confirmed, "{result:?}");
        assert_eq!(result.local, LocalStatus::RepairRequired, "{result:?}");
        let marker = p.b.state().publication().unwrap().unwrap();
        assert!(matches!(
            marker,
            streams_temper::sync::state::PublicationMarker::Adopting(_)
        ));
        assert_eq!(marker.attempt().confirmed_at, Some(2000.0));
        assert_eq!(
            marker.attempt().confirmed_at,
            p.b.state().metadata().unwrap().last_push
        );
        let retained = retained(&p.b);
        // One push-candidate ref plus exactly one adoption old/new pair.
        assert_eq!(retained.len(), retained_before + 3);
        assert_eq!(retained.iter().filter(|tip| *tip == &old).count(), 1);
        assert_eq!(
            retained
                .iter()
                .filter(|tip| *tip == &marker.attempt().candidate)
                .count(),
            2
        );
        for tip in &retained {
            read_history(p.b.git(), Some(tip)).unwrap();
        }
        let restarted = catalog(p.b.root(), "b");
        assert!(restarted.snapshot().is_err());
        assert!(
            restarted
                .save(&create("later", "later", "b"), None)
                .unwrap()
                .requires_repair()
        );
    }
}

#[test]
fn recovered_confirmation_survives_adoption_failure_and_newer_remote_history() {
    for restart in [false, true] {
        for command in ["read-tree", "update-ref"] {
            let mut p = pair();
            accepted(&p.a, &create("create", "work", "a"));
            let original = Arc::new(Mutex::new(None));
            let captured = original.clone();
            let state = p.a.state().clone();
            let mut pushed = false;
            install_hook(&mut p.a, move |git, args, input, index| {
                if args[0] == "push" {
                    *captured.lock().unwrap() =
                        Some(state.publication().unwrap().unwrap().attempt().clone());
                    pushed = true;
                    Some(
                        test_command(git, args, input, index)
                            .and_then(|_| Err(GitError::new("lost reply", None))),
                    )
                } else if restart && pushed && matches!(args[0], "ls-remote" | "fetch") {
                    Some(Err(GitError::new("confirmation unavailable", None)))
                } else if !restart
                    && index.is_none()
                    && ((command == "read-tree" && args.starts_with(&["read-tree", "-m", "-u"]))
                        || (command == "update-ref"
                            && args.starts_with(&["update-ref", "refs/heads/main"])))
                {
                    Some(Err(GitError::new("adoption failure", Some(1))))
                } else {
                    None
                }
            });
            let result = p.a.sync(SyncEvent::Explicit);
            let repaired = if restart {
                assert_eq!(result.sharing, SharingStatus::Uncertain);
                sync(&p.b);
                accepted(&p.b, &create("newer", "newer", "b"));
                sync(&p.b);
                let mut reopened = catalog(p.a.root(), "a");
                reopened.set_clock(Arc::new(|| 2000.0));
                install_hook(&mut reopened, move |_, args, _, index| {
                    if index.is_none()
                        && ((command == "read-tree"
                            && args.starts_with(&["read-tree", "-m", "-u"]))
                            || (command == "update-ref"
                                && args.starts_with(&["update-ref", "refs/heads/main"])))
                    {
                        Some(Err(GitError::new("adoption failure", Some(1))))
                    } else {
                        None
                    }
                });
                let result = reopened.sync(SyncEvent::Explicit);
                assert_eq!(
                    (result.sharing, result.local),
                    (SharingStatus::Confirmed, LocalStatus::RepairRequired),
                    "{result:?}"
                );
                reopened
            } else {
                assert_eq!(
                    (result.sharing, result.local),
                    (SharingStatus::Confirmed, LocalStatus::RepairRequired),
                    "{result:?}"
                );
                p.a
            };
            let mut expected = original.lock().unwrap().clone().unwrap();
            expected.confirmed_at = Some(if restart { 2000.0 } else { 1000.0 });
            let marker = repaired.state().publication().unwrap().unwrap();
            assert!(matches!(
                marker,
                streams_temper::sync::state::PublicationMarker::Adopting(_)
            ));
            assert_eq!(marker.attempt(), &expected);
            let metadata = repaired.state().metadata().unwrap();
            assert_eq!(metadata.last_push, expected.confirmed_at);
            assert_eq!(
                metadata.last_confirmed_attempt.as_ref(),
                Some(&expected.candidate)
            );
            if restart {
                assert_ne!(metadata.confirmed.as_ref(), Some(&expected.candidate));
                assert!(
                    repaired
                        .git()
                        .ancestor(&expected.candidate, metadata.confirmed.as_deref().unwrap())
                        .unwrap()
                );
            }
            let reopened = catalog(repaired.root(), "a");
            assert!(reopened.snapshot().is_err());
            assert!(reopened.sync(SyncEvent::Force).requires_repair());
            assert_eq!(
                reopened.state().metadata().unwrap().last_push,
                expected.confirmed_at
            );
        }
    }
}

#[test]
fn pure_remote_adoption_failure_does_not_claim_an_outgoing_confirmation() {
    let mut p = pair();
    accepted(&p.a, &create("remote", "remote", "a"));
    sync(&p.a);
    install_hook(&mut p.b, |_, args, _, index| {
        if index.is_none() && args.starts_with(&["read-tree", "-m", "-u"]) {
            Some(Err(GitError::new("adoption failure", Some(1))))
        } else {
            None
        }
    });
    let result = p.b.sync(SyncEvent::Explicit);
    assert_eq!(
        (result.sharing, result.local),
        (SharingStatus::Blocked, LocalStatus::RepairRequired),
        "{result:?}"
    );
    let marker = p.b.state().publication().unwrap().unwrap();
    assert_eq!(marker.attempt().confirmed_at, None);
    assert!(marker.attempt().operations.is_empty());
    assert_eq!(p.b.state().metadata().unwrap().last_push, None);
    assert!(catalog(p.b.root(), "b").snapshot().is_err());
}

#[test]
fn failed_attempt_marker_write_prevents_push_and_is_retryable_on_restart() {
    let mut p = pair();
    accepted(&p.b, &create("local", "local", "b"));
    let old = p.b.git().head().unwrap();
    let graphs = retained(&p.b).len();
    let baseline = p.b.state().metadata().unwrap().last_push;
    install_hook(&mut p.b, |_, args, _, _| {
        assert_ne!(args[0], "push", "attempt marker must precede push");
        None
    });
    let writes = Arc::new(AtomicUsize::new(0));
    let count = writes.clone();
    p.b.set_state_hook(Arc::new(Mutex::new(move |op: &str, name: &str| {
        if op == "write" && name == "publication.json" {
            count.fetch_add(1, Ordering::SeqCst);
            Err(RepairRequired::new("attempt marker unavailable"))
        } else {
            Ok(())
        }
    })));
    let result = p.b.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Blocked, "{result:?}");
    assert_eq!(result.local, LocalStatus::RepairRequired);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_eq!(p.b.git().head().unwrap(), old);
    assert!(p.b.state().publication().unwrap().is_none());
    assert_eq!(p.b.state().metadata().unwrap().last_push, baseline);
    assert_eq!(retained(&p.b).len(), graphs + 1);
    assert!(
        p.a.inspect_remote()
            .unwrap()
            .catalog
            .stream("local")
            .is_none()
    );
    let restarted = catalog(p.b.root(), "b");
    assert!(
        restarted
            .snapshot()
            .unwrap()
            .catalog
            .stream("local")
            .is_some()
    );
    sync(&restarted);
    assert!(restarted.state().publication().unwrap().is_none());
}

#[test]
fn confirmation_bookkeeping_faults_remain_attempting_and_recover_once() {
    // The first two sync.json writes after acknowledged push record its observed
    // remote tip and then its scheduling receipt. Neither is an adoption effect.
    for stage in [
        "remote-observation",
        "scheduling-receipt",
        "confirmed-marker",
    ] {
        let mut p = pair();
        accepted(&p.b, &create("local", "local", "b"));
        p.b.set_clock(Arc::new(|| 2000.0));
        let old = p.b.git().head().unwrap();
        let graphs = retained(&p.b).len();
        let pushed = Arc::new(AtomicBool::new(false));
        let seen = pushed.clone();
        install_hook(&mut p.b, move |git, args, input, index| {
            assert!(!args.starts_with(&["read-tree", "-m"]));
            if args[0] == "push" {
                let result = test_command(git, args, input, index);
                seen.store(true, Ordering::SeqCst);
                Some(result)
            } else {
                None
            }
        });
        let mut metadata_writes = 0;
        let mut publication_writes = 0;
        let faults = Arc::new(AtomicUsize::new(0));
        let count = faults.clone();
        p.b.set_state_hook(Arc::new(Mutex::new(move |op: &str, name: &str| {
            if op != "write" || !pushed.load(Ordering::SeqCst) {
                return Ok(());
            }
            if name == "sync.json" {
                metadata_writes += 1;
            }
            if name == "publication.json" {
                publication_writes += 1;
            }
            let fail = match stage {
                "remote-observation" => name == "sync.json" && metadata_writes == 1,
                "scheduling-receipt" => name == "sync.json" && metadata_writes == 2,
                "confirmed-marker" => name == "publication.json" && publication_writes == 1,
                _ => unreachable!(),
            };
            if fail {
                count.fetch_add(1, Ordering::SeqCst);
                Err(RepairRequired::new("confirmation bookkeeping unavailable"))
            } else {
                Ok(())
            }
        })));
        let result = p.b.sync(SyncEvent::Explicit);
        assert_eq!(
            result.sharing,
            SharingStatus::Blocked,
            "{stage}: {result:?}"
        );
        assert_eq!(result.local, LocalStatus::RepairRequired);
        assert_eq!(faults.load(Ordering::SeqCst), 1);
        assert_eq!(p.b.git().head().unwrap(), old);
        let marker = p.b.state().publication().unwrap().unwrap();
        assert!(matches!(
            marker,
            streams_temper::sync::state::PublicationMarker::Attempting(_)
        ));
        assert_eq!(marker.attempt().confirmed_at, None);
        let timestamp = if stage == "confirmed-marker" {
            2000.0
        } else {
            3000.0
        };
        assert_eq!(
            p.b.state().metadata().unwrap().last_push,
            (stage == "confirmed-marker").then_some(2000.0)
        );
        assert_eq!(retained(&p.b).len(), graphs + 1);
        assert!(
            p.a.inspect_remote()
                .unwrap()
                .catalog
                .stream("local")
                .is_some()
        );
        let mut restarted = catalog(p.b.root(), "b");
        restarted.set_clock(Arc::new(|| 3000.0));
        install_hook(&mut restarted, |_, args, _, _| {
            assert_ne!(
                args[0], "push",
                "recover accepted candidate without republishing"
            );
            None
        });
        sync(&restarted);
        assert_eq!(
            restarted.state().metadata().unwrap().last_push,
            Some(timestamp)
        );
        assert!(restarted.state().publication().unwrap().is_none());
        restarted.set_clock(Arc::new(|| 4000.0));
        sync(&restarted);
        assert_eq!(
            restarted.state().metadata().unwrap().last_push,
            Some(timestamp)
        );
    }
}

#[test]
fn adoption_journal_faults_preserve_confirmation_and_reassert_fence() {
    for stage in ["before-adoption", "before-clear", "after-unlink"] {
        let mut p = pair();
        accepted(&p.a, &create("remote", "remote", "a"));
        sync(&p.a);
        accepted(&p.b, &create("local", "local", "b"));
        p.b.set_clock(Arc::new(|| 2000.0));
        let old = p.b.git().head().unwrap();
        let graphs = retained(&p.b).len();
        let checked_out = Arc::new(AtomicBool::new(false));
        let seen = checked_out.clone();
        install_hook(&mut p.b, move |_, args, _, _| {
            if args.starts_with(&["read-tree", "-m"]) {
                seen.store(true, Ordering::SeqCst);
            }
            None
        });
        let marker_path = p.b.root().join(".catalog-sync/publication.json");
        let faults = Arc::new(AtomicUsize::new(0));
        let count = faults.clone();
        let mut publication_writes = 0;
        p.b.set_state_hook(Arc::new(Mutex::new(move |op: &str, name: &str| {
            if name != "publication.json" {
                return Ok(());
            }
            if op == "write" {
                publication_writes += 1;
            }
            // Attempting, confirmed Attempting, then Adopting. Fail only the
            // first adoption write: the error path must durably reassert it.
            let fail = (stage == "before-adoption" && op == "write" && publication_writes == 3)
                || (stage != "before-adoption" && op == "clear");
            if fail {
                assert_eq!(
                    checked_out.load(Ordering::SeqCst),
                    stage != "before-adoption"
                );
                if stage == "after-unlink" {
                    // StateHook runs before I/O. Explicitly model the filesystem
                    // state left by unlink succeeding and directory fsync failing.
                    std::fs::remove_file(&marker_path).unwrap();
                }
                count.fetch_add(1, Ordering::SeqCst);
                Err(RepairRequired::new("adoption journal failure"))
            } else {
                Ok(())
            }
        })));
        let result = p.b.sync(SyncEvent::Explicit);
        assert_eq!(
            result.sharing,
            SharingStatus::Confirmed,
            "{stage}: {result:?}"
        );
        assert_eq!(result.local, LocalStatus::RepairRequired);
        assert_eq!(faults.load(Ordering::SeqCst), 1);
        let marker = p.b.state().publication().unwrap().unwrap();
        assert!(matches!(
            marker,
            streams_temper::sync::state::PublicationMarker::Adopting(_)
        ));
        assert_eq!(marker.attempt().confirmed_at, Some(2000.0));
        assert_eq!(
            p.b.state().metadata().unwrap().last_push,
            marker.attempt().confirmed_at
        );
        assert_eq!(
            p.b.git().head().unwrap(),
            if stage == "before-adoption" {
                old.clone()
            } else {
                marker.attempt().candidate.clone()
            }
        );
        let refs = retained(&p.b);
        assert_eq!(refs.len(), graphs + 3);
        assert!(refs.contains(&old));
        assert!(refs.contains(&marker.attempt().candidate));
        for tip in refs {
            read_history(p.b.git(), Some(&tip)).unwrap();
        }
        let mut restarted = catalog(p.b.root(), "b");
        restarted.set_clock(Arc::new(|| 3000.0));
        assert!(restarted.snapshot().is_err());
        let retry = restarted.sync(SyncEvent::Force);
        assert_eq!(retry.sharing, SharingStatus::Blocked);
        assert!(retry.requires_repair());
        assert_eq!(
            restarted.state().metadata().unwrap().last_push,
            Some(2000.0)
        );
        assert_eq!(restarted.state().publication().unwrap(), Some(marker));
    }
}

#[test]
fn save_faults_fence_mutated_records_and_history() {
    for command in ["add", "commit-tree", "update-ref"] {
        let mut p = pair();
        install_hook(&mut p.a, move |_, args, _, _| {
            if args[0] == command {
                Some(Err(GitError::new("save failure", Some(1))))
            } else {
                None
            }
        });
        let outcome = p.a.save(&create("create", "work", "a"), None).unwrap();
        assert_eq!(outcome.local, LocalStatus::RepairRequired);
        assert!(p.a.state().read("save.json").unwrap().is_some());
        assert!(catalog(p.a.root(), "a").snapshot().is_err());
    }
    for name in ["receipts.json", "save.json"] {
        let mut p = pair();
        p.a.set_state_hook(Arc::new(Mutex::new(move |op: &str, file: &str| {
            if (name == "receipts.json" && op == "write" && file == name)
                || (name == "save.json" && op == "clear" && file == name)
            {
                Err(RepairRequired::new("durable receipt failure"))
            } else {
                Ok(())
            }
        })));
        assert!(
            p.a.save(&create("create", "work", "a"), None)
                .unwrap()
                .requires_repair()
        );
        assert!(catalog(p.a.root(), "a").snapshot().is_err());
    }
}

#[test]
fn dirty_ignored_and_unsupported_history_fail_closed() {
    let p = pair();
    std::fs::write(p.a.root().join("unexpected"), "do not publish").unwrap();
    assert!(p.a.snapshot().is_err());
    assert!(p.a.sync(SyncEvent::Explicit).requires_repair());
    let p = pair();
    std::fs::create_dir_all(p.a.root().join(".git/info")).unwrap();
    std::fs::write(p.a.root().join(".git/info/exclude"), "private\n").unwrap();
    std::fs::write(p.a.root().join("private"), "ignored notes").unwrap();
    assert!(p.a.snapshot().is_err());
    let p = pair();
    let tree = p.a.git().text(&["write-tree"], None, None, &[0]).unwrap();
    let bad =
        p.a.git()
            .commit(
                &tree,
                "Unsupported commit\n",
                Some(&p.a.git().head().unwrap()),
            )
            .unwrap();
    p.a.git()
        .run(&["update-ref", "refs/heads/main", &bad], None, None, &[0])
        .unwrap();
    assert!(p.a.snapshot().is_err());
}

#[test]
fn replaced_remote_and_changed_push_target_are_rejected() {
    let p = pair();
    accepted(&p.a, &create("create", "work", "a"));
    sync(&p.a);
    p.b.read(true).unwrap();
    let root = read_history(p.a.git(), Some(&p.a.git().head().unwrap()))
        .unwrap()
        .root
        .unwrap();
    Git::new(&p.remote, 10.0)
        .run_raw(&["update-ref", "refs/heads/main", &root], None, None, &[0])
        .unwrap();
    assert!(p.b.read(true).is_err());
    assert!(p.b.sync(SyncEvent::Explicit).requires_repair());
    let p = pair();
    p.a.git()
        .run(
            &["config", "remote.origin.pushurl", "/must-not-publish"],
            None,
            None,
            &[0],
        )
        .unwrap();
    assert!(p.a.sync(SyncEvent::Explicit).requires_repair());
}

#[test]
fn tampered_attempt_cannot_be_reused_or_confirmed() {
    let mut p = pair();
    accepted(&p.a, &create("create", "work", "a"));
    install_hook(&mut p.a, |_, args, _, _| {
        if args[0] == "push" {
            Some(Err(GitError::new("denied", Some(1))))
        } else {
            None
        }
    });
    assert_eq!(p.a.sync(SyncEvent::Explicit).sharing, SharingStatus::Failed);
    let mut attempt =
        p.a.state()
            .publication()
            .unwrap()
            .unwrap()
            .attempt()
            .clone();
    attempt.operations.clear();
    p.a.state()
        .write_publication(&streams_temper::sync::state::PublicationMarker::Attempting(
            attempt,
        ))
        .unwrap();
    let restarted = catalog(p.a.root(), "a");
    assert!(restarted.sync(SyncEvent::Explicit).requires_repair());
}

#[test]
fn stale_automatic_write_keeps_original_capture() {
    let p = pair();
    accepted(&p.a, &create("create", "work", "a"));
    let before =
        p.a.snapshot()
            .unwrap()
            .catalog
            .stream("work")
            .unwrap()
            .clone();
    let mut after = before.clone();
    after.goal = Some("Generated from old inputs".into());
    let automatic = Operation::new(
        "generated",
        Some(Record::Stream(before)),
        Record::Stream(after),
        Actor::Automatic,
        Some("create".into()),
        vec![],
    )
    .unwrap();
    accepted(&p.a, &edit(&p.a, "user", "work", "A new title"));
    let result = p.a.save(&automatic, None).unwrap();
    assert_eq!(result.domain.unwrap().outcome, DomainOutcome::Superseded);
}

#[test]
fn confirmed_lost_attempt_counts_once_across_restart_and_later_conflict() {
    let mut p = pair();
    accepted(&p.a, &create("create", "work", "a"));
    let pushed = Arc::new(AtomicBool::new(false));
    let seen = pushed.clone();
    install_hook(&mut p.a, move |git, args, input, index| {
        if args[0] == "push" {
            let result = test_command(git, args, input, index);
            seen.store(true, Ordering::SeqCst);
            Some(result.and_then(|_| Err(GitError::new("lost response", None))))
        } else if seen.load(Ordering::SeqCst) && args[0] == "ls-remote" {
            Some(Err(GitError::new("offline", None)))
        } else {
            None
        }
    });
    assert_eq!(
        p.a.sync(SyncEvent::Explicit).sharing,
        SharingStatus::Uncertain
    );
    sync(&p.b);
    accepted(&p.b, &edit(&p.b, "remote-title", "work", "Remote"));
    sync(&p.b);
    accepted(&p.a, &edit(&p.a, "local-title", "work", "Local"));
    let mut restarted = catalog(p.a.root(), "a");
    restarted.set_clock(Arc::new(|| 2000.0));
    let result = restarted.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Blocked, "{result:?}");
    assert!(result.needs_attention());
    assert!(!result.conflicts.is_empty());
    assert_eq!(
        restarted.state().metadata().unwrap().last_push,
        Some(2000.0)
    );
    let mut restarted = catalog(p.a.root(), "a");
    restarted.set_clock(Arc::new(|| 3000.0));
    let result = restarted.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Blocked, "{result:?}");
    assert!(result.needs_attention());
    assert!(!result.conflicts.is_empty());
    assert_eq!(
        restarted.state().metadata().unwrap().last_push,
        Some(2000.0)
    );
}

#[test]
fn earlier_confirmation_does_not_mask_later_transport_failures() {
    for stage in ["rejected", "confirmation-unavailable", "candidate-failed"] {
        let mut p = pair();
        accepted(&p.a, &create("first", "first", "a"));
        let pushed = Arc::new(AtomicBool::new(false));
        let seen = pushed.clone();
        install_hook(&mut p.a, move |git, args, input, index| {
            if args[0] == "push" {
                let result = test_command(git, args, input, index);
                seen.store(true, Ordering::SeqCst);
                Some(result.and_then(|_| Err(GitError::new("lost acknowledgement", None))))
            } else if seen.load(Ordering::SeqCst) && args[0] == "ls-remote" {
                Some(Err(GitError::new("offline", None)))
            } else {
                None
            }
        });
        assert_eq!(
            p.a.sync(SyncEvent::Explicit).sharing,
            SharingStatus::Uncertain
        );
        let first =
            p.a.state()
                .publication()
                .unwrap()
                .unwrap()
                .attempt()
                .candidate
                .clone();
        let mut restarted = catalog(p.a.root(), "a");
        accepted(&restarted, &create("second", "second", "a"));
        let saved = restarted.snapshot().unwrap();
        let old = restarted.git().head().unwrap();
        restarted.set_clock(Arc::new(|| 2000.0));
        let mut attempted = false;
        install_hook(&mut restarted, move |_, args, _, index| {
            if stage == "candidate-failed" && args[0] == "read-tree" && index.is_some() {
                Some(Err(GitError::new("candidate index unavailable", Some(1))))
            } else if args[0] == "push" {
                attempted = true;
                Some(Err(GitError::new("later push rejected", Some(1))))
            } else if stage == "confirmation-unavailable" && attempted && args[0] == "ls-remote" {
                Some(Err(GitError::new("later confirmation unavailable", None)))
            } else {
                None
            }
        });
        let result = restarted.sync(SyncEvent::Explicit);
        assert_eq!(
            result.sharing,
            if stage == "confirmation-unavailable" {
                SharingStatus::Uncertain
            } else {
                SharingStatus::Failed
            },
            "{stage}: {result:?}"
        );
        assert_eq!(result.local, LocalStatus::Unchanged);
        assert!(result.needs_attention());
        if stage == "confirmation-unavailable" {
            assert_eq!(result.remote_revision, None);
        }
        assert_eq!(restarted.git().head().unwrap(), old);
        assert_eq!(restarted.snapshot().unwrap(), saved);
        let metadata = restarted.state().metadata().unwrap();
        assert_eq!(metadata.last_push, Some(2000.0));
        assert_eq!(metadata.last_confirmed_attempt, Some(first));
    }
}

fn contribution(catalog: &GitCatalog) -> Operation {
    Operation::new(
        "session",
        None,
        Record::Session(
            Session::new(
                "session",
                catalog.host_id(),
                "work",
                "pi",
                "native",
                "Session",
            )
            .unwrap(),
        ),
        Actor::User,
        Some("create".into()),
        vec![],
    )
    .unwrap()
}

#[test]
fn cross_record_capture_is_preserved_on_replay_and_retired_when_stale() {
    for stale in [false, true] {
        let p = pair();
        accepted(&p.a, &create("create", "work", "a"));
        accepted(&p.a, &contribution(&p.a));
        sync(&p.a);
        sync(&p.b);
        let snapshot = p.a.snapshot().unwrap();
        let before = snapshot.catalog.session("a", "session").unwrap().clone();
        let mut after = before.clone();
        after.state = "Generated".into();
        let report = Operation::new(
            "report",
            Some(Record::Session(before)),
            Record::Session(after),
            Actor::Automatic,
            Some("create".into()),
            vec![Record::Stream(
                snapshot.catalog.stream("work").unwrap().clone(),
            )],
        )
        .unwrap();
        accepted(&p.a, &report);
        if stale {
            let before =
                p.b.snapshot()
                    .unwrap()
                    .catalog
                    .stream("work")
                    .unwrap()
                    .clone();
            let mut after = before.clone();
            after.goal = Some("Changed scope".into());
            after.goal_mode = NamingMode::Manual;
            let edit = Operation::new(
                "goal",
                Some(Record::Stream(before)),
                Record::Stream(after),
                Actor::User,
                None,
                vec![],
            )
            .unwrap();
            accepted(&p.b, &edit);
        } else {
            accepted(&p.b, &create("independent", "independent", "b"));
        }
        sync(&p.b);
        let remote = p.b.git().head().unwrap();
        sync(&p.a);
        let history = read_history(p.a.git(), Some(&p.a.git().head().unwrap())).unwrap();
        assert_eq!(
            history.catalog.session("a", "session").unwrap().state,
            if stale { "" } else { "Generated" }
        );
        if stale {
            assert!(!history.has_operation("report"));
            assert_eq!(p.a.git().head().unwrap(), remote);
            assert!(p.a.state().seen(&report).unwrap());
        } else {
            assert_eq!(history.operation("report").unwrap().operation, report);
        }
    }
}

#[test]
fn session_event_uses_pending_stream_gate_across_reopen() {
    let mut p = pair();
    accepted(&p.a, &create("create", "work", "a"));
    sync(&p.a);
    p.a.set_clock(Arc::new(|| 1001.0));
    accepted(&p.a, &contribution(&p.a));
    assert_eq!(p.a.sync(SyncEvent::Session).sharing, SharingStatus::Pending);
    accepted(&p.a, &edit(&p.a, "title", "work", "New title"));
    let mut reopened = catalog(p.a.root(), "a");
    reopened.set_clock(Arc::new(|| 1299.0));
    assert_eq!(
        reopened.sync(SyncEvent::Session).sharing,
        SharingStatus::Pending
    );
    reopened.set_clock(Arc::new(|| 1300.0));
    reopened.read(true).unwrap();
    assert_eq!(
        reopened.sync(SyncEvent::Session).sharing,
        SharingStatus::Confirmed
    );
    assert_eq!(reopened.state().metadata().unwrap().last_push, Some(1300.0));
}

#[test]
fn read_cooldown_does_not_delay_eligible_publication() {
    let mut p = pair();
    accepted(&p.a, &create("local", "local", "a"));
    p.a.set_clock(Arc::new(|| 5000.0));
    let attempts = Arc::new(AtomicUsize::new(0));
    let count = attempts.clone();
    install_hook(&mut p.a, move |_, args, _, _| {
        if args[0] == "ls-remote" && count.fetch_add(1, Ordering::SeqCst) == 0 {
            Some(Err(GitError::new("offline read", None)))
        } else {
            None
        }
    });
    let read = p.a.read(false).unwrap();
    assert_eq!(read.freshness, Freshness::Failed);
    assert_eq!(read.fetched_at, Some(1000.0));
    let cached = p.a.read(false).unwrap();
    assert_eq!(cached.freshness, Freshness::Cached);
    assert!(cached.detail.contains("offline read"));
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(
        p.a.sync(SyncEvent::Stream).sharing,
        SharingStatus::Confirmed
    );
    assert!(p.a.state().metadata().unwrap().refresh_error.is_empty());
}

#[test]
fn failed_push_cooldown_is_shared_and_force_bypasses_it() {
    let mut p = pair();
    accepted(&p.a, &create("local", "local", "a"));
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    install_hook(&mut p.a, move |_, args, _, _| {
        if args[0] == "push" {
            count.fetch_add(1, Ordering::SeqCst);
            Some(Err(GitError::new("remote rejected", Some(1))))
        } else {
            None
        }
    });
    let baseline = p.a.state().metadata().unwrap().last_push;
    assert_eq!(p.a.sync(SyncEvent::Explicit).sharing, SharingStatus::Failed);
    assert_eq!(p.a.state().metadata().unwrap().last_push, baseline);
    assert_eq!(p.a.state().metadata().unwrap().retry_after, 1060.0);
    let reopened = catalog(p.a.root(), "a");
    assert_eq!(
        reopened.sync(SyncEvent::Stream).sharing,
        SharingStatus::Pending
    );
    assert_eq!(p.a.sync(SyncEvent::Force).sharing, SharingStatus::Failed);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(p.a.snapshot().unwrap().catalog.stream("local").is_some());
}

#[test]
fn matching_candidate_is_reused_without_recommitting() {
    let mut p = pair();
    accepted(&p.a, &create("local", "local", "a"));
    install_hook(&mut p.a, |_, args, _, _| {
        if args[0] == "push" {
            Some(Err(GitError::new("denied", Some(1))))
        } else {
            None
        }
    });
    assert_eq!(p.a.sync(SyncEvent::Explicit).sharing, SharingStatus::Failed);
    let candidate =
        p.a.state()
            .publication()
            .unwrap()
            .unwrap()
            .attempt()
            .candidate
            .clone();
    let mut reopened = catalog(p.a.root(), "a");
    install_hook(&mut reopened, |_, args, _, _| {
        assert_ne!(
            args[0], "commit-tree",
            "matching candidate should be reused"
        );
        None
    });
    sync(&reopened);
    assert_eq!(reopened.git().head().unwrap(), candidate);
}

#[test]
fn competing_pushes_stop_at_policy_budget_without_force() {
    let mut p = pair();
    accepted(&p.a, &create("local", "local", "a"));
    let b = p.b;
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    install_hook(&mut p.a, move |_, args, _, _| {
        if args[0] == "push" {
            assert!(
                !args
                    .iter()
                    .any(|arg| arg.starts_with('+') || arg.contains("force"))
            );
            let index = count.fetch_add(1, Ordering::SeqCst);
            accepted(
                &b,
                &create(&format!("remote-{index}"), &format!("remote-{index}"), "b"),
            );
            sync(&b);
        }
        None
    });
    let before = p.a.git().head().unwrap();
    let result = p.a.sync(SyncEvent::Explicit);
    assert_eq!(result.sharing, SharingStatus::Failed, "{result:?}");
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(p.a.git().head().unwrap(), before);
    assert!(p.a.state().publication().unwrap().is_some());
    accepted(&p.a, &create("later", "later", "a"));
}

#[test]
fn signing_failure_has_no_unsigned_fallback() {
    let mut p = pair();
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    install_hook(&mut p.a, move |git, args, input, index| {
        if args == ["config", "--bool", "--get", "commit.gpgsign"] {
            return Some(Ok(b"true\n".to_vec()));
        }
        if args[0] == "commit-tree" {
            assert!(args.contains(&"-S"));
            count.fetch_add(1, Ordering::SeqCst);
            let mut configured = vec![
                "-c",
                "user.name=Streams test",
                "-c",
                "user.email=streams@example.invalid",
                "-c",
                "commit.gpgsign=true",
                "-c",
                "gpg.format=openpgp",
                "-c",
                "gpg.program=/nonexistent-streams-signing-program",
            ];
            configured.extend_from_slice(args);
            Some(git.run_raw(&configured, input, index, &[0]))
        } else {
            None
        }
    });
    let before = p.a.git().head().unwrap();
    let result = p.a.save(&create("local", "local", "a"), None).unwrap();
    assert!(result.requires_repair());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(p.a.git().head().unwrap(), before);
    assert!(p.a.state().read("save.json").unwrap().is_some());
}

#[test]
fn local_git_mode_has_no_remote_activity_and_setup_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("catalog");
    let mut local = catalog(&root, "host").initialize(None, "main").unwrap();
    install_hook(&mut local, |_, args, _, _| {
        assert!(!matches!(args[0], "fetch" | "push" | "ls-remote"));
        None
    });
    let operation = create("create", "work", "host");
    let result = local.save(&operation, None).unwrap();
    assert_eq!(result.sharing, SharingStatus::LocalOnly);
    assert_eq!(result.local, LocalStatus::Committed);
    assert_eq!(local.read(true).unwrap().freshness, Freshness::LocalOnly);
    assert_eq!(
        local.sync(SyncEvent::Force).sharing,
        SharingStatus::LocalOnly
    );
    assert!(local.inspect_remote().is_err());
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]
    #[test]
    fn restart_after_lost_ack_preserves_generated_save_sequence(
        accepted_remote in any::<bool>(),
        competing_remote in any::<bool>(),
        interrupted_adoption in any::<bool>(),
        ids in prop::collection::vec(0u8..6, 1..5),
    ) {
        let mut p = pair();
        accepted(&p.a, &create("initial", "initial", "a"));
        let pushed = Arc::new(AtomicBool::new(false));
        let seen = pushed.clone();
        install_hook(&mut p.a, move |git, args, input, index| {
            if args[0] == "push" {
                let result = if accepted_remote { test_command(git, args, input, index) } else { Ok(Vec::new()) };
                seen.store(true, Ordering::SeqCst);
                Some(result.and_then(|_| Err(GitError::new("uncertain push", None))))
            } else if seen.load(Ordering::SeqCst) && args[0] == "ls-remote" {
                Some(Err(GitError::new("confirmation unavailable", None)))
            } else { None }
        });
        prop_assert_eq!(p.a.sync(SyncEvent::Explicit).sharing, SharingStatus::Uncertain);
        let mut expected = std::collections::BTreeSet::new();
        for id in ids {
            let id = format!("stream-{id}");
            let reopened = catalog(p.a.root(), "a");
            let result = reopened.save(&create(&id, &id, "a"), None).unwrap();
            prop_assert!(result.accepted());
            prop_assert_eq!(result.local, if expected.insert(id) { LocalStatus::Committed } else { LocalStatus::Unchanged });
        }
        let mut reopened = catalog(p.a.root(), "a");
        let id = expected.iter().next().unwrap();
        let changed = Operation::new(
            id,
            None,
            Record::Stream(Stream::new(id, "Changed intent", None, "a").unwrap()),
            Actor::User,
            None,
            vec![],
        ).unwrap();
        let snapshot = reopened.snapshot().unwrap();
        let tip = reopened.git().head().unwrap();
        let receipts = reopened.state().fingerprints().unwrap();
        let marker = reopened.state().publication().unwrap();
        let graphs = retained(&reopened);
        prop_assert!(reopened.save(&changed, None).unwrap().requires_repair());
        prop_assert_eq!(reopened.snapshot().unwrap(), snapshot);
        prop_assert_eq!(reopened.git().head().unwrap(), tip);
        prop_assert_eq!(reopened.state().fingerprints().unwrap(), receipts);
        prop_assert_eq!(reopened.state().publication().unwrap(), marker);
        prop_assert_eq!(retained(&reopened), graphs);
        prop_assert!(reopened.save(&create(id, id, "a"), None).unwrap().accepted());
        if competing_remote {
            accepted(&p.b, &create("competing", "competing", "b"));
            sync(&p.b);
        }
        if interrupted_adoption {
            install_hook(&mut reopened, |_, args, _, index| {
                if index.is_none() && args.starts_with(&["read-tree", "-m", "-u"]) {
                    Some(Err(GitError::new("generated adoption failure", Some(1))))
                } else { None }
            });
            let result = reopened.sync(SyncEvent::Explicit);
            prop_assert_eq!(result.sharing, SharingStatus::Confirmed);
            prop_assert!(result.requires_repair());
            let restarted = catalog(reopened.root(), "a");
            prop_assert!(restarted.snapshot().is_err());
            prop_assert!(restarted.sync(SyncEvent::Force).requires_repair());
            prop_assert!(matches!(restarted.state().publication().unwrap(),
                Some(streams_temper::sync::state::PublicationMarker::Adopting(_))));
        } else {
            sync(&reopened);
            prop_assert!(reopened.state().publication().unwrap().is_none());
        }
        sync(&p.b);
        prop_assert_eq!(p.b.snapshot().unwrap().catalog.streams.len(), expected.len() + 1 + usize::from(competing_remote));
    }
}
