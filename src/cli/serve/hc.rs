//! Helper-owned hc source lifecycle. Catalog work remains independent of this route.
use super::{App, CliError, Result};
use std::{sync::atomic::AtomicBool, time::Instant};
use streams::{
    cancellation::Cancellation,
    frontend::{
        EndpointId,
        hc::{HcClient, HcEndpoint, processes::OwnerProcesses},
    },
    integration::{ConversationId, Effect},
    model::{Record, RecordKind, Session},
    runtime::{
        RuntimeError,
        conversations::{ConversationRoots, Harness, conversation_file},
        frontends::HcFrontend,
        hc::sources::SourceRegistration,
    },
    storage::records::{RecordAddress, RecordStore},
};

/// Retain only the exact SDK-supplied file for an already enrolled Session.
/// Missing files before Pi persists them are not failures and never trigger discovery.
pub(super) fn remember_locator(
    app: &App,
    context: &streams::child_protocol::PiRequestContext,
) -> Result<()> {
    let Some(file) = context.session_file.as_deref() else {
        return Ok(());
    };
    let locator = match conversation_file(
        Harness::Pi,
        &context.conversation.native_id,
        Some(file),
        &ConversationRoots::default(),
    ) {
        Ok(path) => path,
        Err(RuntimeError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(());
        }
        Err(_) => return Err(CliError::message("Pi session file could not be verified")),
    };
    let opened = super::super::application::open_records(&app.config)?;
    let Some(session) = pi_session(&opened, &context.conversation.native_id)? else {
        return Ok(());
    };
    opened
        .locations()?
        .remember_transcript(&session.stream_id, &session.id, &locator)
        .map_err(|error| {
            if error.is_uncertain() {
                CliError::message("Pi session file retention is uncertain; inspect before retrying")
            } else {
                CliError::message("Pi session file could not be retained")
            }
        })?;
    Ok(())
}

/// The Session saved for this Pi conversation, checked against the identity it was read by.
fn pi_session(
    opened: &super::super::application::OpenedRecords,
    native_id: &str,
) -> Result<Option<Session>> {
    let host = opened.host().as_str();
    let id = Session::id_for(host, "pi", native_id);
    let address = RecordAddress::new(host, RecordKind::Session, &id)
        .map_err(|_| CliError::message("Pi Session is unavailable"))?;
    let found = opened
        .records()
        .with_reader(|reader| reader.read(&address))
        .map_err(|_| CliError::message("Pi Session is unavailable"))?;
    let Some(found) = found else {
        return Ok(None);
    };
    let Record::Session(session) = found.record else {
        return Err(CliError::message("Pi Session is unavailable"));
    };
    if session.host_id != host || session.harness != "pi" || session.harness_session_id != native_id
    {
        return Err(CliError::message("Pi Session identity does not match"));
    }
    Ok(Some(session))
}

pub(super) struct Source {
    socket: HcEndpoint,
    endpoint: EndpointId,
    registration: SourceRegistration,
}
impl Source {
    pub fn register(
        app: &App,
        session_id: &str,
        stopping: Cancellation,
        cancelled: &AtomicBool,
    ) -> Result<Self> {
        let terminal = app
            .environment
            .get("HC_TERMINAL_ID")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| CliError::message("hc terminal context is unavailable"))?;
        let socket = HcEndpoint::resolve(&app.environment, &app.cwd)
            .map_err(|_| CliError::message("hc source endpoint is unavailable"))?;
        let mut client = HcClient::connect(socket.clone(), None, 5.0, Some(stopping))
            .map_err(|_| CliError::message("hc source connection is unavailable"))?;
        let helper = OwnerProcesses::new()
            .and_then(|observer| observer.observe(std::process::id()))
            .map_err(|_| CliError::message("hc source process facts are unavailable"))?;
        let conversation = ConversationId {
            harness: "pi".into(),
            native_id: session_id.into(),
        };
        let registration = match SourceRegistration::register(
            &mut client,
            terminal,
            conversation,
            helper,
            cancelled,
        ) {
            Effect::Applied { value } => value,
            Effect::NotApplied { .. } => {
                return Err(CliError::message(
                    "hc Pi source registration was not applied",
                ));
            }
            Effect::Uncertain { .. } => {
                return Err(CliError::message(
                    "hc Pi source registration is uncertain; no retry was attempted",
                ));
            }
        };
        Ok(Self {
            socket,
            endpoint: client.endpoint().clone(),
            registration,
        })
    }

    /// hc as this source's caller: its terminal, on the endpoint it registered with.
    pub fn frontend(
        &self,
        app: &App,
        scope: streams::integration::WorkScope,
        stopping: Cancellation,
    ) -> HcFrontend {
        HcFrontend::registered(
            app.caller(scope, 5.0),
            self.registration.terminal(),
            self.socket.clone(),
            self.endpoint.clone(),
            stopping,
        )
    }

    pub fn retire(self, deadline: Instant) -> Result<()> {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_secs_f64();
        if remaining <= 0.0 {
            return Err(CliError::message("hc Pi source retirement timed out"));
        }
        let cancellation = Cancellation::new()?;
        let mut client = HcClient::connect(
            self.socket,
            Some(&self.endpoint),
            remaining,
            Some(cancellation),
        )
        .map_err(|_| CliError::message("hc Pi source retirement is unavailable or timed out"))?;
        match self.registration.retire(&mut client) {
            Effect::Applied { .. } => Ok(()),
            Effect::NotApplied { .. } => {
                Err(CliError::message("hc Pi source retirement was not applied"))
            }
            Effect::Uncertain { .. } => Err(CliError::message(
                "hc Pi source retirement is uncertain; no retry was attempted",
            )),
        }
    }
}

pub(super) struct SessionResolution {
    pub scope: streams::integration::WorkScope,
    pub session: Option<Session>,
    pub desired_pi_name: Option<String>,
}

pub(super) fn resolve_session(
    app: &App,
    session_id: &str,
    _plugin_root: &std::path::Path,
    remember_directory: bool,
) -> Result<SessionResolution> {
    let opened = super::super::application::open_records(&app.config)?;
    let host = opened.host().as_str();
    let scope = streams::integration::WorkScope {
        catalog: opened.source_id().into(),
        host: host.into(),
    };
    let Some(session) = pi_session(&opened, session_id)? else {
        return Ok(SessionResolution {
            scope,
            session: None,
            desired_pi_name: None,
        });
    };
    // Best effort: a directory that cannot be remembered leaves the Session as it is.
    if remember_directory
        && let Ok(directory) = streams::runtime::checkouts::canonical_directory(&app.cwd)
    {
        let _ = super::super::application::remember_directory(
            &opened,
            &session.stream_id,
            &session.id,
            &directory,
        );
    }
    let desired_pi_name = opened
        .settings()
        .pi
        .session_naming
        .then(|| session.title.clone());
    Ok(SessionResolution {
        scope,
        session: Some(session),
        desired_pi_name,
    })
}
