//! Host-local Session/Stream lifecycle transitions, guarded closures, and age decay.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{OpenedRecords, Result};
use crate::{
    model::{Record, RecordKind, Session, Status, ValidationError},
    operations::DomainOutcome,
    runtime::conversations::Harness,
    storage::records::{RecordAddress, RecordChange, RecordRequest, RecordSave, RecordStore},
};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DecaySummary {
    pub closed_sessions: usize,
    pub closed_streams: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleOutcome {
    pub session: Option<Session>,
    pub session_changed: bool,
    pub stream_changed: bool,
    pub saves: Vec<RecordSave>,
    pub completion_error: Option<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CloseChildrenOutcome {
    pub closed_sessions: usize,
    pub saves: Vec<RecordSave>,
    pub completion_error: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
struct SessionGuard<'a> {
    revision: Option<&'a str>,
    stream_id: Option<&'a str>,
}

fn save_session_status(
    store: &impl RecordStore,
    host: &str,
    session_id: &str,
    operation_id: &str,
    next_status: Status,
    allowed_from: &[Status],
    guard: SessionGuard<'_>,
) -> Result<(Option<Session>, Option<RecordSave>)> {
    let address = RecordAddress::new(host, RecordKind::Session, session_id)?;
    let request = RecordRequest::new(
        address,
        operation_id,
        &serde_json::json!({"lifecycle":"session_status","status":next_status}),
    )?;
    let mut changed = false;
    let saved = store.save::<super::ApplicationError>(&request, |_, before| {
        let Some(before) = before else {
            return Err(ValidationError::new("Session is missing").into());
        };
        let Record::Session(mut session) = before.record.clone() else {
            return Err(ValidationError::new("lifecycle target must be a Session").into());
        };
        let revision_ok = guard.revision.is_none_or(|rev| rev == before.revision);
        let stream_ok = guard.stream_id.is_none_or(|sid| sid == session.stream_id);
        let outcome = if revision_ok
            && stream_ok
            && allowed_from.contains(&session.status)
            && session.status != next_status
        {
            session.status = next_status;
            changed = true;
            DomainOutcome::Changes
        } else {
            DomainOutcome::NoChange
        };
        let receipt = serde_json::json!({
            "outcome": outcome,
            "session_id": session.id,
            "status": session.status,
        });
        Ok(RecordChange {
            record: Record::Session(session),
            outcome: receipt.as_object().expect("receipt object").clone(),
        })
    })?;
    let Record::Session(session) = saved.current.record.clone() else {
        return Err(ValidationError::new("lifecycle target must be a Session").into());
    };
    Ok((Some(session), changed.then_some(saved)))
}

fn save_stream_status(
    store: &impl RecordStore,
    host: &str,
    stream_id: &str,
    operation_id: &str,
    next_status: Status,
    allowed_from: &[Status],
    expected_revision: Option<&str>,
) -> Result<Option<RecordSave>> {
    let address = RecordAddress::new(host, RecordKind::Stream, stream_id)?;
    let request = RecordRequest::new(
        address,
        operation_id,
        &serde_json::json!({"lifecycle":"stream_status","status":next_status}),
    )?;
    let mut changed = false;
    let saved = store.save::<super::ApplicationError>(&request, |_, before| {
        let Some(before) = before else {
            return Err(ValidationError::new("Stream is missing").into());
        };
        let Record::Stream(mut stream) = before.record.clone() else {
            return Err(ValidationError::new("lifecycle target must be a Stream").into());
        };
        let revision_ok = expected_revision.is_none_or(|rev| rev == before.revision);
        let outcome =
            if revision_ok && allowed_from.contains(&stream.status) && stream.status != next_status
            {
                stream.status = next_status;
                changed = true;
                DomainOutcome::Changes
            } else {
                DomainOutcome::NoChange
            };
        let receipt = serde_json::json!({
            "outcome": outcome,
            "stream_id": stream.id,
            "status": stream.status,
        });
        Ok(RecordChange {
            record: Record::Stream(stream),
            outcome: receipt.as_object().expect("receipt object").clone(),
        })
    })?;
    Ok(changed.then_some(saved))
}

/// Reopen a `closed` Stream only when an enrolled Session in it is actively resumed/enrolled.
/// Reads and saves only the addressed Stream record (zero host scans).
pub fn reopen_closed_stream(
    opened: &OpenedRecords,
    stream_id: &str,
    operation_id: &str,
) -> Result<Option<RecordSave>> {
    save_stream_status(
        opened.records(),
        opened.host().as_str(),
        stream_id,
        operation_id,
        Status::Active,
        &[Status::Closed],
        None,
    )
}

/// Close every non-done, non-closed Session still belonging to `stream_id`, preserving
/// all accepted saves and surfacing any partial failure without rolling back prior writes.
pub fn close_stream_sessions(
    opened: &OpenedRecords,
    stream_id: &str,
    operation_id_prefix: &str,
) -> CloseChildrenOutcome {
    let host = opened.host().as_str();
    let session_reads = match opened.host_sessions() {
        Ok(reads) => reads,
        Err(error) => {
            return CloseChildrenOutcome {
                closed_sessions: 0,
                saves: Vec::new(),
                completion_error: Some(error.to_string()),
            };
        }
    };
    let mut candidates = Vec::new();
    for read in session_reads {
        let Record::Session(session) = read.record else {
            continue;
        };
        if session.stream_id != stream_id || matches!(session.status, Status::Done | Status::Closed)
        {
            continue;
        }
        candidates.push((session.id, read.revision));
    }
    let mut outcome = CloseChildrenOutcome::default();
    let mut errors = Vec::new();
    for (session_id, revision) in candidates {
        let op = format!("{operation_id_prefix}:{session_id}");
        match save_session_status(
            opened.records(),
            host,
            &session_id,
            &op,
            Status::Closed,
            &[
                Status::Active,
                Status::Waiting,
                Status::Blocked,
                Status::Parked,
            ],
            SessionGuard {
                revision: Some(&revision),
                stream_id: Some(stream_id),
            },
        ) {
            Ok((_, Some(save))) => {
                outcome.closed_sessions += 1;
                outcome.saves.push(save);
            }
            Ok((_, None)) => {}
            Err(error) => errors.push(format!("{session_id}: {error}")),
        }
    }
    if !errors.is_empty() {
        outcome.completion_error = Some(errors.join("; "));
    }
    outcome
}

/// Reactivate a parked or closed Session, reopening a closed Stream only if the Session
/// actually transitioned. Preserves accepted Session writes if reopening the Stream fails.
pub fn resume_session(
    opened: &OpenedRecords,
    harness: Harness,
    native_id: &str,
    operation_id: &str,
) -> Result<LifecycleOutcome> {
    let host = opened.host().as_str();
    let id = Session::id_for(host, harness.as_ref(), native_id);
    let addr = RecordAddress::new(host, RecordKind::Session, &id)?;
    let Some(read) = opened.records().with_reader(|r| r.read(&addr))? else {
        return Ok(LifecycleOutcome {
            session: None,
            session_changed: false,
            stream_changed: false,
            saves: Vec::new(),
            completion_error: None,
        });
    };
    let Record::Session(existing) = read.record else {
        return Ok(LifecycleOutcome {
            session: None,
            session_changed: false,
            stream_changed: false,
            saves: Vec::new(),
            completion_error: None,
        });
    };
    if !matches!(existing.status, Status::Parked | Status::Closed) {
        return Ok(LifecycleOutcome {
            session: Some(existing),
            session_changed: false,
            stream_changed: false,
            saves: Vec::new(),
            completion_error: None,
        });
    }
    let (session, session_save) = save_session_status(
        opened.records(),
        host,
        &existing.id,
        &format!("{operation_id}:session"),
        Status::Active,
        &[Status::Parked, Status::Closed],
        SessionGuard {
            revision: Some(&read.revision),
            stream_id: None,
        },
    )?;
    let mut saves = Vec::new();
    let session_changed = if let Some(save) = session_save {
        saves.push(save);
        true
    } else {
        false
    };
    let mut stream_changed = false;
    let mut completion_error = None;
    if session_changed && let Some(session_ref) = &session {
        match reopen_closed_stream(
            opened,
            &session_ref.stream_id,
            &format!("{operation_id}:stream"),
        ) {
            Ok(Some(stream_save)) => {
                saves.push(stream_save);
                stream_changed = true;
            }
            Ok(None) => {}
            Err(error) => {
                completion_error = Some(format!("reopening Stream failed: {error}"));
            }
        }
    }
    Ok(LifecycleOutcome {
        session,
        session_changed,
        stream_changed,
        saves,
        completion_error,
    })
}

/// Park an active Session when its harness conversation shuts down.
/// Reads and saves only the addressed Session record (zero host scans, never reopens a Stream).
pub fn park_session(
    opened: &OpenedRecords,
    harness: Harness,
    native_id: &str,
    operation_id: &str,
) -> Result<LifecycleOutcome> {
    let host = opened.host().as_str();
    let id = Session::id_for(host, harness.as_ref(), native_id);
    let addr = RecordAddress::new(host, RecordKind::Session, &id)?;
    let Some(read) = opened.records().with_reader(|r| r.read(&addr))? else {
        return Ok(LifecycleOutcome {
            session: None,
            session_changed: false,
            stream_changed: false,
            saves: Vec::new(),
            completion_error: None,
        });
    };
    let Record::Session(existing) = read.record else {
        return Ok(LifecycleOutcome {
            session: None,
            session_changed: false,
            stream_changed: false,
            saves: Vec::new(),
            completion_error: None,
        });
    };
    if existing.status != Status::Active {
        return Ok(LifecycleOutcome {
            session: Some(existing),
            session_changed: false,
            stream_changed: false,
            saves: Vec::new(),
            completion_error: None,
        });
    }
    let (session, session_save) = save_session_status(
        opened.records(),
        host,
        &existing.id,
        &format!("{operation_id}:session"),
        Status::Parked,
        &[Status::Active],
        SessionGuard {
            revision: Some(&read.revision),
            stream_id: None,
        },
    )?;
    let mut saves = Vec::new();
    let session_changed = if let Some(save) = session_save {
        saves.push(save);
        true
    } else {
        false
    };
    Ok(LifecycleOutcome {
        session,
        session_changed,
        stream_changed: false,
        saves,
        completion_error: None,
    })
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DecayPlan {
    pub(crate) sessions: Vec<(String, String, String)>, // (session_id, stream_id, revision)
    pub(crate) streams: Vec<(String, String)>,          // (stream_id, revision)
}

pub(crate) fn plan_decay(
    opened: &OpenedRecords,
    decay_days: u32,
    now_ms: u64,
) -> Result<DecayPlan> {
    if decay_days == 0 {
        return Ok(DecayPlan::default());
    }
    let threshold_ms = u64::from(decay_days).saturating_mul(86_400_000);
    let cutoff_ms = now_ms.saturating_sub(threshold_ms);
    if cutoff_ms == 0 {
        return Ok(DecayPlan::default());
    }

    let host = opened.host().as_str();
    let session_reads = opened.host_sessions()?;
    let stream_reads = opened.host_streams()?;

    let mut sessions = Vec::with_capacity(session_reads.len());
    let mut streams = Vec::with_capacity(stream_reads.len());
    let mut addresses = Vec::with_capacity(session_reads.len() + stream_reads.len());
    for read in session_reads {
        if let Record::Session(s) = read.record {
            addresses.push(RecordAddress::new(host, RecordKind::Session, &s.id)?);
            sessions.push((s, read.revision));
        }
    }
    for read in stream_reads {
        if let Record::Stream(s) = read.record {
            addresses.push(RecordAddress::new(host, RecordKind::Stream, &s.id)?);
            streams.push((s, read.revision));
        }
    }
    if addresses.is_empty() {
        return Ok(DecayPlan::default());
    }

    let timestamps = opened.last_updated_ms(&addresses)?;
    let mut latest_session_ms: BTreeMap<String, u64> = BTreeMap::new();
    for (s, _) in &sessions {
        let addr = RecordAddress::new(host, RecordKind::Session, &s.id)?;
        if let Some(&ms) = timestamps.get(&addr) {
            let entry = latest_session_ms.entry(s.stream_id.clone()).or_insert(0);
            *entry = (*entry).max(ms);
        }
    }

    let mut plan = DecayPlan::default();
    let mut decayed_streams = BTreeSet::new();
    for (stream, revision) in &streams {
        let addr = RecordAddress::new(host, RecordKind::Stream, &stream.id)?;
        let stream_ms = timestamps.get(&addr).copied().unwrap_or(0);
        let session_ms = latest_session_ms.get(&stream.id).copied().unwrap_or(0);
        let effective_ms = stream_ms.max(session_ms);
        if !matches!(stream.status, Status::Done | Status::Closed)
            && effective_ms > 0
            && effective_ms < cutoff_ms
        {
            decayed_streams.insert(stream.id.clone());
            plan.streams.push((stream.id.clone(), revision.clone()));
        }
    }

    for (s, revision) in &sessions {
        if matches!(s.status, Status::Done | Status::Closed) {
            continue;
        }
        let addr = RecordAddress::new(host, RecordKind::Session, &s.id)?;
        let ms = timestamps.get(&addr).copied().unwrap_or(0);
        if decayed_streams.contains(&s.stream_id) || (ms > 0 && ms < cutoff_ms) {
            plan.sessions
                .push((s.id.clone(), s.stream_id.clone(), revision.clone()));
        }
    }

    Ok(plan)
}

pub(crate) fn apply_decay_plan(
    opened: &OpenedRecords,
    plan: &DecayPlan,
    operation_id_prefix: &str,
) -> DecaySummary {
    let host = opened.host().as_str();
    let mut summary = DecaySummary::default();

    for (session_id, stream_id, revision) in &plan.sessions {
        let op = format!("{operation_id_prefix}:decay:session:{session_id}");
        match save_session_status(
            opened.records(),
            host,
            session_id,
            &op,
            Status::Closed,
            &[
                Status::Active,
                Status::Waiting,
                Status::Blocked,
                Status::Parked,
            ],
            SessionGuard {
                revision: Some(revision),
                stream_id: Some(stream_id),
            },
        ) {
            Ok((_, Some(_))) => summary.closed_sessions += 1,
            Ok((_, None)) => {}
            Err(error) => summary
                .errors
                .push(format!("session {session_id}: {error}")),
        }
    }

    for (stream_id, revision) in &plan.streams {
        let op = format!("{operation_id_prefix}:decay:stream:{stream_id}");
        match save_stream_status(
            opened.records(),
            host,
            stream_id,
            &op,
            Status::Closed,
            &[
                Status::Active,
                Status::Waiting,
                Status::Blocked,
                Status::Parked,
            ],
            Some(revision),
        ) {
            Ok(Some(_)) => summary.closed_streams += 1,
            Ok(None) => {}
            Err(error) => summary.errors.push(format!("stream {stream_id}: {error}")),
        }
    }

    summary
}

/// Transition non-done, non-closed Sessions and Streams untouched past `decay_days` to `closed`,
/// checking each record's observed revision under its write lock so concurrent updates are never
/// overwritten by stale decay observations.
pub fn decay_host_records(
    opened: &OpenedRecords,
    decay_days: u32,
    now_ms: u64,
    operation_id_prefix: &str,
) -> Result<DecaySummary> {
    let plan = plan_decay(opened, decay_days, now_ms)?;
    Ok(apply_decay_plan(opened, &plan, operation_id_prefix))
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, Mutex},
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde_json::json;
    use tempfile::tempdir;

    use super::*;
    use crate::{
        application::{
            create_record, initialize_records, move_session_observed, open_records_observed,
            save_user_edit,
        },
        config::{Config, Strategy},
        model::{Stream, TitleDefaults},
        operations::{effective_stream_status, membership::MoveIntent},
        yaml_store::records::RecordIoEvent,
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

    fn read_stream_status(opened: &OpenedRecords, stream_id: &str) -> Status {
        let addr =
            RecordAddress::new(opened.host().as_str(), RecordKind::Stream, stream_id).unwrap();
        match opened
            .records()
            .with_reader(|r| r.read(&addr))
            .unwrap()
            .unwrap()
            .record
        {
            Record::Stream(s) => s.status,
            _ => panic!("expected stream"),
        }
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

    #[test]
    fn effective_stream_status_preserves_explicit_states_and_projects_session_activity() {
        let host = "01234567-89ab-4def-8123-456789abcdef";
        let active = session_with_status(host, "s", "a", Status::Active);
        let blocked = session_with_status(host, "s", "b", Status::Blocked);
        let waiting = session_with_status(host, "s", "w", Status::Waiting);
        let parked = session_with_status(host, "s", "p", Status::Parked);
        let done = session_with_status(host, "s", "d", Status::Done);
        let closed = session_with_status(host, "s", "c", Status::Closed);

        for explicit in [
            Status::Done,
            Status::Closed,
            Status::Blocked,
            Status::Waiting,
        ] {
            assert_eq!(
                effective_stream_status(explicit, &[&active, &parked]),
                explicit
            );
        }
        assert_eq!(effective_stream_status(Status::Active, &[]), Status::Active);
        assert_eq!(
            effective_stream_status(
                Status::Active,
                &[&closed, &parked, &waiting, &blocked, &active]
            ),
            Status::Active
        );
        assert_eq!(
            effective_stream_status(Status::Active, &[&closed, &parked, &waiting, &blocked]),
            Status::Blocked
        );
        assert_eq!(
            effective_stream_status(Status::Active, &[&closed, &parked, &waiting]),
            Status::Waiting
        );
        assert_eq!(
            effective_stream_status(Status::Active, &[&closed, &parked, &done]),
            Status::Parked
        );
        assert_eq!(
            effective_stream_status(Status::Active, &[&closed, &done]),
            Status::Parked
        );
    }

    #[test]
    fn park_noop_never_reopens_closed_stream_and_resume_reopens_closed_stream_only_when_session_changes()
     {
        let (_dir, _config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();
        let mut closed_stream = Stream::new("work", "Work", None, &host).unwrap();
        closed_stream.status = Status::Closed;
        create_record(opened.records(), "cs-1", &Record::Stream(closed_stream)).unwrap();

        let done = session_with_status(&host, "work", "n-done", Status::Done);
        let parked = session_with_status(&host, "work", "n-parked", Status::Parked);
        create_record(opened.records(), "cs-2", &Record::Session(done)).unwrap();
        create_record(opened.records(), "cs-3", &Record::Session(parked)).unwrap();

        // No-op park on Done or Parked Session must leave Closed Stream untouched!
        let noop_park = park_session(&opened, Harness::Pi, "n-done", "park-done").unwrap();
        assert!(!noop_park.session_changed);
        assert!(!noop_park.stream_changed);
        assert!(noop_park.saves.is_empty());
        assert_eq!(read_stream_status(&opened, "work"), Status::Closed);

        // No-op resume on Done Session also leaves Closed Stream untouched.
        let noop_resume = resume_session(&opened, Harness::Pi, "n-done", "resume-done").unwrap();
        assert!(!noop_resume.session_changed);
        assert!(!noop_resume.stream_changed);
        assert_eq!(read_stream_status(&opened, "work"), Status::Closed);

        // Explicit resume of a Parked Session reactivates both the Session and its Closed Stream.
        let resumed = resume_session(&opened, Harness::Pi, "n-parked", "resume-parked").unwrap();
        assert!(resumed.session_changed);
        assert!(resumed.stream_changed);
        assert_eq!(resumed.saves.len(), 2);
        assert!(resumed.completion_error.is_none());
        assert_eq!(read_session_status(&opened, "n-parked"), Status::Active);
        assert_eq!(read_stream_status(&opened, "work"), Status::Active);

        // Parking n-parked changes only the Session (Stream remains Active on disk).
        let parked_again = park_session(&opened, Harness::Pi, "n-parked", "park-again").unwrap();
        assert!(parked_again.session_changed);
        assert!(!parked_again.stream_changed);
        assert_eq!(parked_again.saves.len(), 1);
        assert_eq!(read_session_status(&opened, "n-parked"), Status::Parked);
        assert_eq!(read_stream_status(&opened, "work"), Status::Active);
    }

    #[test]
    fn park_and_resume_do_not_scan_unrelated_sessions() {
        let mut park_counts = Vec::new();
        let mut resume_counts = Vec::new();
        for unrelated in [0, 64] {
            let (_dir, config, opened) = setup_opened();
            let host = opened.host().as_str().to_string();
            let stream = Stream::new("work", "Work", None, &host).unwrap();
            create_record(opened.records(), "cs-1", &Record::Stream(stream)).unwrap();
            let target = session_with_status(&host, "work", "target", Status::Active);
            create_record(opened.records(), "cs-2", &Record::Session(target)).unwrap();
            for i in 0..unrelated {
                let extra =
                    session_with_status(&host, "work", &format!("extra-{i}"), Status::Parked);
                create_record(
                    opened.records(),
                    &format!("cs-e-{i}"),
                    &Record::Session(extra),
                )
                .unwrap();
            }

            let events = Arc::new(Mutex::new(Vec::<RecordIoEvent>::new()));
            let sink = events.clone();
            let observed =
                open_records_observed(&config, move |ev| sink.lock().unwrap().push(ev)).unwrap();
            events.lock().unwrap().clear();

            let parked = park_session(&observed, Harness::Pi, "target", "park-bounded").unwrap();
            assert!(parked.session_changed);
            park_counts.push(events.lock().unwrap().len());

            events.lock().unwrap().clear();
            let resumed =
                resume_session(&observed, Harness::Pi, "target", "resume-bounded").unwrap();
            assert!(resumed.session_changed);
            resume_counts.push(events.lock().unwrap().len());
        }
        assert_eq!(park_counts[0], park_counts[1]);
        assert_eq!(resume_counts[0], resume_counts[1]);
    }

    #[test]
    fn close_stream_sessions_preserves_done_and_skips_moved_sessions() {
        let (_dir, _config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();
        let stream_a = Stream::new("a", "A", None, &host).unwrap();
        let stream_b = Stream::new("b", "B", None, &host).unwrap();
        create_record(opened.records(), "cs-a", &Record::Stream(stream_a)).unwrap();
        create_record(opened.records(), "cs-b", &Record::Stream(stream_b)).unwrap();

        let s_active = session_with_status(&host, "a", "n-active", Status::Active);
        let s_done = session_with_status(&host, "a", "n-done", Status::Done);
        create_record(opened.records(), "cs-1", &Record::Session(s_active.clone())).unwrap();
        create_record(opened.records(), "cs-2", &Record::Session(s_done)).unwrap();

        // Simulate a concurrent move out of stream `a` into `b` after scanning:
        let mv = MoveIntent::new(&host, "pi", "n-active", "b").unwrap();
        move_session_observed(opened.records(), "mv-1", &mv).unwrap();

        // Direct guarded save with stale stream expectation skips the moved Session!
        let (_, skipped) = save_session_status(
            opened.records(),
            &host,
            &s_active.id,
            "close-moved",
            Status::Closed,
            &[Status::Active],
            SessionGuard {
                revision: None,
                stream_id: Some("a"),
            },
        )
        .unwrap();
        assert!(skipped.is_none());
        assert_eq!(read_session_status(&opened, "n-active"), Status::Active);

        // Normal close_stream_sessions closes Active/Parked in `b` and preserves Done in `a`.
        let s_parked_b = session_with_status(&host, "b", "n-parked-b", Status::Parked);
        create_record(opened.records(), "cs-3", &Record::Session(s_parked_b)).unwrap();
        let outcome = close_stream_sessions(&opened, "b", "close-b");
        assert_eq!(outcome.closed_sessions, 2);
        assert_eq!(outcome.saves.len(), 2);
        assert!(outcome.completion_error.is_none());
        assert_eq!(read_session_status(&opened, "n-active"), Status::Closed);
        assert_eq!(read_session_status(&opened, "n-parked-b"), Status::Closed);
        assert_eq!(read_session_status(&opened, "n-done"), Status::Done);
    }

    #[test]
    fn decay_host_records_skips_concurrently_modified_records_via_revision_guard() {
        let (_dir, _config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();

        let stale_stream = Stream::new("stale", "Stale", None, &host).unwrap();
        let mut done_stream = Stream::new("finished", "Finished", None, &host).unwrap();
        done_stream.status = Status::Done;
        create_record(opened.records(), "cs-1", &Record::Stream(stale_stream)).unwrap();
        create_record(opened.records(), "cs-2", &Record::Stream(done_stream)).unwrap();

        let s_stale = session_with_status(&host, "stale", "n-stale", Status::Parked);
        let s_raced = session_with_status(&host, "stale", "n-raced", Status::Active);
        let s_done = session_with_status(&host, "finished", "n-done", Status::Done);
        create_record(opened.records(), "cs-3", &Record::Session(s_stale)).unwrap();
        create_record(opened.records(), "cs-4", &Record::Session(s_raced.clone())).unwrap();
        create_record(opened.records(), "cs-5", &Record::Session(s_done)).unwrap();

        let base = now_ms();
        // Disabled (0 days) or within threshold (5 days) plans nothing.
        assert_eq!(
            decay_host_records(&opened, 0, base + 30 * 86_400_000, "decay-0").unwrap(),
            DecaySummary::default()
        );
        assert_eq!(
            decay_host_records(&opened, 14, base + 5 * 86_400_000, "decay-recent").unwrap(),
            DecaySummary::default()
        );

        // Capture a stale decay plan past 14 days, then concurrently edit n-raced before applying!
        let plan = plan_decay(&opened, 14, base + 15 * 86_400_000).unwrap();
        let changes = json!({"state": "Fresh concurrent update"})
            .as_object()
            .unwrap()
            .clone();
        save_user_edit(
            opened.records(),
            "concurrent-report",
            &Record::Session(s_raced),
            &changes,
            TitleDefaults::default(),
        )
        .unwrap();

        let summary = apply_decay_plan(&opened, &plan, "decay-stale");
        assert_eq!(
            summary,
            DecaySummary {
                closed_sessions: 1, // n-stale closed, n-raced skipped due to revision mismatch!
                closed_streams: 1,
                errors: Vec::new(),
            }
        );
        assert_eq!(read_session_status(&opened, "n-stale"), Status::Closed);
        assert_eq!(read_session_status(&opened, "n-raced"), Status::Active);
        assert_eq!(read_stream_status(&opened, "stale"), Status::Closed);
        assert_eq!(read_session_status(&opened, "n-done"), Status::Done);
        assert_eq!(read_stream_status(&opened, "finished"), Status::Done);
    }

    #[test]
    fn resume_session_retains_session_save_when_stream_reopen_fails() {
        let (_dir, _config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();
        // Create Session whose Stream is missing on disk so reopening Stream fails.
        let orphan_stream = Stream::new("work", "Work", None, &host).unwrap();
        create_record(opened.records(), "cs-1", &Record::Stream(orphan_stream)).unwrap();
        let s = session_with_status(&host, "work", "n-1", Status::Parked);
        create_record(opened.records(), "cs-2", &Record::Session(s)).unwrap();
        let stream_file = opened
            .root()
            .join(format!("hosts/{host}/streams/work.yaml"));
        std::fs::remove_file(stream_file).unwrap();

        let outcome = resume_session(&opened, Harness::Pi, "n-1", "resume-orphan").unwrap();
        assert!(outcome.session_changed);
        assert!(!outcome.stream_changed);
        assert_eq!(outcome.saves.len(), 1);
        assert!(outcome.completion_error.is_some());
        assert_eq!(read_session_status(&opened, "n-1"), Status::Active);
    }
}
