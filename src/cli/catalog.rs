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
    sync::{
        self, RemoteUpdate,
        outcomes::{LocalStatus, Result as SyncResult, SharingStatus},
    },
};

use super::{App, CliError, Output, Result, args::CatalogCommand, result_data, select};

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
                label,
            } => {
                let opened = application::initialize_records_with_label(
                    &self.config,
                    sync,
                    remote.as_deref(),
                    &branch,
                    label.as_deref(),
                )?;
                let host_label = application::read_host_label(&opened, opened.host().as_str())?;
                let value = json!({
                    "catalog": opened.root(),
                    "host_id": opened.host().as_str(),
                    "host_label": host_label,
                    "sync": sync,
                });
                let plain = format!(
                    "initialized catalog at {} (host {} ({}), sync {})",
                    opened.root().display(),
                    opened.host().as_str(),
                    host_label.as_deref().unwrap_or("-"),
                    sync
                );
                let output = Output::plain(value, plain);
                Ok(output)
            }
            CatalogCommand::Label { label } => {
                let opened = application::open_records(&self.config)?;
                if let Some(new_label) = label {
                    let (host, written) = application::set_host_label(&opened, &new_label)?;
                    let value = json!({
                        "host_id": host.id,
                        "label": host.label,
                        "written": written,
                    });
                    let mut output = Output::plain(value, host.label);
                    output.notify_ui = written;
                    output.wrote_catalog = written;
                    Ok(output)
                } else {
                    let current = application::read_host_label(&opened, opened.host().as_str())?;
                    let value = json!({
                        "host_id": opened.host().as_str(),
                        "label": current,
                    });
                    let plain = current.unwrap_or_else(|| "-".into());
                    Ok(Output::plain(value, plain))
                }
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
                let output = Output::plain(value, plain);
                Ok(output)
            }
            CatalogCommand::Sync {
                background,
                if_configured,
                timeout,
            } => {
                if let Some(secs) = timeout
                    && (!secs.is_finite() || secs <= 0.0)
                {
                    return Err(CliError::Usage(
                        "--timeout must be a positive number of seconds".into(),
                    ));
                }
                if background || if_configured {
                    let settings = self.config.load()?;
                    if !self.config.host_path().exists()
                        || !settings
                            .catalog_root
                            .join(".catalog-local/records.json")
                            .exists()
                    {
                        return noop_sync_output(background);
                    }
                }
                let opened = application::open_records(&self.config)?;
                sync_catalog(&opened, background, if_configured, timeout)
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
                output.wrote_catalog = summary.closed_sessions > 0;
                if !summary.errors.is_empty() {
                    output.code = 3;
                    output.notices.extend(summary.errors);
                }
                Ok(output)
            }
        }
    }
}

fn noop_sync_output(background: bool) -> Result<Output> {
    if background {
        return Ok(Output::silent());
    }
    let result = SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Unchanged);
    let value = result_data(&result)?;
    Ok(Output::plain(
        value,
        "sync skipped (catalog uninitialized or local-only)".to_owned(),
    ))
}

pub(super) fn sync_catalog(
    opened: &OpenedRecords,
    background: bool,
    if_configured: bool,
    timeout: Option<f64>,
) -> Result<Output> {
    let backend = sync::open_backend(opened.root(), opened.host().as_str())
        .map_err(application::ApplicationError::from)?;
    if (background || if_configured) && backend.strategy() == Strategy::None {
        return noop_sync_output(background);
    }
    let result = if background {
        match backend.try_sync_with_timeout(timeout) {
            Some(result) => result,
            None => return Ok(Output::silent()),
        }
    } else {
        backend.sync_with_timeout(timeout)
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
    output.notify_ui = result.local == LocalStatus::Committed;
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
