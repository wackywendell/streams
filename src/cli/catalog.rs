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
    sync::records::RecordSync,
};

pub(super) fn list_records(
    opened: &streams::application::OpenedRecords,
    query: Option<&str>,
    status: &[Status],
    archived: bool,
    all: bool,
    publication: Option<serde_json::Value>,
) -> Result<Output> {
    let query = query.map(fold);
    let mut streams: Vec<_> = opened
        .inventory()?
        .into_iter()
        .filter_map(|read| match read.record {
            Record::Stream(stream) => Some(stream),
            _ => None,
        })
        .filter(|stream| {
            (all || stream.archived == archived)
                && (status.is_empty() || status.contains(&stream.status))
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
    let mut output = Output::plain(json!({"streams":streams}), plain);
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
    finish_save(
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
    output
}

pub(super) fn sync(opened: &OpenedRecords) -> Result<Output> {
    let transport = RecordSync::open(opened.root(), opened.host().as_str())
        .map_err(application::ApplicationError::from)?;
    let result = transport.sync();
    Ok(Output::receipt(
        result_data(&result)?,
        !result.needs_attention(),
    ))
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
