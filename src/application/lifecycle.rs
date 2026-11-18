//! Host-local Session lifecycle transitions and age decay.

use serde::Serialize;

use super::{OpenedRecords, Result};
use crate::{
    codec::record_to_data,
    model::{Record, RecordKind, Session, Status, ValidationError},
    operations::{DomainOutcome, decay_cutoff_ms, is_stale_ms},
    runtime::conversations::Harness,
    storage::records::{RecordAddress, RecordChange, RecordSave, RecordStore},
};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DecaySummary {
    pub closed_sessions: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// One accepted status change. Skipped transitions write nothing and are not listed.
pub type LifecycleSave = RecordSave<StatusChange>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StatusChange {
    pub outcome: DomainOutcome,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleOutcome {
    pub session: Option<Session>,
    pub session_changed: bool,
    pub saves: Vec<LifecycleSave>,
}

impl LifecycleOutcome {
    pub fn unchanged(session: Option<Session>) -> Self {
        Self {
            session,
            session_changed: false,
            saves: Vec::new(),
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let saves: Vec<serde_json::Value> = self
            .saves
            .iter()
            .map(|save| {
                serde_json::json!({
                    "target": save.target(),
                    "save": {"local": save.local(), "domain": save.outcome},
                })
            })
            .collect();
        serde_json::json!({
            "session_changed": self.session_changed,
            "saves": saves,
            "session": self.session.as_ref().map(|s| record_to_data(&Record::Session(s.clone()))),
        })
    }
}

/// Change one Session's status when the guard still holds; otherwise keep it as it is,
/// which writes nothing.
fn save_session_status(
    store: &impl RecordStore,
    host: &str,
    session_id: &str,
    next_status: Status,
    allowed: impl Fn(Status) -> bool,
    expected_revision: Option<u64>,
) -> Result<(Session, Option<LifecycleSave>)> {
    let address = RecordAddress::new(host, RecordKind::Session, session_id)?;
    let saved = store.save(
        &address,
        |_, before| -> Result<RecordChange<StatusChange>> {
            let Some(before) = before else {
                return Err(ValidationError::new("Session is missing").into());
            };
            let Record::Session(mut session) = before.record.clone() else {
                return Err(ValidationError::new("lifecycle target must be a Session").into());
            };
            let current_status = session.status;
            let revision_ok = expected_revision.is_none_or(|rev| rev == session.revision);
            let admitted = revision_ok && allowed(current_status);
            if admitted {
                session.status = next_status;
            }
            let outcome = if admitted && current_status != next_status {
                DomainOutcome::Changes
            } else {
                DomainOutcome::NoChange
            };
            Ok(RecordChange {
                record: Record::Session(session),
                outcome: StatusChange {
                    outcome,
                    status: next_status,
                },
            })
        },
    )?;
    let Record::Session(session) = saved.current.record.clone() else {
        return Err(ValidationError::new("lifecycle target must be a Session").into());
    };
    Ok((session, saved.written.then_some(saved)))
}

fn transition_session_status(
    opened: &OpenedRecords,
    harness: Harness,
    native_id: &str,
    next_status: Status,
    allowed: impl Fn(Status) -> bool + Copy,
) -> Result<LifecycleOutcome> {
    let host = opened.host().as_str();
    let id = Session::id_for(host, harness.as_ref(), native_id);
    let addr = RecordAddress::new(host, RecordKind::Session, &id)?;
    let Some(read) = opened.records().with_reader(|r| r.read(&addr))? else {
        return Ok(LifecycleOutcome::unchanged(None));
    };
    let Record::Session(existing) = read.record else {
        return Ok(LifecycleOutcome::unchanged(None));
    };
    if !allowed(existing.status) || existing.status == next_status {
        return Ok(LifecycleOutcome::unchanged(Some(existing)));
    }
    let (session, session_save) = save_session_status(
        opened.records(),
        host,
        &existing.id,
        next_status,
        allowed,
        None,
    )?;
    let mut saves = Vec::new();
    let session_changed = if let Some(save) = session_save {
        saves.push(save);
        true
    } else {
        false
    };
    Ok(LifecycleOutcome {
        session: Some(session),
        session_changed,
        saves,
    })
}

/// Reactivate a parked or closed Session. Reads and saves only the addressed Session record.
pub fn resume_session(
    opened: &OpenedRecords,
    harness: Harness,
    native_id: &str,
) -> Result<LifecycleOutcome> {
    transition_session_status(opened, harness, native_id, Status::Active, |status| {
        matches!(status, Status::Parked | Status::Closed)
    })
}

/// Park an active Session when its harness conversation shuts down.
/// Reads and saves only the addressed Session record (zero host scans).
pub fn park_session(
    opened: &OpenedRecords,
    harness: Harness,
    native_id: &str,
) -> Result<LifecycleOutcome> {
    transition_session_status(opened, harness, native_id, Status::Parked, |status| {
        status == Status::Active
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DecayPlan {
    pub(crate) sessions: Vec<(String, u64)>, // (session_id, revision)
}

pub(crate) fn plan_decay(
    opened: &OpenedRecords,
    decay_days: u32,
    now_ms: u64,
) -> Result<DecayPlan> {
    let Some(cutoff_ms) = decay_cutoff_ms(decay_days, now_ms) else {
        return Ok(DecayPlan::default());
    };
    let mut plan = DecayPlan::default();
    for read in opened.host_sessions()?.complete()? {
        let Record::Session(session) = read.record else {
            continue;
        };
        if session.status.is_open() && is_stale_ms(session.updated_at_ms, Some(cutoff_ms)) {
            plan.sessions.push((session.id, session.revision));
        }
    }
    Ok(plan)
}

pub(crate) fn apply_decay_plan(opened: &OpenedRecords, plan: &DecayPlan) -> DecaySummary {
    let host = opened.host().as_str();
    let mut summary = DecaySummary::default();

    for (session_id, revision) in &plan.sessions {
        match save_session_status(
            opened.records(),
            host,
            session_id,
            Status::Closed,
            Status::is_open,
            Some(*revision),
        ) {
            Ok((_, Some(_))) => summary.closed_sessions += 1,
            Ok((_, None)) => {}
            Err(error) => {
                summary
                    .errors
                    .push(format!("session {session_id}: {error}"));
            }
        }
    }

    summary
}

/// Transition non-done, non-closed Sessions untouched past `decay_days` to `closed`,
/// comparing each Session's observed revision under its write lock so concurrent updates
/// are never overwritten by stale decay observations.
pub fn decay_host_records(
    opened: &OpenedRecords,
    decay_days: u32,
    now_ms: u64,
) -> Result<DecaySummary> {
    let plan = plan_decay(opened, decay_days, now_ms)?;
    Ok(apply_decay_plan(opened, &plan))
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{Arc, Barrier, Mutex},
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use tempfile::tempdir;

    use super::*;
    use crate::{
        application::{
            StreamFilter, create_record, initialize_records, list_streams, move_session_observed,
            open_records_observed, save_session_edit,
        },
        config::{Config, Strategy},
        file_store::records::RecordIoEvent,
        model::{Stamps, Stream, TitleDefaults},
        operations::{
            membership::MoveIntent, patches::SessionPatch, project_stream_status, stream_status,
        },
        storage::{
            StorageError,
            records::{RecordRead, RecordReader},
        },
    };

    fn now_ms() -> u64 {
        u64::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock after epoch")
                .as_millis(),
        )
        .unwrap_or(0)
    }

    fn setup_opened() -> (tempfile::TempDir, Config, OpenedRecords) {
        let dir = tempdir().unwrap();
        let env = BTreeMap::from([
            ("HOME".to_string(), dir.path().display().to_string()),
            (
                "XDG_CONFIG_HOME".to_string(),
                dir.path().join("cfg").display().to_string(),
            ),
            (
                "XDG_DATA_HOME".to_string(),
                dir.path().join("data").display().to_string(),
            ),
        ]);
        let config = Config::new(&env, dir.path()).unwrap();
        let opened = initialize_records(&config, Strategy::None, None, "main").unwrap();
        (dir, config, opened)
    }

    fn session_with_status(
        host: &str,
        stream_id: &str,
        native_id: &str,
        status: Status,
    ) -> Session {
        let mut s = Session::new(
            Session::id_for(host, "pi", native_id),
            host,
            stream_id,
            "pi",
            native_id,
            native_id,
        )
        .unwrap();
        s.status = status;
        s
    }

    fn read_session_status(opened: &OpenedRecords, native_id: &str) -> Status {
        let host = opened.host().as_str();
        let id = Session::id_for(host, "pi", native_id);
        let addr = RecordAddress::new(host, RecordKind::Session, &id).unwrap();
        match opened
            .records()
            .with_reader(|r| r.read(&addr))
            .unwrap()
            .unwrap()
            .record
        {
            Record::Session(s) => s.status,
            _ => panic!("expected session"),
        }
    }

    fn derived_stream_status(opened: &OpenedRecords, stream_id: &str) -> Status {
        let sessions: Vec<Session> = opened
            .host_sessions()
            .unwrap()
            .complete()
            .unwrap()
            .into_iter()
            .filter_map(|read| match read.record {
                Record::Session(s) if s.stream_id == stream_id => Some(s),
                _ => None,
            })
            .collect();
        let refs: Vec<&Session> = sessions.iter().collect();
        stream_status(&refs)
    }

    /// In-memory fake RecordStore with synchronized saves for deterministic concurrency tests.
    struct FakeStore {
        records: Mutex<BTreeMap<RecordAddress, Record>>,
        clock: Mutex<u64>,
    }

    impl FakeStore {
        fn new() -> Self {
            Self {
                records: Mutex::new(BTreeMap::new()),
                clock: Mutex::new(1_000),
            }
        }

        fn stream_status(&self, stream_id: &str) -> Status {
            let guard = self.records.lock().unwrap();
            let members: Vec<&Session> = guard
                .values()
                .filter_map(|record| match record {
                    Record::Session(session) if session.stream_id == stream_id => Some(session),
                    _ => None,
                })
                .collect();
            stream_status(&members)
        }
    }

    struct FakeReader<'a>(&'a BTreeMap<RecordAddress, Record>);

    impl RecordReader for FakeReader<'_> {
        fn read(
            &self,
            address: &RecordAddress,
        ) -> std::result::Result<Option<RecordRead>, StorageError> {
            Ok(self
                .0
                .get(address)
                .cloned()
                .map(|record| RecordRead { record }))
        }
    }

    impl RecordStore for FakeStore {
        fn with_reader<T, E: From<StorageError>>(
            &self,
            read: impl FnOnce(&dyn RecordReader) -> std::result::Result<T, E>,
        ) -> std::result::Result<T, E> {
            let guard = self.records.lock().unwrap();
            read(&FakeReader(&guard))
        }

        fn save<T, E: From<StorageError>>(
            &self,
            target: &RecordAddress,
            apply: impl FnOnce(
                &dyn RecordReader,
                Option<&RecordRead>,
            ) -> std::result::Result<RecordChange<T>, E>,
        ) -> std::result::Result<RecordSave<T>, E> {
            let mut guard = self.records.lock().unwrap();
            let previous = guard
                .get(target)
                .cloned()
                .map(|record| RecordRead { record });
            let RecordChange {
                mut record,
                outcome,
            } = apply(&FakeReader(&guard), previous.as_ref())?;
            let stamps = previous
                .as_ref()
                .and_then(|read| read.record.stamps())
                .unwrap_or_default();
            record.set_stamps(stamps);
            if let Some(current) = previous
                && current.record == record
            {
                return Ok(RecordSave {
                    current,
                    outcome,
                    written: false,
                });
            }
            let mut clock = self.clock.lock().unwrap();
            *clock += 1;
            record.set_stamps(Stamps {
                revision: stamps.revision + 1,
                updated_at_ms: *clock,
            });
            guard.insert(target.clone(), record.clone());
            Ok(RecordSave {
                current: RecordRead { record },
                outcome,
                written: true,
            })
        }
    }

    #[test]
    fn stream_status_follows_precedence_order_and_empty_defaults_to_active() {
        let host = "01234567-89ab-4def-8123-456789abcdef";
        let active = session_with_status(host, "s", "a", Status::Active);
        let blocked = session_with_status(host, "s", "b", Status::Blocked);
        let waiting = session_with_status(host, "s", "w", Status::Waiting);
        let parked = session_with_status(host, "s", "p", Status::Parked);
        let done = session_with_status(host, "s", "d", Status::Done);
        let closed = session_with_status(host, "s", "c", Status::Closed);

        assert_eq!(stream_status(&[]), Status::Active);
        assert_eq!(
            stream_status(&[&closed, &done, &parked, &waiting, &blocked, &active]),
            Status::Active
        );
        assert_eq!(
            stream_status(&[&closed, &done, &parked, &waiting, &blocked]),
            Status::Blocked
        );
        assert_eq!(
            stream_status(&[&closed, &done, &parked, &waiting]),
            Status::Waiting
        );
        assert_eq!(stream_status(&[&closed, &done, &parked]), Status::Parked);
        assert_eq!(stream_status(&[&closed, &done]), Status::Done);
        assert_eq!(stream_status(&[&done]), Status::Done);
        assert_eq!(stream_status(&[&closed]), Status::Closed);

        // Read-time age decay never turns a Done Stream into Closed.
        assert_eq!(
            project_stream_status(&[&done], 100, Some(1_000)),
            Status::Done
        );
        assert_eq!(
            project_stream_status(&[&parked], 100, Some(1_000)),
            Status::Closed
        );
        assert_eq!(project_stream_status(&[], 100, Some(1_000)), Status::Closed);
    }

    #[test]
    fn park_and_resume_skip_done_sessions_and_derive_stream_status() {
        let (_dir, _config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();
        let stream = Stream::new("work", "Work", None, &host).unwrap();
        create_record(opened.records(), &Record::Stream(stream)).unwrap();

        let done = session_with_status(&host, "work", "n-done", Status::Done);
        let parked = session_with_status(&host, "work", "n-parked", Status::Parked);
        create_record(opened.records(), &Record::Session(done)).unwrap();
        create_record(opened.records(), &Record::Session(parked)).unwrap();
        assert_eq!(derived_stream_status(&opened, "work"), Status::Parked);

        // No-op park and resume on Done Session leave it Done.
        let noop_park = park_session(&opened, Harness::Pi, "n-done").unwrap();
        assert!(!noop_park.session_changed);
        assert!(noop_park.saves.is_empty());

        let noop_resume = resume_session(&opened, Harness::Pi, "n-done").unwrap();
        assert!(!noop_resume.session_changed);
        assert!(noop_resume.saves.is_empty());

        // Resuming the Parked Session moves derived Stream status to Active.
        let resumed = resume_session(&opened, Harness::Pi, "n-parked").unwrap();
        assert!(resumed.session_changed);
        assert_eq!(resumed.saves.len(), 1);
        assert_eq!(read_session_status(&opened, "n-parked"), Status::Active);
        assert_eq!(derived_stream_status(&opened, "work"), Status::Active);

        // Parking it moves derived Stream status back to Parked.
        let parked_again = park_session(&opened, Harness::Pi, "n-parked").unwrap();
        assert!(parked_again.session_changed);
        assert_eq!(parked_again.saves.len(), 1);
        assert_eq!(read_session_status(&opened, "n-parked"), Status::Parked);
        assert_eq!(derived_stream_status(&opened, "work"), Status::Parked);
    }

    #[test]
    fn park_and_resume_do_not_scan_unrelated_sessions() {
        let mut park_counts = Vec::new();
        let mut resume_counts = Vec::new();
        for unrelated in [0, 64] {
            let (_dir, config, opened) = setup_opened();
            let host = opened.host().as_str().to_string();
            let stream = Stream::new("work", "Work", None, &host).unwrap();
            create_record(opened.records(), &Record::Stream(stream)).unwrap();
            let target = session_with_status(&host, "work", "target", Status::Active);
            create_record(opened.records(), &Record::Session(target)).unwrap();
            for i in 0..unrelated {
                let extra =
                    session_with_status(&host, "work", &format!("extra-{i}"), Status::Parked);
                create_record(opened.records(), &Record::Session(extra)).unwrap();
            }

            let events = Arc::new(Mutex::new(Vec::<RecordIoEvent>::new()));
            let sink = events.clone();
            let observed =
                open_records_observed(&config, move |ev| sink.lock().unwrap().push(ev)).unwrap();
            events.lock().unwrap().clear();

            let parked = park_session(&observed, Harness::Pi, "target").unwrap();
            assert!(parked.session_changed);
            park_counts.push(events.lock().unwrap().len());

            events.lock().unwrap().clear();
            let resumed = resume_session(&observed, Harness::Pi, "target").unwrap();
            assert!(resumed.session_changed);
            resume_counts.push(events.lock().unwrap().len());
        }
        assert_eq!(park_counts[0], park_counts[1]);
        assert_eq!(resume_counts[0], resume_counts[1]);
    }

    #[test]
    fn decay_host_records_skips_concurrently_modified_sessions_and_preserves_done() {
        let (_dir, _config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();

        let stale_stream = Stream::new("stale", "Stale", None, &host).unwrap();
        let done_stream = Stream::new("finished", "Finished", None, &host).unwrap();
        create_record(opened.records(), &Record::Stream(stale_stream)).unwrap();
        create_record(opened.records(), &Record::Stream(done_stream)).unwrap();

        let s_stale = session_with_status(&host, "stale", "n-stale", Status::Parked);
        let s_raced = session_with_status(&host, "stale", "n-raced", Status::Active);
        let s_done = session_with_status(&host, "finished", "n-done", Status::Done);
        create_record(opened.records(), &Record::Session(s_stale)).unwrap();
        let Record::Session(raced) =
            create_record(opened.records(), &Record::Session(s_raced.clone()))
                .unwrap()
                .current
                .record
        else {
            panic!("created a Session")
        };
        create_record(opened.records(), &Record::Session(s_done)).unwrap();

        let base = now_ms();
        // Disabled (0 days) or within threshold (5 days) plans nothing.
        assert_eq!(
            decay_host_records(&opened, 0, base + 30 * 86_400_000).unwrap(),
            DecaySummary::default()
        );
        assert_eq!(
            decay_host_records(&opened, 14, base + 5 * 86_400_000).unwrap(),
            DecaySummary::default()
        );

        // Capture a stale decay plan past 14 days, then concurrently edit n-raced before applying!
        let plan = plan_decay(&opened, 14, base + 15 * 86_400_000).unwrap();
        let patch = SessionPatch {
            state: Some("Fresh concurrent update".into()),
            ..SessionPatch::default()
        };
        save_session_edit(opened.records(), &raced, patch, TitleDefaults::default()).unwrap();
        let raced_addr = RecordAddress::new(&host, RecordKind::Session, &s_raced.id).unwrap();
        let raced_rev_before = opened
            .records()
            .with_reader(|r| r.read(&raced_addr))
            .unwrap()
            .unwrap()
            .record
            .stamps();

        let summary = apply_decay_plan(&opened, &plan);
        assert_eq!(
            summary,
            DecaySummary {
                closed_sessions: 1, // n-stale closed, n-raced skipped due to revision mismatch!
                errors: Vec::new(),
            }
        );
        assert_eq!(read_session_status(&opened, "n-stale"), Status::Closed);
        assert_eq!(read_session_status(&opened, "n-raced"), Status::Active);
        assert_eq!(derived_stream_status(&opened, "stale"), Status::Active);
        // Guard rejection must NOT rewrite n-raced or bump its revision!
        let raced_rev_after = opened
            .records()
            .with_reader(|r| r.read(&raced_addr))
            .unwrap()
            .unwrap()
            .record
            .stamps();
        assert_eq!(raced_rev_before, raced_rev_after);
        assert_eq!(read_session_status(&opened, "n-done"), Status::Done);
        assert_eq!(derived_stream_status(&opened, "finished"), Status::Done);

        // Once n-raced also decays, stale's derived Stream status becomes Closed.
        let summary = decay_host_records(&opened, 14, base + 30 * 86_400_000).unwrap();
        assert_eq!(summary.closed_sessions, 1);
        assert_eq!(derived_stream_status(&opened, "stale"), Status::Closed);
        assert_eq!(derived_stream_status(&opened, "finished"), Status::Done);
    }

    #[test]
    fn park_and_resume_tolerate_concurrent_non_status_edits() {
        let (_dir, config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();
        let stream = Stream::new("work", "Work", None, &host).unwrap();
        create_record(opened.records(), &Record::Stream(stream)).unwrap();

        let active = session_with_status(&host, "work", "n-active", Status::Active);
        let Record::Session(active) = create_record(opened.records(), &Record::Session(active))
            .unwrap()
            .current
            .record
        else {
            panic!("created a Session")
        };

        // Already-active session performs zero writes on resume.
        let events = Arc::new(Mutex::new(Vec::<RecordIoEvent>::new()));
        let sink = events.clone();
        let observed =
            open_records_observed(&config, move |ev| sink.lock().unwrap().push(ev)).unwrap();
        events.lock().unwrap().clear();
        let noop = resume_session(&observed, Harness::Pi, "n-active").unwrap();
        assert!(!noop.session_changed);
        assert!(noop.saves.is_empty());
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .all(|ev| !matches!(ev, RecordIoEvent::Written(_, _)))
        );

        // Concurrent non-status edit does not block park_session.
        let patch = SessionPatch {
            recap: Some("Concurrent report".into()),
            ..SessionPatch::default()
        };
        save_session_edit(opened.records(), &active, patch, TitleDefaults::default()).unwrap();
        let parked = park_session(&opened, Harness::Pi, "n-active").unwrap();
        assert!(parked.session_changed);
        assert_eq!(read_session_status(&opened, "n-active"), Status::Parked);
        assert_eq!(derived_stream_status(&opened, "work"), Status::Parked);
    }

    #[test]
    fn concurrent_session_saves_and_moves_preserve_derived_stream_status() {
        let store = Arc::new(FakeStore::new());
        let host = "01234567-89ab-4def-8123-456789abcdef";
        let s1 = Stream::new("s1", "First", None, host).unwrap();
        let s2 = Stream::new("s2", "Second", None, host).unwrap();
        create_record(store.as_ref(), &Record::Stream(s1)).unwrap();
        create_record(store.as_ref(), &Record::Stream(s2)).unwrap();

        let Record::Session(a) = create_record(
            store.as_ref(),
            &Record::Session(session_with_status(host, "s1", "a", Status::Active)),
        )
        .unwrap()
        .current
        .record
        else {
            panic!()
        };
        let Record::Session(b) = create_record(
            store.as_ref(),
            &Record::Session(session_with_status(host, "s1", "b", Status::Active)),
        )
        .unwrap()
        .current
        .record
        else {
            panic!()
        };
        assert_eq!(store.stream_status("s1"), Status::Active);

        // Two Sessions in the same Stream save Done concurrently across threads.
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = [a.clone(), b.clone()]
            .into_iter()
            .map(|session| {
                let store = Arc::clone(&store);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    save_session_edit(
                        store.as_ref(),
                        &session,
                        SessionPatch {
                            status: Some(Status::Done),
                            ..Default::default()
                        },
                        TitleDefaults::default(),
                    )
                    .unwrap()
                })
            })
            .collect();
        for handle in handles {
            assert!(handle.join().unwrap().written);
        }
        assert_eq!(store.stream_status("s1"), Status::Done);

        // Adding a new Active Session to a Done Stream immediately makes it Active again.
        let c = session_with_status(host, "s1", "c", Status::Active);
        create_record(store.as_ref(), &Record::Session(c)).unwrap();
        assert_eq!(store.stream_status("s1"), Status::Active);

        // Moving the only open Session out of s1 into s2 makes s1 Done and s2 Active.
        let mv = MoveIntent::new(host, "pi", "c", "s2").unwrap();
        move_session_observed(store.as_ref(), &mv).unwrap();
        assert_eq!(store.stream_status("s1"), Status::Done);
        assert_eq!(store.stream_status("s2"), Status::Active);
    }

    #[test]
    fn list_streams_reflects_done_when_all_sessions_are_done() {
        let (_dir, _config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();
        let stream = Stream::new("work", "Work", None, &host).unwrap();
        create_record(opened.records(), &Record::Stream(stream)).unwrap();

        let a = session_with_status(&host, "work", "a", Status::Done);
        let b = session_with_status(&host, "work", "b", Status::Closed);
        create_record(opened.records(), &Record::Session(a)).unwrap();
        create_record(opened.records(), &Record::Session(b)).unwrap();

        let listed = list_streams(&opened, StreamFilter::default(), None).unwrap();
        assert_eq!(listed.streams.len(), 1);
        assert_eq!(listed.streams[0].status, Status::Done);
    }
}
