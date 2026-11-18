//! Host-local Session/Stream lifecycle transitions, guarded closures, and age decay.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{ApplicationError, OpenedRecords, Result};
use crate::{
    codec::record_to_data,
    model::{Record, RecordKind, Session, Status, ValidationError},
    operations::{DomainOutcome, decay_cutoff_ms, is_stale_ms},
    runtime::conversations::Harness,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRequest, RecordSave, RecordStore},
    },
};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DecaySummary {
    pub closed_sessions: usize,
    pub closed_streams: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleSave {
    pub operation_id: String,
    pub save: RecordSave,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleOutcome {
    pub session: Option<Session>,
    pub session_changed: bool,
    pub stream_changed: bool,
    pub saves: Vec<LifecycleSave>,
    pub completion_error: Option<String>,
}

impl LifecycleOutcome {
    pub fn unchanged(session: Option<Session>) -> Self {
        Self {
            session,
            session_changed: false,
            stream_changed: false,
            saves: Vec::new(),
            completion_error: None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        let saves: Vec<serde_json::Value> = self
            .saves
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "operation_id": entry.operation_id,
                    "save": {
                        "local": if entry.save.repeated { "unchanged" } else { "persisted" },
                        "domain": entry.save.outcome,
                    }
                })
            })
            .collect();
        let mut body = serde_json::json!({
            "session_changed": self.session_changed,
            "stream_changed": self.stream_changed,
            "saves": saves,
            "session": self.session.as_ref().map(|s| record_to_data(&Record::Session(s.clone()))),
        });
        if let Some(error) = &self.completion_error {
            body["completion_error"] = serde_json::json!(error);
        }
        body
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CloseChildrenOutcome {
    pub closed_sessions: usize,
    pub saves: Vec<LifecycleSave>,
    pub completion_error: Option<String>,
}

#[derive(Debug, Default, Clone, Copy)]
struct SessionGuard<'a> {
    revision: Option<&'a str>,
    stream_id: Option<&'a str>,
}

enum LifecycleSaveError {
    App(ApplicationError),
    GuardRejected(Box<Record>),
}

impl From<StorageError> for LifecycleSaveError {
    fn from(error: StorageError) -> Self {
        Self::App(error.into())
    }
}

impl From<ValidationError> for LifecycleSaveError {
    fn from(error: ValidationError) -> Self {
        Self::App(error.into())
    }
}

fn save_record_status(
    store: &impl RecordStore,
    address: RecordAddress,
    operation_id: &str,
    next_status: Status,
    allowed: impl Fn(Status) -> bool,
    guard: SessionGuard<'_>,
) -> Result<(Record, Option<LifecycleSave>)> {
    let kind_label = match address.kind() {
        RecordKind::Session => "session_status",
        RecordKind::Stream => "stream_status",
        RecordKind::Host => return Err(ValidationError::new("invalid lifecycle target").into()),
    };
    let expected_kind = address.kind();
    let request = RecordRequest::new(
        address,
        operation_id,
        &serde_json::json!({"lifecycle": kind_label, "status": next_status}),
    )?;
    let saved = store.save::<LifecycleSaveError>(&request, |_, before| {
        let Some(before) = before else {
            let missing = if expected_kind == RecordKind::Session {
                "Session is missing"
            } else {
                "Stream is missing"
            };
            return Err(ValidationError::new(missing).into());
        };
        let mut record = before.record.clone();
        let (id_owned, current_status, stream_ok) = match &record {
            Record::Session(session) if expected_kind == RecordKind::Session => (
                session.id.clone(),
                session.status,
                guard.stream_id.is_none_or(|sid| sid == session.stream_id),
            ),
            Record::Stream(stream) if expected_kind == RecordKind::Stream => {
                (stream.id.clone(), stream.status, true)
            }
            _ => {
                return Err(ValidationError::new("lifecycle target record kind mismatch").into());
            }
        };
        let revision_ok = guard.revision.is_none_or(|rev| rev == before.revision);
        if !(revision_ok && stream_ok && allowed(current_status) && current_status != next_status) {
            // Abort without calling atomic_write or appending a no-op receipt.
            return Err(LifecycleSaveError::GuardRejected(Box::new(
                before.record.clone(),
            )));
        }
        match &mut record {
            Record::Session(session) => session.status = next_status,
            Record::Stream(stream) => stream.status = next_status,
            Record::Host(_) => unreachable!(),
        }
        let receipt = match expected_kind {
            RecordKind::Session => serde_json::json!({
                "outcome": DomainOutcome::Changes,
                "session_id": id_owned,
                "status": next_status,
            }),
            RecordKind::Stream => serde_json::json!({
                "outcome": DomainOutcome::Changes,
                "stream_id": id_owned,
                "status": next_status,
            }),
            RecordKind::Host => unreachable!(),
        };
        Ok(RecordChange {
            record,
            outcome: receipt.as_object().expect("receipt object").clone(),
        })
    });
    match saved {
        Ok(save) => {
            let record = save.current.record.clone();
            let entry = LifecycleSave {
                operation_id: operation_id.to_owned(),
                save,
            };
            Ok((record, Some(entry)))
        }
        Err(LifecycleSaveError::GuardRejected(record)) => Ok((*record, None)),
        Err(LifecycleSaveError::App(error)) => Err(error),
    }
}

fn save_session_status(
    store: &impl RecordStore,
    host: &str,
    session_id: &str,
    operation_id: &str,
    next_status: Status,
    allowed: impl Fn(Status) -> bool,
    guard: SessionGuard<'_>,
) -> Result<(Option<Session>, Option<LifecycleSave>)> {
    let address = RecordAddress::new(host, RecordKind::Session, session_id)?;
    let (record, save) =
        save_record_status(store, address, operation_id, next_status, allowed, guard)?;
    let Record::Session(session) = record else {
        return Err(ValidationError::new("lifecycle target must be a Session").into());
    };
    Ok((Some(session), save))
}

fn save_stream_status(
    store: &impl RecordStore,
    host: &str,
    stream_id: &str,
    operation_id: &str,
    next_status: Status,
    allowed: impl Fn(Status) -> bool,
    expected_revision: Option<&str>,
) -> Result<Option<LifecycleSave>> {
    let address = RecordAddress::new(host, RecordKind::Stream, stream_id)?;
    let (_, save) = save_record_status(
        store,
        address,
        operation_id,
        next_status,
        allowed,
        SessionGuard {
            revision: expected_revision,
            stream_id: None,
        },
    )?;
    Ok(save)
}

/// Reopen a `closed` Stream when an enrolled Session in it is actively resumed or enrolled.
/// Reads first so an already-open Stream performs zero write-lock acquisitions or file rewrites.
pub fn reopen_closed_stream(
    opened: &OpenedRecords,
    stream_id: &str,
    operation_id: &str,
) -> Result<Option<LifecycleSave>> {
    let host = opened.host().as_str();
    let addr = RecordAddress::new(host, RecordKind::Stream, stream_id)?;
    let read = opened
        .records()
        .with_reader(|r| r.read(&addr))?
        .ok_or_else(|| ValidationError::new("Stream is missing"))?;
    let Record::Stream(stream) = read.record else {
        return Err(ValidationError::new("lifecycle target must be a Stream").into());
    };
    if stream.status != Status::Closed {
        return Ok(None);
    }
    save_stream_status(
        opened.records(),
        host,
        stream_id,
        operation_id,
        Status::Active,
        |status| status == Status::Closed,
        None,
    )
}

/// Close every open Session still belonging to `stream_id`, preserving all accepted saves
/// and surfacing any partial failure without rolling back prior writes.
pub fn close_stream_sessions(
    opened: &OpenedRecords,
    stream_id: &str,
    operation_id_prefix: &str,
) -> CloseChildrenOutcome {
    let host = opened.host().as_str();
    let session_reads = match opened
        .host_sessions()
        .and_then(|listing| listing.complete().map_err(Into::into))
    {
        Ok(reads) => reads,
        Err(error) => {
            return CloseChildrenOutcome {
                closed_sessions: 0,
                saves: Vec::new(),
                completion_error: Some(error.to_string()),
            };
        }
    };
    let candidates: Vec<String> = session_reads
        .into_iter()
        .filter_map(|read| match read.record {
            Record::Session(session)
                if session.stream_id == stream_id && session.status.is_open() =>
            {
                Some(session.id)
            }
            _ => None,
        })
        .collect();
    let mut outcome = CloseChildrenOutcome::default();
    let mut errors = Vec::new();
    for session_id in candidates {
        let op = format!("{operation_id_prefix}:{session_id}");
        match save_session_status(
            opened.records(),
            host,
            &session_id,
            &op,
            Status::Closed,
            Status::is_open,
            SessionGuard {
                revision: None,
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

/// Cascade child Session closures when a Stream edit transitions its status to `Closed`.
pub fn cascade_stream_close_if_transitioned(
    opened: &OpenedRecords,
    before_status: Status,
    after_record: &Record,
    operation_id_prefix: &str,
) -> Option<CloseChildrenOutcome> {
    let Record::Stream(stream) = after_record else {
        return None;
    };
    if before_status == Status::Closed || stream.status != Status::Closed {
        return None;
    }
    Some(close_stream_sessions(
        opened,
        &stream.id,
        operation_id_prefix,
    ))
}

fn transition_session_status(
    opened: &OpenedRecords,
    harness: Harness,
    native_id: &str,
    operation_id: &str,
    next_status: Status,
    allowed: impl Fn(Status) -> bool + Copy,
    reopen_stream: bool,
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
    let (session, session_save) = if allowed(existing.status) && existing.status != next_status {
        save_session_status(
            opened.records(),
            host,
            &existing.id,
            &format!("{operation_id}:session"),
            next_status,
            allowed,
            SessionGuard::default(),
        )?
    } else {
        (Some(existing), None)
    };
    let mut saves = Vec::new();
    let session_changed = if let Some(save) = session_save {
        saves.push(save);
        true
    } else {
        false
    };
    let mut stream_changed = false;
    let mut completion_error = None;
    if reopen_stream
        && let Some(session_ref) = &session
        && session_ref.status == Status::Active
    {
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

/// Reactivate a parked or closed Session, reopening a closed parent Stream when active.
/// Preserves accepted Session writes if reopening the Stream fails.
pub fn resume_session(
    opened: &OpenedRecords,
    harness: Harness,
    native_id: &str,
    operation_id: &str,
) -> Result<LifecycleOutcome> {
    transition_session_status(
        opened,
        harness,
        native_id,
        operation_id,
        Status::Active,
        |status| matches!(status, Status::Parked | Status::Closed),
        true,
    )
}

/// Park an active Session when its harness conversation shuts down.
/// Reads and saves only the addressed Session record (zero host scans, never reopens a Stream).
pub fn park_session(
    opened: &OpenedRecords,
    harness: Harness,
    native_id: &str,
    operation_id: &str,
) -> Result<LifecycleOutcome> {
    transition_session_status(
        opened,
        harness,
        native_id,
        operation_id,
        Status::Parked,
        |status| status == Status::Active,
        false,
    )
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
    let Some(cutoff_ms) = decay_cutoff_ms(decay_days, now_ms) else {
        return Ok(DecayPlan::default());
    };

    let host = opened.host().as_str();
    let session_reads = opened.host_sessions()?.complete()?;
    let stream_reads = opened.host_streams()?.complete()?;

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
        if stream.status.is_open() && is_stale_ms(effective_ms, Some(cutoff_ms)) {
            decayed_streams.insert(stream.id.clone());
            plan.streams.push((stream.id.clone(), revision.clone()));
        }
    }

    for (s, revision) in &sessions {
        if !s.status.is_open() {
            continue;
        }
        let addr = RecordAddress::new(host, RecordKind::Session, &s.id)?;
        let ms = timestamps.get(&addr).copied().unwrap_or(0);
        if decayed_streams.contains(&s.stream_id) || is_stale_ms(ms, Some(cutoff_ms)) {
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
    let mut skipped_streams = BTreeSet::new();

    for (session_id, stream_id, revision) in &plan.sessions {
        let op = format!("{operation_id_prefix}:decay:session:{session_id}");
        match save_session_status(
            opened.records(),
            host,
            session_id,
            &op,
            Status::Closed,
            Status::is_open,
            SessionGuard {
                revision: Some(revision),
                stream_id: Some(stream_id),
            },
        ) {
            Ok((_, Some(_))) => summary.closed_sessions += 1,
            Ok((_, None)) => {
                skipped_streams.insert(stream_id.clone());
            }
            Err(error) => {
                skipped_streams.insert(stream_id.clone());
                summary
                    .errors
                    .push(format!("session {session_id}: {error}"));
            }
        }
    }

    for (stream_id, revision) in &plan.streams {
        if skipped_streams.contains(stream_id) {
            continue;
        }
        let op = format!("{operation_id_prefix}:decay:stream:{stream_id}");
        match save_stream_status(
            opened.records(),
            host,
            stream_id,
            &op,
            Status::Closed,
            Status::is_open,
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
            Status::is_open,
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
            &Record::Session(s_raced.clone()),
            &changes,
            TitleDefaults::default(),
        )
        .unwrap();
        let raced_addr = RecordAddress::new(&host, RecordKind::Session, &s_raced.id).unwrap();
        let raced_rev_before = opened
            .records()
            .with_reader(|r| r.read(&raced_addr))
            .unwrap()
            .unwrap()
            .revision;

        let summary = apply_decay_plan(&opened, &plan, "decay-stale");
        assert_eq!(
            summary,
            DecaySummary {
                closed_sessions: 1, // n-stale closed, n-raced skipped due to revision mismatch!
                closed_streams: 0,  // stale Stream stays open because n-raced remained Active!
                errors: Vec::new(),
            }
        );
        assert_eq!(read_session_status(&opened, "n-stale"), Status::Closed);
        assert_eq!(read_session_status(&opened, "n-raced"), Status::Active);
        assert_eq!(read_stream_status(&opened, "stale"), Status::Active);
        // Guard rejection must NOT rewrite n-raced or bump its revision!
        let raced_rev_after = opened
            .records()
            .with_reader(|r| r.read(&raced_addr))
            .unwrap()
            .unwrap()
            .revision;
        assert_eq!(raced_rev_before, raced_rev_after);
        assert_eq!(read_session_status(&opened, "n-done"), Status::Done);
        assert_eq!(read_stream_status(&opened, "finished"), Status::Done);
    }

    #[test]
    fn park_and_resume_tolerate_concurrent_non_status_edits_and_reopen_closed_stream_on_active() {
        let (_dir, config, opened) = setup_opened();
        let host = opened.host().as_str().to_string();
        let mut closed_stream = Stream::new("work", "Work", None, &host).unwrap();
        closed_stream.status = Status::Closed;
        create_record(opened.records(), "cs-1", &Record::Stream(closed_stream)).unwrap();

        // Already-active session inside a closed stream reopens the stream on resume without a redundant session write.
        let active = session_with_status(&host, "work", "n-active", Status::Active);
        create_record(opened.records(), "cs-2", &Record::Session(active.clone())).unwrap();
        let reopened = resume_session(&opened, Harness::Pi, "n-active", "resume-active").unwrap();
        assert!(!reopened.session_changed);
        assert!(reopened.stream_changed);
        assert_eq!(reopened.saves.len(), 1);
        assert_eq!(read_stream_status(&opened, "work"), Status::Active);

        // Second resume on already-open stream performs zero file writes or lock acquisitions.
        let events = Arc::new(Mutex::new(Vec::<RecordIoEvent>::new()));
        let sink = events.clone();
        let observed =
            open_records_observed(&config, move |ev| sink.lock().unwrap().push(ev)).unwrap();
        events.lock().unwrap().clear();
        let noop = resume_session(&observed, Harness::Pi, "n-active", "resume-noop").unwrap();
        assert!(!noop.session_changed);
        assert!(!noop.stream_changed);
        assert!(noop.saves.is_empty());
        assert!(
            events
                .lock()
                .unwrap()
                .iter()
                .all(|ev| !matches!(ev, RecordIoEvent::Written(_, _)))
        );

        // Concurrent non-status edit does not block park_session.
        let changes = json!({"recap": "Concurrent report"})
            .as_object()
            .unwrap()
            .clone();
        save_user_edit(
            opened.records(),
            "concurrent-report-2",
            &Record::Session(active),
            &changes,
            TitleDefaults::default(),
        )
        .unwrap();
        let parked = park_session(&opened, Harness::Pi, "n-active", "park-concurrent").unwrap();
        assert!(parked.session_changed);
        assert_eq!(read_session_status(&opened, "n-active"), Status::Parked);
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
