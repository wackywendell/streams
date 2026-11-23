//! `streams catalog` subcommand handlers (`init`, `sync`, `check`, `tidy`).

use std::{
    collections::BTreeSet,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use streams::{
    application::{self, OpenedRecords},
    model::{Record, ValidationError},
    sync::{self, RemoteUpdate, outcomes::LocalStatus},
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
                let mut output = Output::plain(value, plain);
                output.trigger_auto_sync = false;
                Ok(output)
            }
            CatalogCommand::Configure {
                sync,
                remote,
                clear_remote,
                branch,
            } => {
                let remote_update = if clear_remote {
                    RemoteUpdate::Clear
                } else if let Some(url) = remote {
                    RemoteUpdate::Set(url)
                } else {
                    RemoteUpdate::Keep
                };
                let (opened, strategy, configured_remote, configured_branch) =
                    application::configure_records(
                        &self.config,
                        sync,
                        remote_update,
                        branch.as_deref(),
                    )?;
                let value = json!({
                    "catalog": opened.root(),
                    "host_id": opened.host().as_str(),
                    "sync": strategy,
                    "remote": configured_remote,
                    "branch": configured_branch,
                });
                let plain = match (&configured_remote, &configured_branch) {
                    (Some(remote), Some(branch)) => format!(
                        "configured catalog at {} (sync {strategy}, remote {remote}, branch {branch})",
                        opened.root().display()
                    ),
                    (None, Some(branch)) => format!(
                        "configured catalog at {} (sync {strategy}, local-only, branch {branch})",
                        opened.root().display()
                    ),
                    _ => format!(
                        "configured catalog at {} (sync {strategy})",
                        opened.root().display()
                    ),
                };
                let mut output = Output::plain(value, plain);
                output.trigger_auto_sync = false;
                Ok(output)
            }
            CatalogCommand::Sync { background } => {
                let opened = application::open_records(&self.config)?;
                sync_catalog(&opened, background)
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

pub(super) fn sync_catalog(opened: &OpenedRecords, background: bool) -> Result<Output> {
    let backend = sync::open_backend(opened.root(), opened.host().as_str())
        .map_err(application::ApplicationError::from)?;
    let result = if background {
        match backend.try_sync() {
            Some(result) => result,
            None => {
                let mut output = Output::silent();
                output.trigger_auto_sync = false;
                return Ok(output);
            }
        }
    } else {
        backend.sync()
    };
    let ok = !result.needs_attention();
    let value = result_data(&result)?;
    let plain = if ok {
        format!(
            "sync complete (local: {}, sharing: {})",
            result.local, result.sharing
        )
    } else if result.detail.is_empty() {
        format!(
            "sync needs attention (local: {}, sharing: {})",
            result.local, result.sharing
        )
    } else {
        format!(
            "sync needs attention (local: {}, sharing: {}): {}",
            result.local, result.sharing, result.detail
        )
    };
    let mut output = Output::plain(value, plain);
    if !ok {
        output.code = 3;
    }
    output.accepted_save = true;
    output.wrote = result.local == LocalStatus::Committed;
    output.trigger_auto_sync = false;
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
    let backend = sync::open_backend(opened.root(), opened.host().as_str())
        .map_err(application::ApplicationError::from)?;
    let transport = backend.check();
    let last_sync =
        sync::read_last_sync(opened.root()).map_err(application::ApplicationError::from)?;
    let valid = transport.issues.is_empty();
    let value = json!({
        "valid": valid,
        "streams": stream_count,
        "sessions": sessions.len(),
        "sync": transport,
        "last_sync": last_sync,
    });
    let mut plain = if valid {
        format!(
            "catalog valid ({stream_count} stream(s), {} session(s))",
            sessions.len()
        )
    } else {
        format!(
            "catalog transport needs attention ({stream_count} stream(s), {} session(s))",
            sessions.len()
        )
    };
    let mut output = Output::plain(value, plain.clone());
    if let (Some(remote), Some(effective)) = (&transport.remote, &transport.effective_remote)
        && remote != effective
    {
        plain.push_str(&format!(
            "\nnote: Git rewrites configured remote {remote} to {effective}"
        ));
        output.plain = Some(plain.clone());
    }
    if !transport.issues.is_empty() {
        for issue in &transport.issues {
            plain.push_str(&format!("\ntransport issue: {issue}"));
            output.notices.push(format!("warning: {issue}"));
        }
        output.plain = Some(plain.clone());
        output.code = 3;
    }
    if let Some(state) = &last_sync
        && state.needs_attention
    {
        let detail = if state.detail.is_empty() {
            format!("local: {}, sharing: {}", state.local, state.sharing)
        } else {
            format!(
                "local: {}, sharing: {}: {}",
                state.local, state.sharing, state.detail
            )
        };
        plain.push_str(&format!("\nlast sync needs attention ({detail})"));
        output.plain = Some(plain);
        output.code = 3;
        output.notices.push(format!(
            "warning: last catalog sync needs attention ({detail}); run `streams catalog sync` to inspect"
        ));
    }
    Ok(output)
}
