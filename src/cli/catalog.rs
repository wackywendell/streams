//! Catalog actions over admitted records; publication is a separate transport request.

#[cfg(test)]
mod tests;

use super::args::StreamFields;
use super::{CliError, Output, Result, fields, printable, resolve, result_data};
use serde_json::json;
use std::{collections::BTreeSet, path::Path};
use streams::{
    agent,
    application::{self, OpenedRecords},
    codec::record_to_data,
    model::{Record, Status, Stream, TitleDefaults, ValidationError},
    operations::{
        DomainOutcome, EditOutcome,
        patches::{Patch, StreamPatch},
    },
    storage::records::RecordSave,
    sync::records::RecordSync,
};

pub(super) fn list_records(
    opened: &streams::application::OpenedRecords,
    query: Option<&str>,
    status: &[Status],
    closed: bool,
    all: bool,
    cutoff_ms: Option<u64>,
    publication: Option<serde_json::Value>,
) -> Result<Output> {
    let listing = application::list_streams(
        opened,
        application::StreamFilter {
            query,
            status,
            closed,
            all,
        },
        cutoff_ms,
    )?;
    let streams = listing.streams;
    let mut plain = String::from("HOST/ID  STATUS  TITLE");
    for entry in &streams {
        plain.push_str(&format!(
            "\n{}/{}  {}  {}",
            entry.stream.owner_host_id,
            entry.stream.id,
            entry.status,
            printable(&entry.stream.title)
        ));
    }
    let mut output = Output::plain(
        json!({"streams":streams,"unreadable":listing.unreadable}),
        plain,
    );
    // The listing still succeeds: it names each record it skipped, and how to repair it.
    output.notices.extend(
        listing
            .unreadable
            .iter()
            .map(|record| format!("{record}\n  {}", record.repair)),
    );
    if listing
        .unreadable
        .iter()
        .any(|record| record.host != opened.host().as_str())
    {
        output.notices.push(
            "/streams reset repairs only this host's records; repair another host's records on that host.".into(),
        );
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
) -> Result<Output> {
    let mut patch = fields::stream(stream, intent, &mut read)?;
    if title.is_some() {
        patch.title = title;
    }
    if patch.is_empty() {
        return Err(CliError::message(
            "update requires at least one field change",
        ));
    }
    save_stream_edit(opened, stream, patch, defaults)
}

pub(super) fn save_stream_edit(
    opened: &OpenedRecords,
    stream: &Stream,
    patch: StreamPatch,
    defaults: TitleDefaults,
) -> Result<Output> {
    Ok(finish_record(application::save_stream_edit(
        opened.records(),
        stream,
        patch,
        defaults,
    )?))
}

#[cfg(test)]
pub(super) fn apply(
    store: &impl streams::storage::records::RecordStore,
    captured: &str,
    source_id: &str,
    defaults: streams::model::TitleDefaults,
) -> Result<Output> {
    let update =
        agent::parse_record_update(streams::codec::load_json(captured)?, source_id, defaults)?;
    Ok(finish_record(application::save_record_update(
        store, &update,
    )?))
}

pub(super) fn apply_opened(opened: &OpenedRecords, request: serde_json::Value) -> Result<Output> {
    let update = agent::parse_record_update(request, opened.source_id(), opened.settings().naming)?;
    Ok(finish_record(application::save_record_update(
        opened.records(),
        &update,
    )?))
}

/// A created record reports like an edit with nothing refused.
pub(super) fn finish_created(saved: RecordSave<DomainOutcome>) -> Output {
    finish_record(RecordSave {
        current: saved.current,
        written: saved.written,
        outcome: EditOutcome {
            outcome: saved.outcome,
            conflicts: Vec::new(),
            reason: String::new(),
            reference_results: None,
        },
    })
}

pub(super) fn finish_record(saved: RecordSave<EditOutcome>) -> Output {
    let ok = saved.outcome.accepted();
    let kind = saved.current.record.kind().as_ref().to_owned();
    let mut output = Output::receipt(
        json!({
            "save":{"local":saved.local(),"domain":saved.outcome},
            (kind):record_to_data(&saved.current.record),
        }),
        ok,
    );
    output.accepted_save = saved.written;
    for row in saved.outcome.reference_results.iter().flatten() {
        if matches!(
            row.outcome,
            DomainOutcome::Conflict | DomainOutcome::Superseded
        ) {
            output.notices.push(format!(
                "Reference not applied: {}; {}",
                super::references::safe_reference_json(&row.key),
                super::references::reference_text(&row.reason),
            ));
        }
    }
    if !output.notices.is_empty() {
        output.code = 2;
    }
    output
}

pub(super) fn sync(opened: &OpenedRecords) -> Result<Output> {
    let transport = RecordSync::open(opened.root(), opened.host().as_str())
        .map_err(application::ApplicationError::from)?;
    let result = transport.sync();
    let mut output = Output::receipt(result_data(&result)?, !result.needs_attention());
    output.accepted_save = result.local == streams::sync::outcomes::LocalStatus::Committed;
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
