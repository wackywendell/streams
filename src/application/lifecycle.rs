//! Host-local Session age decay (`catalog tidy`).

use serde::Serialize;

use super::{OpenedRecords, Result};
use crate::{
    model::{Record, Status, ValidationError},
    operations::{decay_cutoff_ms, is_stale_ms},
    storage::records::{RecordAddress, RecordChange, RecordStore},
};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct DecaySummary {
    pub closed_sessions: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct DecayPlan {
    pub(crate) sessions: Vec<(String, String, u64)>, // (harness, session_id, revision)
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
            plan.sessions
                .push((session.harness, session.id, session.revision));
        }
    }
    Ok(plan)
}

fn close_if_unchanged(
    store: &impl RecordStore,
    host: &str,
    harness: &str,
    session_id: &str,
    expected_revision: u64,
) -> Result<bool> {
    let address = RecordAddress::session(host, harness, session_id)?;
    let saved = store.save(&address, |_, before| -> Result<RecordChange<()>> {
        let Some(Record::Session(mut session)) = before.cloned() else {
            return Err(ValidationError::new("Session is missing").into());
        };
        if session.revision == expected_revision && session.status.is_open() {
            session.status = Status::Closed;
        }
        Ok(RecordChange {
            record: Record::Session(session),
            outcome: (),
        })
    })?;
    Ok(saved.written)
}

pub(crate) fn apply_decay_plan(opened: &OpenedRecords, plan: &DecayPlan) -> DecaySummary {
    let host = opened.host().as_str();
    let mut summary = DecaySummary::default();

    for (harness, session_id, revision) in &plan.sessions {
        match close_if_unchanged(opened.records(), host, harness, session_id, *revision) {
            Ok(true) => summary.closed_sessions += 1,
            Ok(false) => {}
            Err(error) => {
                summary
                    .errors
                    .push(format!("session {host}/{harness}/{session_id}: {error}"));
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
    use std::time::{SystemTime, UNIX_EPOCH};

    use tempfile::tempdir;

    use super::*;
    use crate::{
        application::{StreamFilter, create_record, initialize_records, list_streams},
        config::{Config, Strategy},
        model::{Session, Stream, Title},
        operations::derived_stream_status,
    };

    fn test_config() -> (tempfile::TempDir, Config) {
        let dir = tempdir().unwrap();
        let env = std::collections::BTreeMap::from([(
            "HOME".into(),
            dir.path().to_str().unwrap().to_owned(),
        )]);
        let config = Config::new(&env, dir.path()).unwrap();
        (dir, config)
    }

    #[test]
    fn decay_closes_stale_open_sessions_and_skips_concurrently_edited_ones() {
        let (_dir, config) = test_config();
        let opened = initialize_records(&config, Strategy::None, None, "main").unwrap();
        let host = opened.host().as_str().to_owned();
        let stream = Stream::new(&host, "s1", Title::new("Work", false), None).unwrap();
        create_record(opened.records(), &Record::Stream(stream)).unwrap();

        let s_stale = Session::new(&host, "pi", "sess-1", "s1", Title::new("One", false)).unwrap();
        let s_done = Session {
            status: Status::Done,
            ..Session::new(&host, "pi", "sess-2", "s1", Title::new("Two", false)).unwrap()
        };
        create_record(opened.records(), &Record::Session(s_stale)).unwrap();
        create_record(opened.records(), &Record::Session(s_done)).unwrap();

        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let future_ms = now_ms + 30 * 86_400_000;

        let plan = plan_decay(&opened, 14, future_ms).unwrap();
        assert_eq!(plan.sessions.len(), 1);
        assert_eq!(plan.sessions[0].1, "sess-1");

        // Concurrent save bumps revision; stale plan must skip it.
        let addr = RecordAddress::session(&host, "pi", "sess-1").unwrap();
        opened
            .records()
            .save(&addr, |_, before| -> Result<RecordChange<()>> {
                let Some(Record::Session(mut s)) = before.cloned() else {
                    unreachable!();
                };
                s.title = Title::new("Updated", true);
                Ok(RecordChange {
                    record: Record::Session(s),
                    outcome: (),
                })
            })
            .unwrap();
        let skipped = apply_decay_plan(&opened, &plan);
        assert_eq!(skipped.closed_sessions, 0);

        // Fresh plan closes the stale session.
        let summary = decay_host_records(&opened, 14, future_ms).unwrap();
        assert_eq!(summary.closed_sessions, 1);

        let listed = list_streams(&opened, StreamFilter::default(), None).unwrap();
        assert_eq!(listed.streams.len(), 1);
        assert_eq!(listed.streams[0].status, Status::Done);
        let refs: Vec<&Session> = listed.streams[0].members.iter().collect();
        assert_eq!(
            derived_stream_status(&listed.streams[0].stream, &refs, None),
            Status::Done
        );
    }
}
