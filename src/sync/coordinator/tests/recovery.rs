//! Recovery decisions use production histories and fresh coordinator/state wrappers.
//! These faults describe semantic effects, not filesystem crash durability.
use super::*;
use crate::model::{NamingMode, Session};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

fn edit(repo: &MemoryRepository, id: &str, stream: &str, title: &str) -> Operation {
    let before = repo.snapshot().catalog.stream(stream).unwrap().clone();
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

#[test]
fn lost_response_is_confirmed_before_adoption() {
    for budget in [1, 3] {
        let (mut a, _) = pair();
        a.policy = Policy::new(0.0, 0.0, 0.0, 30.0, budget).unwrap();
        accepted(a.save(&create("create", "work", "a")));
        let pushes = a.push_count.get();
        a.pushes.borrow_mut().push_back(Push::LoseAck {
            accepted: true,
            offline: false,
        });
        confirmed(a.sync());
        assert_eq!(a.push_count.get(), pushes + 1);
        assert!(a.marker().is_none());
    }
}

#[test]
fn unaccepted_uncertain_push_reconciles_later_remote_edit() {
    let (a, b) = pair();
    accepted(a.save(&create("local", "local", "a")));
    a.pushes.borrow_mut().push_back(Push::LoseAck {
        accepted: false,
        offline: true,
    });
    let result = a.sync();
    assert_eq!(result.sharing, SharingStatus::Uncertain);
    assert_eq!(
        result.remote_revision, None,
        "failed confirmation must not claim a stale tip"
    );
    accepted(a.save(&create("later", "later", "a")));
    accepted(b.save(&create("remote", "remote", "b")));
    confirmed(b.sync());
    a.fetch_unavailable.set(false);
    confirmed(a.sync());
    assert_eq!(a.snapshot().catalog.streams().len(), 3);
}

#[test]
fn remote_race_is_retried_against_new_history() {
    let (a, b) = pair();
    accepted(a.save(&create("local", "local", "a")));
    accepted(b.save(&create("remote", "remote", "b")));
    confirmed(b.sync());
    let tip = b.head().unwrap();
    // Initial observation is the old tip; the competing publisher wins at dispatch.
    *a.remote.borrow_mut() = a.metadata().confirmed.map(|oid| oid.to_string());
    a.pushes.borrow_mut().push_back(Push::Race(tip));
    confirmed(a.sync());
    assert_eq!(a.snapshot().catalog.streams().len(), 2);
    assert_eq!(a.push_count.get(), 3); // bootstrap plus race plus retry
}

#[test]
fn recovered_confirmation_survives_adoption_failure_and_newer_remote_history() {
    for restart in [false, true] {
        for stage in ["before-adoption", "checkout", "before-clear", "after-clear"] {
            let (a, b) = pair();
            accepted(a.save(&create("create", "work", "a")));
            let original = Rc::new(RefCell::new(None));
            let captured = original.clone();
            *a.after_push.borrow_mut() = Some(Box::new(move |repo| {
                *captured.borrow_mut() = Some(repo.marker().unwrap().attempt().clone());
            }));
            if restart {
                a.pushes.borrow_mut().push_back(Push::LoseAck {
                    accepted: true,
                    offline: true,
                });
                assert_eq!(a.sync().sharing, SharingStatus::Uncertain);
                a.fetch_unavailable.set(false);
                confirmed(b.sync());
                accepted(b.save(&create("newer", "newer", "b")));
                confirmed(b.sync());
                a.now.set(2000.0);
            } else {
                a.pushes.borrow_mut().push_back(Push::LoseAck {
                    accepted: true,
                    offline: false,
                });
            }
            let old = a.head().unwrap();
            match stage {
                "before-adoption" => a.journal.failure.set(Some(JournalFailure::BeforeAdoption)),
                "checkout" => a.checkout_failure.set(Some(CheckoutFailure::BeforeEffect)),
                "before-clear" => a.journal.failure.set(Some(JournalFailure::BeforeClear)),
                "after-clear" => a.journal.failure.set(Some(JournalFailure::AfterClear)),
                _ => unreachable!(),
            }
            let result = a.sync();
            assert_eq!(
                (result.sharing, result.local),
                (SharingStatus::Confirmed, LocalStatus::RepairRequired),
                "{stage}: {result:?}"
            );
            let expected = original.borrow().clone().unwrap().with_confirmed_at(Some(
                Seconds::try_from(if restart { 2000.0 } else { 1000.0 }).unwrap(),
            ));
            let marker = a.marker().unwrap();
            assert!(matches!(marker, PublicationMarker::Adopting(_)));
            assert_eq!(marker.attempt(), &expected);
            assert_eq!(
                a.head().unwrap(),
                if matches!(stage, "before-adoption" | "checkout") {
                    old
                } else {
                    let PublicationMarker::Adopting(adoption) = &marker else {
                        unreachable!()
                    };
                    adoption.adoption_to().to_string()
                }
            );
            let metadata = a.metadata();
            assert_eq!(metadata.last_push, expected.confirmed_at());
            assert_eq!(
                metadata.last_confirmed_attempt.as_ref(),
                Some(expected.candidate())
            );
            if restart {
                assert_ne!(metadata.confirmed.as_ref(), Some(expected.candidate()));
                assert!(
                    a.ancestor(
                        expected.candidate().as_str(),
                        metadata.confirmed.as_ref().unwrap().as_str()
                    )
                    .unwrap()
                );
            }
            assert!(a.run(|core| core.snapshot()).is_err());
            assert!(a.run(|core| core.sync(SyncEvent::Force)).requires_repair());
            assert_eq!(a.metadata().last_push, expected.confirmed_at());
        }
    }
}

#[test]
fn pure_remote_adoption_failure_does_not_claim_an_outgoing_confirmation() {
    let (a, b) = pair();
    accepted(a.save(&create("remote", "remote", "a")));
    confirmed(a.sync());
    b.checkout_failure.set(Some(CheckoutFailure::BeforeEffect));
    let result = b.sync();
    assert_eq!(
        (result.sharing, result.local),
        (SharingStatus::Blocked, LocalStatus::RepairRequired),
        "{result:?}"
    );
    let marker = b.marker().unwrap();
    assert_eq!(marker.attempt().confirmed_at(), None);
    assert!(marker.attempt().operations().is_empty());
    assert_eq!(b.metadata().last_push, None);
    assert!(b.run(|core| core.snapshot()).is_err());
}

#[test]
fn failed_attempt_marker_write_prevents_push_and_is_retryable_on_restart() {
    let (a, b) = pair();
    accepted(b.save(&create("local", "local", "b")));
    let old = b.head().unwrap();
    let graphs = b.retained.borrow().len();
    let baseline = b.metadata().last_push;
    let writes = Arc::new(AtomicUsize::new(0));
    let count = writes.clone();
    *b.state_hook.borrow_mut() = Some(Arc::new(Mutex::new(move |op: &str, name: &str| {
        if op == "write" && name == "publication.json" {
            count.fetch_add(1, Ordering::SeqCst);
            Err(RepairRequired::new("attempt marker unavailable"))
        } else {
            Ok(())
        }
    })));
    let result = b.sync();
    assert_eq!(result.sharing, SharingStatus::Blocked, "{result:?}");
    assert_eq!(result.local, LocalStatus::RepairRequired);
    assert_eq!(writes.load(Ordering::SeqCst), 1);
    assert_eq!(b.push_count.get(), 0);
    assert_eq!(b.head().unwrap(), old);
    assert!(b.marker().is_none());
    assert_eq!(b.metadata().last_push, baseline);
    assert_eq!(b.retained.borrow().len(), graphs + 1);
    assert!(
        a.run(|core| core.inspect_remote())
            .unwrap()
            .catalog
            .stream("local")
            .is_none()
    );
    b.state_hook.borrow_mut().take();
    assert!(b.snapshot().catalog.stream("local").is_some());
    confirmed(b.sync());
    assert!(b.marker().is_none());
}

#[test]
fn confirmation_bookkeeping_faults_remain_attempting_and_recover_once() {
    for stage in [
        "remote-observation",
        "scheduling-receipt",
        "confirmed-marker",
    ] {
        let (a, b) = pair();
        accepted(b.save(&create("local", "local", "b")));
        b.now.set(2000.0);
        let old = b.head().unwrap();
        let graphs = b.retained.borrow().len();
        let faults = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let activated = active.clone();
        let count = faults.clone();
        let mut metadata_writes = 0;
        let mut publication_writes = 0;
        *b.state_hook.borrow_mut() = Some(Arc::new(Mutex::new(move |op: &str, name: &str| {
            if op != "write" || !active.load(Ordering::SeqCst) {
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
        *b.after_push.borrow_mut() = Some(Box::new(move |_| {
            activated.store(true, Ordering::SeqCst);
        }));
        let result = b.sync();
        assert_eq!(
            result.sharing,
            SharingStatus::Blocked,
            "{stage}: {result:?}"
        );
        assert_eq!(result.local, LocalStatus::RepairRequired);
        assert_eq!(faults.load(Ordering::SeqCst), 1);
        assert_eq!(b.head().unwrap(), old);
        let marker = b.marker().unwrap();
        assert!(matches!(marker, PublicationMarker::Attempting(_)));
        assert_eq!(marker.attempt().confirmed_at(), None);
        let timestamp = if stage == "confirmed-marker" {
            2000.0
        } else {
            3000.0
        };
        assert_eq!(
            b.metadata().last_push,
            (stage == "confirmed-marker").then_some(Seconds::try_from(2000.0).unwrap())
        );
        assert_eq!(b.retained.borrow().len(), graphs + 1);
        assert!(
            a.run(|core| core.inspect_remote())
                .unwrap()
                .catalog
                .stream("local")
                .is_some()
        );
        b.state_hook.borrow_mut().take();
        b.now.set(3000.0);
        let pushes = b.push_count.get();
        confirmed(b.sync());
        assert_eq!(b.push_count.get(), pushes, "recover without republishing");
        assert_eq!(
            b.metadata().last_push,
            Some(Seconds::try_from(timestamp).unwrap())
        );
        assert!(b.marker().is_none());
        b.now.set(4000.0);
        confirmed(b.sync());
        assert_eq!(
            b.metadata().last_push,
            Some(Seconds::try_from(timestamp).unwrap())
        );
    }
}

#[test]
fn retain_failure_recovers_against_the_actual_prior_marker() {
    for prior_attempt in [false, true] {
        for remote_race in [false, true] {
            let (a, b) = pair();
            accepted(a.save(&create("local", "local", "a")));
            if prior_attempt {
                a.pushes.borrow_mut().push_back(Push::Reject);
                assert_eq!(a.sync().sharing, SharingStatus::Failed);
                accepted(a.save(&create("later", "later", "a")));
            }
            let marker = a.marker();
            *a.before_retain.borrow_mut() = Some(Box::new(move |_| {
                if remote_race {
                    accepted(b.save(&create("remote", "remote", "b")));
                    confirmed(b.sync());
                }
                Err(GitError::new("retain failed before marker", Some(1)).into())
            }));
            let result = a.sync();
            assert!(a.before_retain.borrow().is_none(), "fault exercised");
            if remote_race {
                assert_eq!(result.sharing, SharingStatus::Confirmed, "{result:?}");
                assert_eq!(
                    a.snapshot().catalog.streams().len(),
                    if prior_attempt { 3 } else { 2 }
                );
                assert!(a.marker().is_none());
            } else {
                assert_eq!(result.sharing, SharingStatus::Failed, "{result:?}");
                assert_eq!(result.local, LocalStatus::Unchanged);
                assert!(result.detail.contains("retain failed before marker"));
                assert_eq!(a.marker(), marker);
            }
        }
    }
}

#[test]
fn save_faults_fence_mutated_records_and_history() {
    for after_effect in [false, true] {
        let (a, _) = pair();
        let old = a.head().unwrap();
        a.commit_before_failure.set(!after_effect);
        a.commit_failure.set(after_effect);
        assert_eq!(
            a.save(&create("create", "work", "a")).local,
            LocalStatus::RepairRequired
        );
        assert!(
            a.run(|core| core.state.read("save.json"))
                .unwrap()
                .is_some()
        );
        assert!(a.run(|core| core.snapshot()).is_err());
        assert_eq!(a.head().unwrap() != old, after_effect);
        assert_eq!(
            a.records.borrow().catalog.stream("work").is_some(),
            after_effect
        );
    }
    for name in ["receipts.json", "save.json"] {
        let (a, _) = pair();
        *a.state_hook.borrow_mut() = Some(Arc::new(Mutex::new(move |op: &str, file: &str| {
            if (name == "receipts.json" && op == "write" && file == name)
                || (name == "save.json" && op == "clear" && file == name)
            {
                Err(RepairRequired::new("durable receipt failure"))
            } else {
                Ok(())
            }
        })));
        assert!(a.save(&create("create", "work", "a")).requires_repair());
        a.state_hook.borrow_mut().take();
        assert!(a.run(|core| core.snapshot()).is_err());
    }
}

#[test]
fn replaced_remote_is_rejected() {
    let (a, b) = pair();
    accepted(a.save(&create("create", "work", "a")));
    confirmed(a.sync());
    b.run(|core| core.read(true)).unwrap();
    let root = a
        .history(Some(&a.head().unwrap()))
        .unwrap()
        .root()
        .unwrap()
        .to_owned();
    *a.remote.borrow_mut() = Some(root);
    assert!(b.run(|core| core.read(true)).is_err());
    assert!(b.sync().requires_repair());
}

#[test]
fn reused_candidate_recovery_fences_changed_mutable_facts() {
    for changed in ["marker", "worktree"] {
        let (a, _) = pair();
        accepted(a.save(&create("local", "local", "a")));
        a.pushes.borrow_mut().push_back(Push::Reject);
        assert_eq!(a.sync().sharing, SharingStatus::Failed);
        let old = a.head().unwrap();
        let marker = a.marker();
        let baseline = a.metadata().last_push;
        let candidates = a.candidate_count.get();
        *a.after_push.borrow_mut() = Some(Box::new(move |repo| {
            if changed == "marker" {
                let mut objects = repo.journal.objects.borrow_mut();
                objects
                    .get_mut("publication.json")
                    .unwrap()
                    .insert("confirmed_at".into(), json!(2000.0));
            } else {
                repo.dirty.set(true);
            }
        }));
        a.pushes.borrow_mut().push_back(Push::LoseAck {
            accepted: false,
            offline: false,
        });
        let result = a.sync();
        assert_eq!(
            a.candidate_count.get(),
            candidates,
            "retained candidate reused"
        );
        assert_eq!(result.local, LocalStatus::RepairRequired, "{result:?}");
        assert_eq!(a.head().unwrap(), old);
        assert_eq!(a.metadata().last_push, baseline);
        if changed == "marker" {
            assert_ne!(a.marker(), marker);
            assert!(
                result.detail.contains("publication marker changed"),
                "{result:?}"
            );
        } else {
            assert_eq!(a.marker(), marker);
            assert!(result.detail.contains("working-tree change"), "{result:?}");
        }
    }
}

#[test]
fn local_git_mode_has_no_remote_activity() {
    let local = MemoryRepository::new(Rc::default(), Rc::default(), "host", false);
    let operation = create("create", "work", "host");
    let result = local.save(&operation);
    assert_eq!(result.sharing, SharingStatus::LocalOnly);
    assert_eq!(result.local, LocalStatus::Committed);
    assert_eq!(
        local.run(|core| core.read(true)).unwrap().freshness,
        Freshness::LocalOnly
    );
    assert_eq!(
        local.run(|core| core.sync(SyncEvent::Force)).sharing,
        SharingStatus::LocalOnly
    );
    assert!(local.run(|core| core.inspect_remote()).is_err());
    assert_eq!(local.save(&operation).local, LocalStatus::Unchanged);
    assert_eq!(local.push_count.get(), 0);
    assert_eq!(local.fetch_count.get(), 0);
    assert_eq!(local.clock_reads.get(), 0);
    assert!(local.metadata().confirmed.is_none());
}

#[test]
fn whole_batch_conflict_preserves_local_view_and_allows_later_save() {
    let (a, b) = pair();
    accepted(a.save(&create("create", "work", "a")));
    confirmed(a.sync());
    confirmed(b.sync());
    accepted(a.save(&edit(&a, "remote-edit", "work", "Remote")));
    accepted(b.save(&create("independent", "other", "b")));
    accepted(b.save(&edit(&b, "local-edit", "work", "Local")));
    confirmed(a.sync());
    let old = b.head().unwrap();
    let remote = a.head().unwrap();
    let local_snapshot = b.snapshot();
    let local_history = b.history(Some(&old)).unwrap();
    let remote_history = a.history(Some(&remote)).unwrap();
    let result = b.sync();
    assert_eq!(result.sharing, SharingStatus::Blocked, "{result:?}");
    assert_eq!(result.conflicts[0].operation, "local-edit");
    assert_eq!(b.head().unwrap(), old);
    assert_eq!(a.head().unwrap(), remote);
    let read = b.run(|core| core.read(true)).unwrap();
    assert_eq!(read.catalog, local_snapshot.catalog);
    assert_eq!(read.local, local_snapshot);
    assert_eq!(read.conflicts, result.conflicts);
    assert_eq!(b.history(Some(&old)).unwrap(), local_history);
    assert_eq!(a.history(Some(&remote)).unwrap(), remote_history);
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
    accepted(b.save(&create("later", "later", "b")));
    assert_eq!(b.sync().sharing, SharingStatus::Blocked);
    assert!(
        a.run(|core| core.inspect_remote())
            .unwrap()
            .catalog
            .stream("other")
            .is_none()
    );
}

fn contribution(catalog: &MemoryRepository) -> Operation {
    Operation::new(
        "session",
        None,
        Record::Session(
            Session::new("session", &catalog.host, "work", "pi", "native", "Session").unwrap(),
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
        let (a, b) = pair();
        accepted(a.save(&create("create", "work", "a")));
        accepted(a.save(&contribution(&a)));
        confirmed(a.sync());
        confirmed(b.sync());
        let snapshot = a.snapshot();
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
        accepted(a.save(&report));
        if stale {
            let before = b.snapshot().catalog.stream("work").unwrap().clone();
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
            accepted(b.save(&edit));
        } else {
            accepted(b.save(&create("independent", "independent", "b")));
        }
        confirmed(b.sync());
        let remote = b.head().unwrap();
        confirmed(a.sync());
        let history = a.history(Some(&a.head().unwrap())).unwrap();
        assert_eq!(
            history.catalog().session("a", "session").unwrap().state,
            if stale { "" } else { "Generated" }
        );
        if stale {
            assert!(!history.has_operation("report"));
            assert_eq!(a.head().unwrap(), remote);
            assert!(a.run(|core| core.state.seen(&report)).unwrap());
        } else {
            assert_eq!(history.operation("report").unwrap().operation, report);
        }
    }
}
