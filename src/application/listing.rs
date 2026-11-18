//! The catalog listing: every host's readable Streams, with status projected from members.

use std::collections::BTreeMap;

use icu_casemap::CaseMapper;

use super::{OpenedRecords, Result};
use crate::{
    model::{Record, Session, Status, Stream},
    operations::{project_stream_status, stream_activity_ms},
    storage::records::UnreadableRecord,
};

/// Which Streams a listing shows. Closed Streams are hidden unless `closed`, `all` or a
/// `Closed` status asks for them.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamFilter<'a> {
    /// Case-folded substring of the title or goal.
    pub query: Option<&'a str>,
    pub status: &'a [Status],
    pub closed: bool,
    pub all: bool,
}

pub struct StreamList {
    /// Sorted by folded title, then owner host and ID.
    pub streams: Vec<Stream>,
    pub unreadable: Vec<UnreadableRecord>,
}

/// List Streams with their projected status. Only this host's Streams decay after
/// `cutoff_ms`.
pub fn list_streams(
    opened: &OpenedRecords,
    filter: StreamFilter,
    cutoff_ms: Option<u64>,
) -> Result<StreamList> {
    let fold = |value: &str| CaseMapper::new().fold_string(value).into_owned();
    let query = filter.query.map(fold);
    let listing = opened.listing()?;
    let mut sessions_by_stream: BTreeMap<(String, String), Vec<Session>> = BTreeMap::new();
    for read in &listing.records {
        if let Record::Session(session) = &read.record {
            sessions_by_stream
                .entry((session.host_id.clone(), session.stream_id.clone()))
                .or_default()
                .push(session.clone());
        }
    }
    let mut streams: Vec<_> = listing
        .records
        .into_iter()
        .filter_map(|read| match read.record {
            Record::Stream(mut stream) => {
                let members = sessions_by_stream
                    .get(&(stream.owner_host_id.clone(), stream.id.clone()))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let refs: Vec<&_> = members.iter().collect();
                let stream_cutoff =
                    cutoff_ms.filter(|_| stream.owner_host_id == opened.host().as_str());
                let last_updated_ms = stream_activity_ms(&stream, &refs);
                stream.status =
                    project_stream_status(stream.status, &refs, last_updated_ms, stream_cutoff);
                Some(stream)
            }
            _ => None,
        })
        .filter(|stream| {
            (filter.all
                || if filter.closed {
                    stream.status == Status::Closed
                } else {
                    stream.status != Status::Closed || filter.status.contains(&Status::Closed)
                })
                && (filter.status.is_empty() || filter.status.contains(&stream.status))
                && query.as_ref().is_none_or(|query| {
                    fold(&stream.title).contains(query)
                        || fold(stream.goal.as_deref().unwrap_or("")).contains(query)
                })
        })
        .collect();
    streams.sort_by_cached_key(|stream| {
        (
            fold(&stream.title),
            stream.owner_host_id.clone(),
            stream.id.clone(),
        )
    });
    Ok(StreamList {
        streams,
        unreadable: listing.unreadable,
    })
}
