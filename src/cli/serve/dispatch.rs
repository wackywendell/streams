use streams_temper::{
    child_protocol::{CaptureTarget, DomainObject, PreviewAction, Request},
    runtime::conversations::Harness,
};

use super::super::{App, CliError, Output, Result, agent, application, catalog, json};

pub(super) fn preview(
    catalog: DomainObject,
    action: PreviewAction,
    request: DomainObject,
) -> Result<Output> {
    Ok(Output::value(agent::preview(
        &json!({"catalog":catalog,"action":action,"request":request}).to_string(),
    )?))
}

pub(super) fn execute(app: &App, session_id: &str, request: Request) -> Result<Output> {
    match request {
        Request::Context { stream_id } => app
            .agent_observation_from_native(
                Harness::Pi,
                session_id,
                stream_id.as_deref(),
                false,
                false,
                None,
            )?
            .output(app.config.load()?.naming),
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
        Request::Enroll { request } => app.enroll_request(&serde_json::to_string(&request)?),
        Request::Apply { request } => app.apply_request(&serde_json::to_string(&request)?),
        Request::ListStreams {} => {
            let opened = application::open(&app.config)?;
            catalog::list(opened.backend(), None, &[], false, false, false)
        }
        Request::SetTitleDefault { target, mode } => app.set_title_default(target, mode),
        Request::CaptureWorkspace { stream_id } => app.capture_workspace_stream_id(&stream_id),
        Request::Sync { event } => {
            let opened = application::open(&app.config)?;
            catalog::sync(opened.backend(), event)
        }
        Request::SessionStarted {}
        | Request::Attach {}
        | Request::OpenCard {}
        | Request::CloseCard {} => Err(CliError::message(
            "native lifecycle requests require the native queue",
        )),
    }
}
