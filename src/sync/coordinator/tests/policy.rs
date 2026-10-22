use super::*;
use crate::model::{Host, NamingMode, Session};

fn edit(catalog: &MemoryRepository, id: &str, stream: &str, title: &str) -> Operation {
    let before = catalog.snapshot().catalog.stream(stream).unwrap().clone();
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
fn retired_operation_keeps_receipt_and_changed_intent_is_rejected() {
    let (a, b) = pair();
    accepted(a.save(&create("create", "work", "a")));
    confirmed(a.sync());
    confirmed(b.sync());
    let local = edit(&b, "local-title", "work", "Same");
    accepted(b.save(&local));
    accepted(a.save(&edit(&a, "remote-title", "work", "Same")));
    confirmed(a.sync());
    confirmed(b.sync());
    let history = b.history(Some(&b.head().unwrap())).unwrap();
    assert!(!history.has_operation(local.id()));
    assert!(b.run(|core| core.state.seen(&local)).unwrap());
    let tip = b.head().unwrap();
    assert_eq!(b.save(&local).local, LocalStatus::Unchanged);
    assert_eq!(b.head().unwrap(), tip);
    let changed = edit(&b, local.id(), "work", "Changed intent");
    assert!(b.save(&changed).requires_repair());
}

#[test]
fn local_creation_authority_is_immediate_but_other_producers_are_rejected() {
    let (a, b) = pair();
    let op = create("create", "work", "a");
    accepted(a.save(&op));
    assert_eq!(
        a.run(|core| core.authority("work")).unwrap().unwrap().token,
        "create"
    );
    assert!(b.run(|core| core.save(&op, None)).is_err());
    confirmed(a.sync());
    b.run(|core| core.read(true)).unwrap();
    assert_eq!(
        b.run(|core| core.authority("work"))
            .unwrap()
            .unwrap()
            .host_id,
        "a"
    );
    let before = a.snapshot().catalog.stream("work").unwrap().clone();
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
    assert!(b.run(|core| core.save(&automatic, None)).is_err());
    assert!(b.run(|core| core.save(&contribution(&a), None)).is_err());
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
    assert!(b.run(|core| core.save(&host, None)).is_err());
}

#[test]
fn refreshed_projection_does_not_adopt_or_change_saved_records() {
    let (a, b) = pair();
    accepted(a.save(&create("remote", "remote", "a")));
    accepted(b.save(&create("local", "local", "b")));
    confirmed(a.sync());
    let head = b.head().unwrap();
    let saved = b.snapshot();
    let retained = b.retained.borrow().clone();
    let pushes = b.push_count.get();
    let read = b.run(|core| core.read(true)).unwrap();
    assert_eq!(read.freshness, Freshness::Refreshed);
    assert_eq!(read.catalog.streams().len(), 2);
    assert_eq!(b.head().unwrap(), head);
    assert_eq!(b.snapshot(), saved);
    assert_eq!(*b.retained.borrow(), retained);
    assert_eq!(b.push_count.get(), pushes);
    assert!(b.marker().is_none());
}

#[test]
fn no_push_sync_and_read_do_not_advance_last_push_or_publication_clock() {
    let (a, _) = pair();
    let baseline = a.metadata().last_push;
    a.now.set(2000.0);
    let before = a.metadata().last_sync;
    a.run(|core| core.read(true)).unwrap();
    assert_eq!(a.metadata().last_sync, before);
    confirmed(a.sync());
    let metadata = a.metadata();
    assert_eq!(metadata.last_sync, Some(Seconds::try_from(2000.0).unwrap()));
    assert_eq!(metadata.last_push, baseline);
    accepted(a.save(&create("later", "later", "a")));
    a.now.set(2001.0);
    assert_eq!(
        a.run(|core| core.sync(SyncEvent::Stream)).sharing,
        SharingStatus::Pending
    );
    confirmed(a.sync());
    accepted(a.save(&create("shutdown", "shutdown", "a")));
    confirmed(a.sync());
    accepted(a.save(&create("force", "force", "a")));
    confirmed(a.run(|core| core.sync(SyncEvent::Force)));
    accepted(a.save(&create("final", "final", "a")));
    confirmed(a.run(|core| core.sync(SyncEvent::Shutdown)));
}

#[test]
fn confirmed_lost_attempt_counts_once_across_restart_and_later_conflict() {
    let (a, b) = pair();
    accepted(a.save(&create("create", "work", "a")));
    a.pushes.borrow_mut().push_back(Push::LoseAck {
        accepted: true,
        offline: true,
    });
    assert_eq!(a.sync().sharing, SharingStatus::Uncertain);
    confirmed(b.sync());
    accepted(b.save(&edit(&b, "remote-title", "work", "Remote")));
    confirmed(b.sync());
    accepted(a.save(&edit(&a, "local-title", "work", "Local")));
    a.fetch_unavailable.set(false);
    // Every run reopens the coordinator and journal state.
    for now in [2000.0, 3000.0] {
        a.now.set(now);
        let result = a.sync();
        assert_eq!(result.sharing, SharingStatus::Blocked, "{result:?}");
        assert!(result.needs_attention());
        assert!(!result.conflicts.is_empty());
        assert_eq!(
            a.metadata().last_push,
            Some(Seconds::try_from(2000.0).unwrap())
        );
    }
}

#[test]
fn earlier_confirmation_does_not_mask_later_transport_failures() {
    for stage in ["rejected", "confirmation-unavailable", "candidate-failed"] {
        let (a, _) = pair();
        accepted(a.save(&create("first", "first", "a")));
        a.pushes.borrow_mut().push_back(Push::LoseAck {
            accepted: true,
            offline: true,
        });
        assert_eq!(a.sync().sharing, SharingStatus::Uncertain);
        let first = a.marker().unwrap().attempt().candidate().to_owned();
        accepted(a.save(&create("second", "second", "a")));
        let saved = a.snapshot();
        let old = a.head().unwrap();
        a.fetch_unavailable.set(false);
        a.now.set(2000.0);
        match stage {
            "candidate-failed" => a.candidate_failure.set(true),
            "confirmation-unavailable" => a.pushes.borrow_mut().push_back(Push::RejectThenOffline),
            _ => a.pushes.borrow_mut().push_back(Push::Reject),
        }
        let result = a.sync();
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
        assert_eq!(a.head().unwrap(), old);
        assert_eq!(a.snapshot(), saved);
        let metadata = a.metadata();
        assert_eq!(metadata.last_push, Some(Seconds::try_from(2000.0).unwrap()));
        assert_eq!(metadata.last_confirmed_attempt, Some(first));
    }
}

#[test]
fn session_event_uses_pending_stream_gate_across_reopen() {
    let (a, _) = pair();
    accepted(a.save(&create("create", "work", "a")));
    confirmed(a.sync());
    a.now.set(1001.0);
    accepted(a.save(&contribution(&a)));
    assert_eq!(
        a.run(|core| core.sync(SyncEvent::Session)).sharing,
        SharingStatus::Pending
    );
    accepted(a.save(&edit(&a, "title", "work", "New title")));
    a.now.set(1299.0);
    assert_eq!(
        a.run(|core| core.sync(SyncEvent::Session)).sharing,
        SharingStatus::Pending
    );
    a.now.set(1300.0);
    a.run(|core| core.read(true)).unwrap();
    confirmed(a.run(|core| core.sync(SyncEvent::Session)));
    assert_eq!(
        a.metadata().last_push,
        Some(Seconds::try_from(1300.0).unwrap())
    );
}

#[test]
fn read_cooldown_does_not_delay_eligible_publication() {
    let (a, _) = pair();
    accepted(a.save(&create("local", "local", "a")));
    a.now.set(5000.0);
    a.fetch_errors.set(1);
    let read = a.run(|core| core.read(false)).unwrap();
    assert_eq!(read.freshness, Freshness::Failed);
    assert_eq!(read.fetched_at, Some(1000.0));
    assert_eq!(a.fetch_errors.get(), 0);
    // A fresh failure sentinel must survive the cached read: no second fetch.
    a.fetch_errors.set(1);
    let cached = a.run(|core| core.read(false)).unwrap();
    assert_eq!(cached.freshness, Freshness::Cached);
    assert!(cached.detail.contains("confirmation unavailable"));
    assert_eq!(a.fetch_errors.get(), 1);
    a.fetch_errors.set(0);
    confirmed(a.run(|core| core.sync(SyncEvent::Stream)));
    assert!(a.metadata().refresh_error.is_empty());
}

#[test]
fn failed_push_cooldown_is_shared_and_force_bypasses_it() {
    let (a, _) = pair();
    accepted(a.save(&create("local", "local", "a")));
    a.pushes.borrow_mut().extend([Push::Reject, Push::Reject]);
    let calls = a.push_count.get();
    let baseline = a.metadata().last_push;
    assert_eq!(a.sync().sharing, SharingStatus::Failed);
    assert_eq!(a.metadata().last_push, baseline);
    assert_eq!(a.metadata().retry_after.get(), 1060.0);
    assert_eq!(
        a.run(|core| core.sync(SyncEvent::Stream)).sharing,
        SharingStatus::Pending
    );
    assert_eq!(a.push_count.get() - calls, 1);
    assert_eq!(
        a.run(|core| core.sync(SyncEvent::Force)).sharing,
        SharingStatus::Failed
    );
    assert_eq!(a.push_count.get() - calls, 2);
    assert!(a.snapshot().catalog.stream("local").is_some());
}

#[test]
fn invalid_clock_is_rejected_at_scheduling_and_fresh_fetch_observations() {
    for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let (a, _) = pair();
        let before = a.metadata();
        a.now.set(invalid);
        assert!(
            a.run(|core| core.read(true))
                .unwrap_err()
                .to_string()
                .contains("expected a finite timestamp")
        );
        let result = a.run(|core| core.sync(SyncEvent::Force));
        assert!(result.requires_repair(), "{result:?}");
        assert_eq!(a.metadata(), before);
        a.clock_reads.set(0);
        a.clock_script.borrow_mut().extend([2000.0, invalid]);
        assert!(
            a.run(|core| core.read(true))
                .unwrap_err()
                .to_string()
                .contains("expected a finite timestamp")
        );
        assert_eq!(a.clock_reads.get(), 2);
        assert_eq!(a.metadata(), before);
    }
}

#[test]
fn finite_clock_and_cooldown_overflow_requires_repair_without_corrupting_state() {
    for refresh in [true, false] {
        let (mut a, _) = pair();
        accepted(a.save(&create("local", "local", "a")));
        a.policy = Policy::new(0.0, 0.0, f64::MAX, 30.0, 3).unwrap();
        a.now.set(f64::MAX);
        if refresh {
            a.fetch_unavailable.set(true);
            assert!(
                a.run(|core| core.read(true))
                    .unwrap_err()
                    .to_string()
                    .contains("expected a finite timestamp")
            );
        } else {
            a.pushes.borrow_mut().push_back(Push::Reject);
            let result = a.sync();
            assert!(result.requires_repair(), "{result:?}");
            assert!(
                result.detail.contains("expected a finite timestamp"),
                "{result:?}"
            );
        }
        let metadata = a.metadata();
        assert_eq!(metadata.retry_after, Seconds::ZERO);
        assert_eq!(metadata.refresh_retry_after, Seconds::ZERO);
    }
}

#[test]
fn matching_candidate_is_reused_without_recommitting() {
    let (a, _) = pair();
    accepted(a.save(&create("local", "local", "a")));
    a.pushes.borrow_mut().push_back(Push::Reject);
    assert_eq!(a.sync().sharing, SharingStatus::Failed);
    let candidate = a.marker().unwrap().attempt().candidate().to_owned();
    let candidates = a.candidate_count.get();
    confirmed(a.sync());
    assert_eq!(a.candidate_count.get(), candidates);
    assert_eq!(a.head().unwrap(), candidate.as_str());
}

#[test]
fn competing_pushes_stop_at_policy_budget() {
    let (a, b) = pair();
    accepted(a.save(&create("local", "local", "a")));
    let remote_before = a.remote.borrow().clone();
    let mut races = Vec::new();
    for index in 0..3 {
        let id = format!("remote-{index}");
        accepted(b.save(&create(&id, &id, "b")));
        confirmed(b.sync());
        races.push(Push::Race(b.head().unwrap()));
    }
    // Reveal each prepared competing history only at its corresponding push.
    *a.remote.borrow_mut() = remote_before;
    a.pushes.borrow_mut().extend(races);
    let calls = a.push_count.get();
    let before = a.head().unwrap();
    let result = a.sync();
    assert_eq!(result.sharing, SharingStatus::Failed, "{result:?}");
    assert_eq!(a.push_count.get() - calls, 3);
    assert_eq!(a.head().unwrap(), before);
    assert!(a.marker().is_some());
    accepted(a.save(&create("later", "later", "a")));
}

#[test]
fn tampered_attempt_cannot_be_reused_or_confirmed() {
    let (a, _) = pair();
    accepted(a.save(&create("create", "work", "a")));
    a.pushes.borrow_mut().push_back(Push::Reject);
    assert_eq!(a.sync().sharing, SharingStatus::Failed);
    let mut attempt = a
        .run(|core| core.state.read("publication.json"))
        .unwrap()
        .unwrap();
    attempt.insert("operations".into(), json!([]));
    a.run(|core| core.state.write("publication.json", attempt))
        .unwrap();
    assert!(a.sync().requires_repair());
}

#[test]
fn stale_automatic_write_keeps_original_capture() {
    let (a, _) = pair();
    accepted(a.save(&create("create", "work", "a")));
    let before = a.snapshot().catalog.stream("work").unwrap().clone();
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
    accepted(a.save(&edit(&a, "user", "work", "A new title")));
    let result = a.save(&automatic);
    assert_eq!(result.domain.unwrap().outcome(), DomainOutcome::Superseded);
}
