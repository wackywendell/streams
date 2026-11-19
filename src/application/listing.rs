//! The catalog listing: every host's readable Streams, with status derived from members.

use std::collections::BTreeMap;

use icu_casemap::CaseMapper;
use serde::Serialize;

use super::{OpenedRecords, Result};
use crate::{
    model::{Record, Session, Status, Stream},
    operations::derived_stream_status,
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

/// One Stream in a listing with its view-only derived status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedStream {
    #[serde(flatten)]
    pub stream: Stream,
    pub status: Status,
}

pub struct StreamList {
    /// Sorted by folded title, then owner host and ID.
    pub streams: Vec<ListedStream>,
    pub unreadable: Vec<UnreadableRecord>,
}

/// List Streams with their derived status. Only this host's Streams decay after
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
        if let Record::Session(session) = read {
            sessions_by_stream
                .entry((session.host_id.clone(), session.stream_id.clone()))
                .or_default()
                .push(session.clone());
        }
    }
    let mut streams: Vec<_> = listing
        .records
        .into_iter()
        .filter_map(|read| match read {
            Record::Stream(stream) => {
                let members = sessions_by_stream
                    .get(&(stream.owner_host_id.clone(), stream.id.clone()))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let refs: Vec<&_> = members.iter().collect();
                let stream_cutoff =
                    cutoff_ms.filter(|_| stream.owner_host_id == opened.host().as_str());
                let status = derived_stream_status(&stream, &refs, stream_cutoff);
                Some(ListedStream { stream, status })
            }
            _ => None,
        })
        .filter(|entry| {
            (filter.all
                || if filter.closed {
                    entry.status == Status::Closed
                } else {
                    entry.status != Status::Closed || filter.status.contains(&Status::Closed)
                })
                && (filter.status.is_empty() || filter.status.contains(&entry.status))
                && query.as_ref().is_none_or(|query| {
                    fold(&entry.stream.title).contains(query)
                        || fold(entry.stream.goal.as_deref().unwrap_or("")).contains(query)
                })
        })
        .collect();
    streams.sort_by_cached_key(|entry| {
        (
            fold(&entry.stream.title),
            entry.stream.owner_host_id.clone(),
            entry.stream.id.clone(),
        )
    });
    Ok(StreamList {
        streams,
        unreadable: listing.unreadable,
    })
}
