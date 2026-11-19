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
            let Record::Session(mut session) = before.clone() else {
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
    let Record::Session(session) = saved.current.clone() else {
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
    let Record::Session(existing) = read else {
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
        let Record::Session(session) = read else {
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
        sync::{Arc, Mutex},
        time::{SystemTime, UNIX_EPOCH},
    };

    use tempfile::tempdir;

    use super::*;
    use crate::{
        agent::{RecordUpdate, record_update::CapturedEdit},
        application::{
            StreamFilter, create_record, initialize_records, list_streams, move_session_observed,
            open_records_observed, save_record_update, save_session_edit,
        },
        config::{Config, Strategy},
        file_store::records::{RecordIoEvent, relative_record_path},
        model::{Stream, TitleDefaults},
        operations::{
            Actor, derived_stream_status as derive_status,
            edits::SessionEdit,
            membership::MoveIntent,
            patches::{Channel, SessionPatch},
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
        {
            Record::Session(s) => s.status,
            _ => panic!("expected session"),
        }
    }

    fn derived_stream_status(opened: &OpenedRecords, stream_id: &str) -> Status {
        let host = opened.host().as_str();
        let stream = opened
            .records()
            .with_reader(|r| r.read_stream(host, stream_id))
            .unwrap()
            .unwrap();
        let sessions: Vec<Session> = opened
            .host_sessions()
            .unwrap()
            .complete()
            .unwrap()
            .into_iter()
            .filter_map(|read| match read {
                Record::Session(s) if s.stream_id == stream_id => Some(s),
                _ => None,
            })
            .collect();
        let refs: Vec<&Session> = sessions.iter().collect();
        derive_status(&stream, &refs, None)
    }

    #[test]
    fn derived_stream_status_follows_precedence_and_matches_tidy_decay() {
        let host = "01234567-89ab-4def-8123-456789abcdef";
        let mut stream = Stream::new("s", "Stream", None, host).unwrap();
        stream.updated_at_ms = 100;
        let with_time = |native_id, status| {
            let mut s = session_with_status(host, "s", native_id, status);
            s.updated_at_ms = 100;
            s
        };
        let active = with_time("a", Status::Active);
        let blocked = with_time("b", Status::Blocked);
        let waiting = with_time("w", Status::Waiting);
        let parked = with_time("p", Status::Parked);
        let done = with_time("d", Status::Done);
        let closed = with_time("c", Status::Closed);

        assert_eq!(derive_status(&stream, &[], None), Status::Active);
        assert_eq!(
            derive_status(
                &stream,
                &[&closed, &done, &parked, &waiting, &blocked, &active],
                None
            ),
            Status::Active
        );
        assert_eq!(
            derive_status(
                &stream,
                &[&closed, &done, &parked, &waiting, &blocked],
                None
            ),
            Status::Blocked
        );
        assert_eq!(
            derive_status(&stream, &[&closed, &done, &parked, &waiting], None),
            Status::Waiting
        );
        assert_eq!(
            derive_status(&stream, &[&closed, &done, &parked], None),
            Status::Parked
        );
        assert_eq!(
            derive_status(&stream, &[&closed, &done], None),
            Status::Done
        );
        assert_eq!(derive_status(&stream, &[&done], None), Status::Done);
        assert_eq!(derive_status(&stream, &[&closed], None), Status::Closed);

        // Read-time age decay matches what `streams tidy` persists:
        // stale open Sessions close, so a Stream with a Done Session becomes Done, else Closed.
        assert_eq!(derive_status(&stream, &[&done], Some(1_000)), Status::Done);
        assert_eq!(
            derive_status(&stream, &[&parked, &done], Some(1_000)),
            Status::Done
        );
        assert_eq!(
            derive_status(&stream, &[&parked], Some(1_000)),
            Status::Closed
        );
        assert_eq!(derive_status(&stream, &[], Some(1_000)), Status::Closed);
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
    fn list_streams_matches_before_and_after_decay_host_records() {
        let (_dir, _config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();

        for (id, title) in [
            ("mixed", "Mixed parked and done"),
            ("only-parked", "Only parked"),
            ("only-done", "Only done"),
        ] {
            let stream = Stream::new(id, title, None, &host).unwrap();
            create_record(opened.records(), &Record::Stream(stream)).unwrap();
        }
        create_record(
            opened.records(),
            &Record::Session(session_with_status(
                &host,
                "mixed",
                "m-parked",
                Status::Parked,
            )),
        )
        .unwrap();
        create_record(
            opened.records(),
            &Record::Session(session_with_status(&host, "mixed", "m-done", Status::Done)),
        )
        .unwrap();
        create_record(
            opened.records(),
            &Record::Session(session_with_status(
                &host,
                "only-parked",
                "p-1",
                Status::Parked,
            )),
        )
        .unwrap();
        create_record(
            opened.records(),
            &Record::Session(session_with_status(&host, "only-done", "d-1", Status::Done)),
        )
        .unwrap();

        let future = now_ms() + 30 * 86_400_000;
        let cutoff = decay_cutoff_ms(14, future);
        let statuses = |cutoff_ms| -> BTreeMap<String, Status> {
            list_streams(
                &opened,
                StreamFilter {
                    all: true,
                    ..StreamFilter::default()
                },
                cutoff_ms,
            )
            .unwrap()
            .streams
            .into_iter()
            .map(|entry| (entry.stream.id, entry.status))
            .collect()
        };

        let before = statuses(cutoff);
        assert_eq!(before["mixed"], Status::Done);
        assert_eq!(before["only-parked"], Status::Closed);
        assert_eq!(before["only-done"], Status::Done);

        let summary = decay_host_records(&opened, 14, future).unwrap();
        assert_eq!(summary.closed_sessions, 2);

        let after_with_cutoff = statuses(cutoff);
        let after_without_cutoff = statuses(None);
        assert_eq!(before, after_with_cutoff);
        assert_eq!(before, after_without_cutoff);
    }

    #[test]
    fn session_status_writes_touch_only_their_session_without_scans_or_stream_writes() {
        let mut edit_counts = Vec::new();
        let mut report_counts = Vec::new();
        for unrelated in [0, 64] {
            let (_dir, config, opened) = setup_opened();
            let host = opened.host().as_str().to_string();
            let stream = Stream::new("work", "Work", None, &host).unwrap();
            let other = Stream::new("other", "Other", None, &host).unwrap();
            create_record(opened.records(), &Record::Stream(stream)).unwrap();
            create_record(opened.records(), &Record::Stream(other)).unwrap();
            let Record::Session(target) = create_record(
                opened.records(),
                &Record::Session(session_with_status(&host, "work", "target", Status::Active)),
            )
            .unwrap()
            .current
            else {
                panic!()
            };
            for i in 0..unrelated {
                create_record(
                    opened.records(),
                    &Record::Session(session_with_status(
                        &host,
                        "work",
                        &format!("extra-{i}"),
                        Status::Done,
                    )),
                )
                .unwrap();
            }

            let target_path = opened
                .root()
                .join(relative_record_path(&RecordAddress::session(&target)).unwrap());

            let events = Arc::new(Mutex::new(Vec::<RecordIoEvent>::new()));
            let sink = events.clone();
            let observed =
                open_records_observed(&config, move |ev| sink.lock().unwrap().push(ev)).unwrap();

            // 1. Explicit user edit via save_session_edit (Active -> Done).
            events.lock().unwrap().clear();
            let saved = save_session_edit(
                observed.records(),
                &target,
                SessionPatch {
                    status: Some(Status::Done),
                    ..Default::default()
                },
                TitleDefaults::default(),
            )
            .unwrap();
            assert!(saved.written);
            let trace = events.lock().unwrap().clone();
            assert!(!trace.iter().any(|ev| matches!(ev, RecordIoEvent::List(_))));
            let written: Vec<_> = trace
                .iter()
                .filter_map(|ev| ev.written().map(|(p, _)| p.to_path_buf()))
                .collect();
            assert_eq!(written, vec![target_path.clone()]);
            edit_counts.push(trace.len());
            assert_eq!(derived_stream_status(&observed, "work"), Status::Done);

            // 2. Automatic report via save_record_update (Done -> Active).
            let Record::Session(done_target) = saved.current else {
                panic!()
            };
            let edit = SessionEdit::new(
                &done_target,
                SessionPatch {
                    status: Some(Status::Active),
                    ..Default::default()
                },
                Actor::Automatic,
                Channel::Agent,
                TitleDefaults::default().session,
            )
            .unwrap();
            let update = RecordUpdate {
                target: RecordAddress::session(&done_target),
                edit: CapturedEdit::Session(edit),
                title_defaults: TitleDefaults::default(),
            };
            events.lock().unwrap().clear();
            let reopened = save_record_update(observed.records(), &update).unwrap();
            assert!(reopened.written);
            let trace = events.lock().unwrap().clone();
            assert!(!trace.iter().any(|ev| matches!(ev, RecordIoEvent::List(_))));
            let written: Vec<_> = trace
                .iter()
                .filter_map(|ev| ev.written().map(|(p, _)| p.to_path_buf()))
                .collect();
            assert_eq!(written, vec![target_path]);
            report_counts.push(trace.len());
            assert_eq!(derived_stream_status(&observed, "work"), Status::Active);

            // 3. Moving the active Session out of `work` into `other` updates both derived views.
            let mv = MoveIntent::new(&host, "pi", "target", "other").unwrap();
            move_session_observed(observed.records(), &mv).unwrap();
            assert_eq!(derived_stream_status(&observed, "other"), Status::Active);
            if unrelated > 0 {
                assert_eq!(derived_stream_status(&observed, "work"), Status::Done);
            }
        }
        assert_eq!(edit_counts[0], edit_counts[1]);
        assert_eq!(report_counts[0], report_counts[1]);
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
