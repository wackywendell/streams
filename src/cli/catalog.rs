//! Catalog actions over admitted records; publication is a separate transport request.

#[cfg(test)]
mod tests;

use super::args::StreamFields;
use super::{CliError, Output, Result, fields, fold, printable, resolve, result_data};
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, path::Path};
use streams::{
    agent,
    application::{self, OpenedRecords},
    codec::record_to_data,
    model::{Record, Status, Stream, TitleDefaults, ValidationError},
    storage::records::RecordStore,
    sync::records::RecordSync,
};

const UNREADABLE_REPAIR: &str = "Unreadable records are not listed. Run /streams reset in a conversation on that Stream, or delete or fix the file.";

pub(super) fn list_records(
    opened: &streams::application::OpenedRecords,
    query: Option<&str>,
    status: &[Status],
    closed: bool,
    all: bool,
    cutoff_ms: Option<u64>,
    publication: Option<serde_json::Value>,
) -> Result<Output> {
    let query = query.map(fold);
    let listing = opened.listing()?;
    let records = listing.records;
    let mut sessions_by_stream: std::collections::BTreeMap<
        (String, String),
        Vec<streams::model::Session>,
    > = std::collections::BTreeMap::new();
    let mut addresses = Vec::new();
    for read in &records {
        match &read.record {
            Record::Session(session) => {
                if cutoff_ms.is_some() && session.host_id == opened.host().as_str() {
                    addresses.push(
                        streams::storage::records::RecordAddress::new(
                            &session.host_id,
                            streams::model::RecordKind::Session,
                            &session.id,
                        )
                        .map_err(application::ApplicationError::from)?,
                    );
                }
                sessions_by_stream
                    .entry((session.host_id.clone(), session.stream_id.clone()))
                    .or_default()
                    .push(session.clone());
            }
            Record::Stream(stream) => {
                if cutoff_ms.is_some() && stream.owner_host_id == opened.host().as_str() {
                    addresses.push(
                        streams::storage::records::RecordAddress::new(
                            &stream.owner_host_id,
                            streams::model::RecordKind::Stream,
                            &stream.id,
                        )
                        .map_err(application::ApplicationError::from)?,
                    );
                }
            }
            Record::Host(_) => {}
        }
    }
    let timestamps = if cutoff_ms.is_some() && !addresses.is_empty() {
        opened
            .last_updated_ms(&addresses)
            .or_else(|_| opened.local_last_updated_ms(&addresses))?
    } else {
        std::collections::BTreeMap::new()
    };
    let mut latest_session_ms: std::collections::BTreeMap<(String, String), u64> =
        std::collections::BTreeMap::new();
    for ((host_id, stream_id), members) in &sessions_by_stream {
        if host_id != opened.host().as_str() {
            continue;
        }
        let mut latest = 0u64;
        for session in members {
            let addr = streams::storage::records::RecordAddress::new(
                host_id,
                streams::model::RecordKind::Session,
                &session.id,
            )
            .map_err(application::ApplicationError::from)?;
            if let Some(&ms) = timestamps.get(&addr) {
                latest = latest.max(ms);
            }
        }
        latest_session_ms.insert((host_id.clone(), stream_id.clone()), latest);
    }
    let mut streams: Vec<_> = records
        .into_iter()
        .filter_map(|read| match read.record {
            Record::Stream(mut stream) => {
                let members = sessions_by_stream
                    .get(&(stream.owner_host_id.clone(), stream.id.clone()))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let refs: Vec<&_> = members.iter().collect();
                let (last_updated_ms, stream_cutoff) =
                    if cutoff_ms.is_some() && stream.owner_host_id == opened.host().as_str() {
                        let stream_addr = streams::storage::records::RecordAddress::new(
                            &stream.owner_host_id,
                            streams::model::RecordKind::Stream,
                            &stream.id,
                        )
                        .ok();
                        let stream_ms = stream_addr
                            .as_ref()
                            .and_then(|addr| timestamps.get(addr))
                            .copied()
                            .unwrap_or(0);
                        let session_ms = latest_session_ms
                            .get(&(stream.owner_host_id.clone(), stream.id.clone()))
                            .copied()
                            .unwrap_or(0);
                        (stream_ms.max(session_ms), cutoff_ms)
                    } else {
                        (0, None)
                    };
                stream.status = streams::operations::project_stream_status(
                    stream.status,
                    &refs,
                    last_updated_ms,
                    stream_cutoff,
                );
                Some(stream)
            }
            _ => None,
        })
        .filter(|stream| {
            (all || if closed {
                stream.status == Status::Closed
            } else {
                stream.status != Status::Closed || status.contains(&Status::Closed)
            }) && (status.is_empty() || status.contains(&stream.status))
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
    let mut plain = String::from("HOST/ID  STATUS  TITLE");
    for stream in &streams {
        plain.push_str(&format!(
            "\n{}/{}  {}  {}",
            stream.owner_host_id,
            stream.id,
            stream.status,
            printable(&stream.title)
        ));
    }
    let mut output = Output::plain(
        json!({"streams":streams,"unreadable":listing.unreadable}),
        plain,
    );
    // The listing still succeeds: it names each record it skipped, and how to repair it.
    output
        .notices
        .extend(listing.unreadable.iter().map(ToString::to_string));
    if !listing.unreadable.is_empty() {
        output.notices.push(UNREADABLE_REPAIR.into());
    }
    if let Some(publication) = publication {
        if let Some(detail) = publication["detail"]
            .as_str()
            .filter(|detail| !detail.is_empty())
        {
            output.notices.push(detail.into());
        }
        output.value["sharing"] = publication;
    }
    Ok(output)
}

pub(super) fn select_stream<'a>(
    records: &'a [streams::storage::records::RecordRead],
    reference: &str,
) -> Result<&'a Stream> {
    let streams: Vec<_> = records
        .iter()
        .filter_map(|read| match &read.record {
            Record::Stream(stream) => Some(stream),
            _ => None,
        })
        .collect();
    Ok(*resolve(&streams, reference, |stream| {
        (&stream.id, &stream.title, Some(&stream.owner_host_id))
    })?)
}

pub(super) fn update_record(
    opened: &OpenedRecords,
    stream: &Stream,
    title: Option<String>,
    intent: &StreamFields,
    defaults: TitleDefaults,
    mut read: impl FnMut(&Path) -> Result<String>,
    operation_id: impl FnOnce() -> String,
) -> Result<Output> {
    let mut changes = fields::stream(stream, intent, &mut read)?;
    if let Some(title) = title {
        changes.insert("title".into(), json!(title));
    }
    if changes.is_empty() {
        return Err(CliError::message(
            "update requires at least one field change",
        ));
    }
    save_stream_edit(opened, stream, changes, defaults, &operation_id())
}

pub(super) fn save_stream_edit(
    opened: &OpenedRecords,
    stream: &Stream,
    changes: Map<String, Value>,
    defaults: TitleDefaults,
    operation_id: &str,
) -> Result<Output> {
    finish_stream_save(
        opened,
        Some(stream.status),
        operation_id,
        application::save_user_edit(
            opened.records(),
            operation_id,
            &Record::Stream(stream.clone()),
            &changes,
            defaults,
        ),
    )
}

#[cfg(test)]
pub(super) fn apply(
    store: &impl streams::storage::records::RecordStore,
    captured: &str,
    source_id: &str,
    defaults: streams::model::TitleDefaults,
) -> Result<Output> {
    let update = agent::parse_record_update(captured, source_id, defaults)?;
    finish_save(
        update.operation_id(),
        application::save_record_update(store, &update),
    )
}

pub(super) fn apply_opened(
    opened: &OpenedRecords,
    source: &str,
    defaults: streams::model::TitleDefaults,
) -> Result<Output> {
    let update = agent::parse_record_update(source, opened.source_id(), defaults)?;
    let before_status = if update.target().kind() == streams::model::RecordKind::Stream {
        opened
            .records()
            .with_reader(|reader| reader.read(update.target()))
            .map_err(application::ApplicationError::from)?
            .and_then(|read| match read.record {
                Record::Stream(stream) => Some(stream.status),
                _ => None,
            })
    } else {
        None
    };
    finish_stream_save(
        opened,
        before_status,
        update.operation_id(),
        application::save_record_update(opened.records(), &update),
    )
}

fn finish_stream_save(
    opened: &OpenedRecords,
    before_status: Option<Status>,
    operation_id: &str,
    saved: application::Result<streams::storage::records::RecordSave>,
) -> Result<Output> {
    let after_record = saved.as_ref().ok().map(|s| s.current.record.clone());
    let mut output = finish_save(operation_id, saved)?;
    if output.accepted_save
        && let Some(before) = before_status
        && let Some(after) = &after_record
        && let Some(children) = application::lifecycle::cascade_stream_close_if_transitioned(
            opened,
            before,
            after,
            &uuid::Uuid::new_v4().to_string(),
        )
        && let Some(error) = children.completion_error
    {
        output.code = 2;
        output.notices.push(error.clone());
        output.value["completion_error"] = json!(error);
    }
    Ok(output)
}

pub(super) fn finish_save(
    operation_id: &str,
    saved: application::Result<streams::storage::records::RecordSave>,
) -> Result<Output> {
    match saved {
        Ok(saved) => Ok(finish_record(operation_id, saved)),
        Err(streams::application::ApplicationError::Storage(error)) if error.is_uncertain() => {
            Ok(Output::receipt(
                json!({
                    "operation_id":operation_id,
                    "save":{"local":"uncertain","domain":null,"detail":error.to_string()},
                }),
                false,
            ))
        }
        Err(error) => Err(error.into()),
    }
}

pub(super) fn finish_record(
    operation_id: &str,
    saved: streams::storage::records::RecordSave,
) -> Output {
    let ok = matches!(
        saved
            .outcome
            .get("outcome")
            .and_then(serde_json::Value::as_str),
        Some("changes" | "no-change")
    );
    let kind = match &saved.current.record {
        Record::Stream(_) => "stream",
        Record::Session(_) => "session",
        Record::Host(_) => "host",
    };
    let mut output = Output::receipt(
        json!({
            "operation_id":operation_id,
            "save":{"local":if saved.repeated {"unchanged"} else {"persisted"},"domain":saved.outcome},
            (kind):record_to_data(&saved.current.record),
        }),
        ok,
    );
    output.accepted_save = ok;
    if let Some(rows) = output.value["save"]["domain"]["reference_results"].as_array() {
        for row in rows {
            if matches!(row["outcome"].as_str(), Some("conflict" | "superseded")) {
                output.notices.push(format!(
                    "Reference not applied: {}; {}",
                    super::references::safe_reference_json(
                        &serde_json::from_value::<streams::model::ResourceKey>(row["key"].clone())
                            .expect("typed reference result key")
                    ),
                    super::references::reference_text(row["reason"].as_str().unwrap_or_default()),
                ));
            }
        }
        if !output.notices.is_empty() {
            output.code = 2;
        }
    }
    output
}

pub(super) fn sync(opened: &OpenedRecords) -> Result<Output> {
    let transport = RecordSync::open(opened.root(), opened.host().as_str())
        .map_err(application::ApplicationError::from)?;
    let result = transport.sync();
    let mut output = Output::receipt(result_data(&result)?, !result.needs_attention());
    output.accepted_save = true;
    Ok(output)
}

/// Explicit human validation/discovery, never a prerequisite for a local save.
pub(super) fn check(opened: &OpenedRecords, reference: Option<&str>) -> Result<Output> {
    let records = opened.inventory()?;
    let selected = reference
        .map(|reference| select_stream(&records, reference))
        .transpose()?;
    let streams: BTreeSet<_> = records
        .iter()
        .filter_map(|read| match &read.record {
            Record::Stream(stream) => Some((stream.owner_host_id.as_str(), stream.id.as_str())),
            _ => None,
        })
        .collect();
    let sessions: Vec<_> = records
        .iter()
        .filter_map(|read| match &read.record {
            Record::Session(session)
                if selected.is_none_or(|stream| {
                    session.host_id == stream.owner_host_id && session.stream_id == stream.id
                }) =>
            {
                Some(session)
            }
            _ => None,
        })
        .collect();
    for session in &sessions {
        if !streams.contains(&(session.host_id.as_str(), session.stream_id.as_str())) {
            return Err(ValidationError::new("Session's same-host Stream is missing").into());
        }
    }
    Ok(Output::value(
        json!({"valid":true,"streams":if selected.is_some(){1}else{streams.len()},"sessions":sessions.len()}),
    ))
}
