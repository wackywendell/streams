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
    runtime::hc::sources::SourceRegistration,
};

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
