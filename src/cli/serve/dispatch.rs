use serde_json::Value;
use streams::{
    child_protocol::{CaptureTarget, DomainObject, LifecycleAction, PreviewAction, Request},
    config::ConfigKey,
    runtime::conversations::Harness,
};

use super::super::{App, CliError, Output, Result, agent, application, catalog, json};

pub(super) fn preview(
    catalog: DomainObject,
    action: PreviewAction,
    request: DomainObject,
) -> Result<Output> {
    Ok(Output::value(agent::project(
        &Value::Object(catalog),
        action,
        Value::Object(request),
    )?))
}

pub(super) fn execute(app: &App, session_id: &str, request: Request) -> Result<Output> {
    match request {
        Request::Context { stream_id } => app
            .agent_observation(Harness::Pi, session_id, stream_id.as_deref())?
            .output(),
        Request::DiscardUnreadableCard {} => {
            let discarded = application::open_records(&app.config)?
                .discard_unreadable_card(Harness::Pi, session_id)?;
            let mut output = Output::value(json!(discarded));
            output.accepted_save = discarded.session.is_some() || discarded.stream.is_some();
            Ok(output)
        }
        Request::CaptureInputs { target } => match target {
            CaptureTarget::Inventory {} => app.capture_inputs(None, None),
            CaptureTarget::Stream { stream_id } => app.capture_inputs(Some(&stream_id), None),
            CaptureTarget::Workspace { workspace_id } => {
                app.capture_inputs(None, Some(&workspace_id))
            }
        },
        Request::Preview {
            catalog,
            action,
            request,
        } => preview(catalog, action, request),
        Request::Enroll { request } => app.enroll_request(Value::Object(request)),
        Request::Apply { request } => app.apply_request(Value::Object(request)),
        Request::ListStreams {} => {
            let opened = application::open_records(&app.config)?;
            let cutoff_ms = streams::operations::decay_cutoff_ms(
                opened.settings().lifecycle.decay_days,
                super::super::now_ms(),
            );
            catalog::list_records(&opened, None, &[], false, false, cutoff_ms, None)
        }
        Request::SetTitleDefault { target, mode } => app.set_title_default(target, mode),
        Request::GetConfig {} => Ok(Output::value(
            json!({"config": app.config.extension_snapshot()?}),
        )),
        Request::SetConfig { key, value } => {
            let key: ConfigKey = key
                .parse()
                .map_err(|_| CliError::message(format!("unknown streams config key: {key}")))?;
            match key {
                ConfigKey::StreamNaming | ConfigKey::SessionNaming => {
                    let target = if key == ConfigKey::StreamNaming {
                        streams::child_protocol::TitleTarget::Stream
                    } else {
                        streams::child_protocol::TitleTarget::Session
                    };
                    let mode = value.parse().map_err(|_| {
                        CliError::message(format!("{key} must be automatic or manual"))
                    })?;
                    app.set_title_default(target, mode)?;
                }
                _ => app.config.set(key, &value)?,
            }
            let mut output = Output::value(json!({"config": app.config.extension_snapshot()?}));
            output.accepted_save = application::open_records(&app.config).is_ok();
            Ok(output)
        }
        Request::UnsetConfig { key } => {
            let key: ConfigKey = key
                .parse()
                .map_err(|_| CliError::message(format!("unknown streams config key: {key}")))?;
            app.config.unset(key)?;
            let mut output = Output::value(json!({"config": app.config.extension_snapshot()?}));
            output.accepted_save = application::open_records(&app.config).is_ok();
            Ok(output)
        }
        Request::CardSettings {} => app.card_settings(session_id),
        Request::SetCardMode { target, mode } => app.set_card_mode(session_id, target, mode),
        Request::Sync {} => app.sync_records(),
        Request::Lifecycle { action } => {
            let opened = application::open_records(&app.config)?;
            let update = match action {
                LifecycleAction::Resume => {
                    application::lifecycle::resume_session(&opened, Harness::Pi, session_id)?
                }
                LifecycleAction::Park => {
                    application::lifecycle::park_session(&opened, Harness::Pi, session_id)?
                }
            };
            let mut output = Output::value(update.to_json());
            output.accepted_save = !update.saves.is_empty();
            if let Some(error) = update.completion_error {
                output.code = 2;
                output.notices.push(error);
            }
            Ok(output)
        }
        Request::Artifact { request } => app.artifact_request(request, Some(session_id)),
        Request::SessionStarted {}
        | Request::Attach {}
        | Request::OpenCard {}
        | Request::CloseCard {} => Err(CliError::message(
            "native lifecycle requests require the native queue",
        )),
    }
}
