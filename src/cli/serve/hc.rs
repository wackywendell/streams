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
    let host = opened.host().as_str();
    let id = Session::id_for(host, "pi", &context.conversation.native_id);
    let address = RecordAddress::new(host, RecordKind::Session, &id)
        .map_err(|_| CliError::message("Pi Session is unavailable"))?;
    let found = opened
        .records()
        .with_reader(|reader| reader.read(&address))
        .map_err(|_| CliError::message("Pi Session is unavailable"))?;
    let Some(found) = found else {
        return Ok(());
    };
    let Record::Session(session) = found.record else {
        return Err(CliError::message("Pi Session is unavailable"));
    };
    if session.host_id != host
        || session.harness != "pi"
        || session.harness_session_id != context.conversation.native_id
    {
        return Err(CliError::message("Pi Session identity does not match"));
    }
    app.locations(opened.root(), host)?
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

pub(super) struct Source {
    socket: HcEndpoint,
    endpoint: EndpointId,
    registration: SourceRegistration,
}
impl Source {
    pub fn register(
        app: &App,
        session_id: &str,
        sdk_pid: u32,
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
        let observer = OwnerProcesses::new()
            .map_err(|_| CliError::message("hc source process facts are unavailable"))?;
        let conversation = ConversationId {
            harness: "pi".into(),
            native_id: session_id.into(),
        };
        let registration = match SourceRegistration::register(
            &mut client,
            terminal,
            conversation,
            sdk_pid,
            std::process::id(),
            &observer,
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
