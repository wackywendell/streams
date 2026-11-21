//! Catalog listings for Streams and Sessions, sorted by most recent activity.

use std::collections::{BTreeMap, BTreeSet};

use icu_casemap::CaseMapper;

use super::{OpenedRecords, Result};
use crate::{
    model::{Record, Session, Status, Stream},
    operations::{derived_stream_status, stream_activity_ms},
    storage::records::{RecordAddress, RecordStore, UnreadableRecord},
};

/// Which Streams a listing shows. Closed Streams are hidden unless `all` or `--status closed`.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamFilter<'a> {
    /// Case-folded substring of the title or goal.
    pub query: Option<&'a str>,
    pub status: &'a [Status],
    pub all: bool,
    pub host: Option<&'a str>,
}

/// One Stream in a listing with its member Sessions and derived fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedStream {
    pub stream: Stream,
    pub members: Vec<Session>,
    pub status: Status,
    pub last_activity_ms: u64,
}

pub struct StreamList {
    /// Sorted by `last_activity_ms` descending, then full key ascending.
    pub streams: Vec<ListedStream>,
    pub unreadable: Vec<UnreadableRecord>,
}

/// List Streams with their derived status and member Sessions. Only this host's Streams decay
/// after `cutoff_ms`.
pub fn list_streams(
    opened: &OpenedRecords,
    filter: StreamFilter<'_>,
    cutoff_ms: Option<u64>,
) -> Result<StreamList> {
    let fold = |value: &str| CaseMapper::new().fold_string(value).into_owned();
    let query = filter.query.map(fold);
    let listing = opened.listing()?;
    let mut sessions_by_stream: BTreeMap<(String, String), Vec<Session>> = BTreeMap::new();
    for read in &listing.records {
        if let Record::Session(session) = read {
            sessions_by_stream
                .entry((session.host_id.clone(), session.stream_id.clone()))
                .or_default()
                .push(session.clone());
        }
    }
    let mut streams: Vec<ListedStream> = listing
        .records
        .into_iter()
        .filter_map(|read| match read {
            Record::Stream(stream) => {
                if let Some(host) = filter.host
                    && stream.host_id != host
                {
                    return None;
                }
                let members = sessions_by_stream
                    .remove(&(stream.host_id.clone(), stream.id.clone()))
                    .unwrap_or_default();
                let refs: Vec<&Session> = members.iter().collect();
                let stream_cutoff = cutoff_ms.filter(|_| stream.host_id == opened.host().as_str());
                let status = derived_stream_status(&stream, &refs, stream_cutoff);
                let last_activity_ms = stream_activity_ms(&stream, &refs);
                Some(ListedStream {
                    stream,
                    members,
                    status,
                    last_activity_ms,
                })
            }
            _ => None,
        })
        .filter(|entry| {
            let show_closed = filter.all || filter.status.contains(&Status::Closed);
            (show_closed || entry.status != Status::Closed)
                && (filter.status.is_empty() || filter.status.contains(&entry.status))
                && query.as_ref().is_none_or(|needle| {
                    fold(&entry.stream.title.text).contains(needle)
                        || entry
                            .stream
                            .goal
                            .as_ref()
                            .is_some_and(|goal| fold(&goal.text).contains(needle))
                })
        })
        .collect();
    streams.sort_by(|left, right| {
        right
            .last_activity_ms
            .cmp(&left.last_activity_ms)
            .then_with(|| {
                left.stream
                    .key()
                    .to_string()
                    .cmp(&right.stream.key().to_string())
            })
    });
    Ok(StreamList {
        streams,
        unreadable: listing.unreadable,
    })
}

/// Which Sessions a listing shows. Closed Sessions are hidden unless `all` or `--status closed`.
#[derive(Debug, Clone, Default)]
pub struct SessionFilter<'a> {
    pub query: Option<&'a str>,
    pub stream: Option<(String, String)>,
    /// Exact Session addresses to read directly (`--key`). Missing keys are silently omitted.
    pub keys: Vec<RecordAddress>,
    pub status: &'a [Status],
    pub all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedSession {
    pub session: Session,
}

pub struct SessionList {
    /// Sorted by `updated_at_ms` descending, then full key ascending.
    pub sessions: Vec<ListedSession>,
    pub unreadable: Vec<UnreadableRecord>,
}

pub fn list_sessions(opened: &OpenedRecords, filter: &SessionFilter<'_>) -> Result<SessionList> {
    let fold = |value: &str| CaseMapper::new().fold_string(value).into_owned();
    let query = filter.query.map(fold);

    let (raw_sessions, unreadable) = if !filter.keys.is_empty() {
        // Point reads only: deduplicate keys, silently omit missing Sessions, collect decode errors.
        let unique: BTreeSet<&RecordAddress> = filter.keys.iter().collect();
        let mut sessions = Vec::new();
        let mut unreadable = Vec::new();
        opened.records().with_reader(|reader| -> Result<()> {
            for addr in unique {
                match reader.read(addr) {
                    Ok(Some(Record::Session(session))) => sessions.push(session),
                    Ok(_) => {}
                    Err(error) => {
                        if let Some(record) = error.into_unreadable() {
                            unreadable.push(record);
                        }
                    }
                }
            }
            Ok(())
        })?;
        (sessions, unreadable)
    } else if let Some((host, _)) = &filter.stream {
        let listing = opened.sessions_for_host(host)?;
        let sessions = listing
            .records
            .into_iter()
            .filter_map(|record| match record {
                Record::Session(session) => Some(session),
                _ => None,
            })
            .collect();
        (sessions, listing.unreadable)
    } else {
        let listing = opened.listing()?;
        let sessions = listing
            .records
            .into_iter()
            .filter_map(|record| match record {
                Record::Session(session) => Some(session),
                _ => None,
            })
            .collect();
        (sessions, listing.unreadable)
    };

    let mut sessions: Vec<ListedSession> = raw_sessions
        .into_iter()
        .filter(|session| {
            if let Some((host, stream_id)) = &filter.stream
                && (&session.host_id != host || &session.stream_id != stream_id)
            {
                return false;
            }
            let show_closed = filter.all || filter.status.contains(&Status::Closed);
            (show_closed || session.status != Status::Closed)
                && (filter.status.is_empty() || filter.status.contains(&session.status))
                && query
                    .as_ref()
                    .is_none_or(|needle| fold(&session.title.text).contains(needle))
        })
        .map(|session| ListedSession { session })
        .collect();

    sessions.sort_by(|left, right| {
        right
            .session
            .updated_at_ms
            .cmp(&left.session.updated_at_ms)
            .then_with(|| {
                left.session
                    .key()
                    .to_string()
                    .cmp(&right.session.key().to_string())
            })
    });

    Ok(SessionList {
        sessions,
        unreadable,
    })
}
