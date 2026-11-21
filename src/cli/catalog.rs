//! `streams catalog` subcommand handlers (`init`, `sync`, `check`, `tidy`).

use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use streams::{
    application::{self, OpenedRecords},
    config::Strategy,
    model::{Record, ValidationError},
    sync::records::RecordSync,
};

use super::{App, Output, Result, args::CatalogCommand, result_data, select};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl App {
    pub(super) fn catalog_cmd(&self, command: CatalogCommand) -> Result<Output> {
        match command {
            CatalogCommand::Init {
                sync,
                remote,
                branch,
            } => {
                if sync == Strategy::Git && remote.is_none() {
                    return Err(super::CliError::Usage(
                        "--remote is required when --sync git".into(),
                    ));
                }
                let opened = application::initialize_records(
                    &self.config,
                    sync,
                    remote.as_deref(),
                    &branch,
                )?;
                let value = json!({
                    "catalog": opened.root(),
                    "host_id": opened.host().as_str(),
                    "sync": sync,
                });
                let plain = format!(
                    "initialized catalog at {} (host {}, sync {})",
                    opened.root().display(),
                    opened.host().as_str(),
                    sync
                );
                Ok(Output::plain(value, plain))
            }
            CatalogCommand::Sync => {
                let opened = application::open_records(&self.config)?;
                sync(&opened)
            }
            CatalogCommand::Check { stream } => {
                let opened = application::open_records(&self.config)?;
                check(&opened, stream.as_deref())
            }
            CatalogCommand::Tidy { days } => {
                let opened = application::open_records(&self.config)?;
                let decay_days = days.unwrap_or(opened.settings().lifecycle.decay_days);
                let summary =
                    application::lifecycle::decay_host_records(&opened, decay_days, now_ms())?;
                let plain = format!(
                    "closed {} session(s) (decay_days={decay_days})",
                    summary.closed_sessions
                );
                let mut output = Output::plain(
                    json!({
                        "decay_days": decay_days,
                        "closed_sessions": summary.closed_sessions,
                        "errors": summary.errors,
                    }),
                    plain,
                );
                let wrote = summary.closed_sessions > 0;
                output.accepted_save = wrote;
                output.wrote = wrote;
                if !summary.errors.is_empty() {
                    output.code = 3;
                    output.notices.extend(summary.errors);
                }
                Ok(output)
            }
        }
    }
}

pub(super) fn sync(opened: &OpenedRecords) -> Result<Output> {
    let transport = RecordSync::open(opened.root(), opened.host().as_str())
        .map_err(application::ApplicationError::from)?;
    let result = transport.sync();
    let ok = !result.needs_attention();
    let value = result_data(&result)?;
    let plain = if ok {
        format!(
            "sync complete (local: {:?}, sharing: {:?})",
            result.local, result.sharing
        )
    } else {
        format!(
            "sync needs attention (local: {:?}, sharing: {:?})",
            result.local, result.sharing
        )
    };
    let mut output = Output::plain(value, plain);
    if !ok {
        output.code = 3;
    }
    output.accepted_save = true;
    output.wrote = result.local == streams::sync::outcomes::LocalStatus::Committed;
    Ok(output)
}

/// Explicit human validation/discovery, never a prerequisite for a local save.
pub(super) fn check(opened: &OpenedRecords, selector: Option<&str>) -> Result<Output> {
    let records = opened.inventory()?;
    let selected = selector
        .map(|sel| select::select_stream(opened, sel))
        .transpose()?;
    let streams: BTreeSet<_> = records
        .iter()
        .filter_map(|read| match read {
            Record::Stream(stream) => Some((stream.host_id.as_str(), stream.id.as_str())),
            _ => None,
        })
        .collect();
    let sessions: Vec<_> = records
        .iter()
        .filter_map(|read| match read {
            Record::Session(session)
                if selected.as_ref().is_none_or(|stream| {
                    session.host_id == stream.host_id && session.stream_id == stream.id
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
    let stream_count = if selected.is_some() { 1 } else { streams.len() };
    let value = json!({
        "valid": true,
        "streams": stream_count,
        "sessions": sessions.len(),
    });
    let plain = format!(
        "catalog valid ({stream_count} stream(s), {} session(s))",
        sessions.len()
    );
    Ok(Output::plain(value, plain))
}
