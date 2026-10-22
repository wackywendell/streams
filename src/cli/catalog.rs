//! Catalog command behavior independent of configuration, native frontends and text I/O.

#[cfg(test)]
mod tests;

use std::path::Path;

use serde_json::json;
use streams_temper::{
    codec::record_to_data,
    model::{Catalog, Record, Status, Stream},
    operations::{self, Actor},
    sync::{CatalogBackend, policy::SyncEvent},
};

use super::args::StreamFields;
use super::{
    CliError, Output, Result, fields, finish, fold, printable, read_catalog, resolve, result_data,
    save,
};

pub(super) fn list(
    backend: &dyn CatalogBackend,
    query: Option<&str>,
    status: &[Status],
    archived: bool,
    all: bool,
    refresh: bool,
) -> Result<Output> {
    let (catalog, sharing) = read_catalog(backend, refresh)?;
    let query = query.map(fold);
    let mut streams: Vec<_> = catalog
        .streams()
        .iter()
        .filter(|stream| {
            (all || stream.archived == archived)
                && (status.is_empty() || status.contains(&stream.status))
                && query.as_ref().is_none_or(|q| {
                    fold(&stream.title).contains(q)
                        || fold(stream.goal.as_deref().unwrap_or("")).contains(q)
                })
        })
        .collect();
    streams.sort_by_cached_key(|s| (fold(&s.title), s.id.clone()));
    let mut plain = String::from("ID  STATUS  TITLE");
    for stream in &streams {
        plain.push_str(&format!(
            "\n{}  {}  {}",
            stream.id,
            stream.status,
            printable(&stream.title)
        ));
    }
    let data: Vec<_> = streams
        .iter()
        .map(|s| record_to_data(&Record::Stream((*s).clone())))
        .collect();
    let mut output = Output::plain(json!({"streams":data,"sharing":sharing}), plain);
    let sharing = &output.value["sharing"];
    if let Some(detail) = sharing["detail"]
        .as_str()
        .filter(|detail| !detail.is_empty())
    {
        output.notices.push(detail.to_owned());
    }
    if sharing["conflicts"]
        .as_array()
        .is_some_and(|conflicts| !conflicts.is_empty())
    {
        output.notices.push("catalog conflicts: showing local records; use list --json or show for confirmed alternatives".into());
    }
    Ok(output)
}

pub(super) fn show(
    backend: &dyn CatalogBackend,
    refresh: bool,
    select: impl for<'a> FnOnce(&'a Catalog) -> Result<&'a Stream>,
) -> Result<Output> {
    let (catalog, sharing) = read_catalog(backend, refresh)?;
    let stream = select(&catalog)?;
    let sessions: Vec<_> = catalog
        .sessions()
        .iter()
        .filter(|s| s.stream_id == stream.id)
        .collect();
    Ok(Output::value(
        json!({"stream":stream,"sessions":sessions,"sharing":sharing}),
    ))
}

pub(super) fn update(
    backend: &dyn CatalogBackend,
    select: impl for<'a> FnOnce(&'a Catalog) -> Result<&'a Stream>,
    title: Option<String>,
    fields: &StreamFields,
    mut read: impl FnMut(&Path) -> Result<String>,
    operation_id: impl FnOnce() -> String,
) -> Result<Output> {
    let snapshot = backend.snapshot()?;
    let stream = select(&snapshot.catalog)?;
    let mut changes = fields::stream(stream, fields, &mut read)?;
    if let Some(title) = title {
        changes.insert("title".into(), json!(title));
    }
    if changes.is_empty() {
        return Err(CliError::message(
            "update requires at least one field change",
        ));
    }
    let operation = operations::update(
        operation_id(),
        &Record::Stream(stream.clone()),
        &changes,
        Actor::User,
        None,
        vec![],
    )?;
    let saved = save(backend, &operation)?;
    finish(backend, &operation, saved, SyncEvent::Stream, None)
}

pub(super) fn archive(
    backend: &dyn CatalogBackend,
    select: impl for<'a> FnOnce(&'a Catalog) -> Result<&'a Stream>,
    archived: bool,
    operation_id: impl FnOnce() -> String,
) -> Result<Output> {
    let catalog = backend.snapshot()?.catalog;
    let stream = select(&catalog)?;
    let operation = operations::update(
        operation_id(),
        &Record::Stream(stream.clone()),
        json!({"archived":archived})
            .as_object()
            .expect("archive fields"),
        Actor::User,
        None,
        vec![],
    )?;
    let saved = save(backend, &operation)?;
    finish(backend, &operation, saved, SyncEvent::Stream, None)
}

pub(super) fn sync(backend: &dyn CatalogBackend, event: SyncEvent) -> Result<Output> {
    let result = backend.sync(event);
    let ok = !result.needs_attention();
    Ok(Output::receipt(result_data(&result)?, ok))
}

pub(super) fn check(backend: &dyn CatalogBackend, reference: Option<&str>) -> Result<Output> {
    let catalog = backend.snapshot()?.catalog;
    let stream = reference
        .map(|reference| resolve(catalog.streams(), reference, |s| (&s.id, &s.title, None)))
        .transpose()?;
    Ok(Output::value(
        json!({"valid":true,"streams":if stream.is_some(){1}else{catalog.streams().len()},"sessions":catalog.sessions().iter().filter(|session|stream.is_none_or(|stream|session.stream_id==stream.id)).count()}),
    ))
}
