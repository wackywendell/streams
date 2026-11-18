//! The one interface the CLI and the Pi child use for every frontend.
//!
//! [`FrontendId`] only selects an implementation in [`connect`]; behaviour lives in the
//! implementations. Each implementation connects on first use and observes at most once,
//! so a command that only reports a save never touches a frontend that ignores saves.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use serde::Serialize;

use super::{
    Result, RuntimeError,
    native_markers::selected_stream,
    workspace::{OpenResult, SelectedConversation, WorkspaceRuntime, running},
};
use crate::{
    cancellation::Cancellation,
    frontend::{
        EndpointId, FrontendError, FrontendId,
        cmux::CmuxClient,
        hc::{HcClient, HcEndpoint, processes::OwnerProcesses},
        herdr::{HerdrClient, HerdrNativeSnapshot},
    },
    integration::{ConversationId, Effect, OpenReport, Presence, PresentationSnapshot, WorkScope},
    model::{Session, Stream},
    process::NativeProcessRunner,
    runtime::hc::{HcOperation, HcPresentation, open::hc_needs_attention},
};

/// A frontend as seen from one caller: its workspace, its Stream association and its views.
pub trait Frontend {
    /// The Stream the caller works on, if the frontend names one. hc routes each terminal
    /// with its own `streams.work` label; elsewhere this is the workspace's Stream.
    fn caller_stream(&mut self) -> Result<Option<String>>;
    /// A Stream hint for an unenrolled conversation's context. It is separate from
    /// [`Frontend::caller_stream`] so that a context read never costs Herdr a snapshot:
    /// Herdr answers only from an observation it already holds, while hc reads its labels.
    fn context_stream(&mut self) -> Option<String>;
    /// The Stream the caller's workspace or hc space names, which may differ from a
    /// terminal's routing. A later [`Frontend::open`] reuses this observation.
    fn workspace_stream(&mut self) -> Result<Option<String>>;
    /// Associate the caller with the Stream its Session belongs to.
    fn associate_caller(&mut self, stream_id: &str) -> Result<()>;
    /// Tell the caller's views that this command saved records.
    fn notify_saved(&mut self) -> Result<()>;
    /// Open `stream`'s workspace and launch `sessions` in it.
    fn open(
        &mut self,
        runtime: &WorkspaceRuntime<'_>,
        stream: &Stream,
        sessions: &[Session],
        cwd: Option<&Path>,
    ) -> Result<Opened>;
    /// Whether the frontend observes `selected` running, so resume leaves it in place.
    fn running(&mut self, selected: &SelectedConversation<'_>) -> Result<bool>;
}

/// What one opening did, in the shape each frontend reports it.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Opened {
    Workspace(OpenResult),
    Hc {
        backend: &'static str,
        report: OpenReport,
        needs_attention: Vec<String>,
    },
}
impl Opened {
    pub fn needs_attention(&self) -> &[String] {
        match self {
            Self::Workspace(result) => &result.needs_attention,
            Self::Hc {
                needs_attention, ..
            } => needs_attention,
        }
    }
}

/// The caller a frontend serves: its records' scope, environment and working directory.
pub struct Caller {
    pub scope: WorkScope,
    pub environment: BTreeMap<String, String>,
    pub cwd: PathBuf,
    /// Seconds each native request may take.
    pub timeout: f64,
}

pub fn connect(id: FrontendId, caller: Caller) -> Box<dyn Frontend> {
    match id {
        FrontendId::Herdr => Box::new(HerdrFrontend::new(caller)),
        FrontendId::Cmux => Box::new(CmuxFrontend {
            caller,
            native: None,
        }),
        FrontendId::Hc => Box::new(HcFrontend::new(caller)),
    }
}

fn unavailable(message: impl Into<String>) -> RuntimeError {
    FrontendError::invalid(message).into()
}

fn effect(result: Effect<()>) -> Result<()> {
    match result {
        Effect::Applied { value: () } => Ok(()),
        Effect::NotApplied { reason } | Effect::Uncertain { reason } => Err(unavailable(reason)),
    }
}

/// Open through a Herdr or cmux workspace, with each Session's saved transcript locator.
fn open_workspace(
    runtime: &WorkspaceRuntime<'_>,
    stream: &Stream,
    sessions: &[Session],
    open: impl FnOnce(&AtomicBool, &[SelectedConversation<'_>]) -> Result<OpenResult>,
) -> Result<Opened> {
    let locators = runtime.locators(stream, sessions)?;
    let selected: Vec<_> = sessions
        .iter()
        .zip(&locators)
        .map(|(session, locator)| SelectedConversation {
            session,
            locator: locator.as_deref(),
        })
        .collect();
    open(&AtomicBool::new(false), &selected).map(Opened::Workspace)
}

/// Herdr names a workspace's Stream with metadata tokens. The Pi child's native pass
/// associates workspaces, and cards watch record files, so neither needs a call here.
pub struct HerdrFrontend {
    caller: Caller,
    native: Option<HerdrClient>,
    snapshot: Option<HerdrNativeSnapshot>,
    workspace_stream: Option<Option<String>>,
}
impl HerdrFrontend {
    pub fn new(caller: Caller) -> Self {
        Self {
            caller,
            native: None,
            snapshot: None,
            workspace_stream: None,
        }
    }

    /// Reuse the Pi child's connection and the Stream its last pass observed.
    pub fn observed(
        caller: Caller,
        client: &HerdrClient,
        workspace_stream: Option<String>,
    ) -> Result<Self> {
        let client = client.with_context(caller.environment.clone())?;
        Ok(Self {
            native: Some(client),
            workspace_stream: Some(workspace_stream),
            ..Self::new(caller)
        })
    }

    fn native(&mut self) -> Result<&HerdrClient> {
        if self.native.is_none() {
            let client =
                HerdrClient::connect(self.caller.environment.clone(), self.caller.timeout)?
                    .observing()?;
            self.native = Some(client);
        }
        Ok(self.native.as_ref().expect("connected"))
    }

    /// One Herdr snapshot, for a read of the whole topology.
    pub fn into_snapshot(mut self) -> Result<HerdrNativeSnapshot> {
        self.snapshot()?;
        Ok(self.snapshot.take().expect("observed"))
    }

    /// This command's one Herdr snapshot.
    fn snapshot(&mut self) -> Result<&HerdrNativeSnapshot> {
        if self.snapshot.is_none() {
            let snapshot = HerdrNativeSnapshot::parse(&self.native()?.session_snapshot()?)?;
            self.snapshot = Some(snapshot);
        }
        Ok(self.snapshot.as_ref().expect("observed"))
    }
}
impl Frontend for HerdrFrontend {
    fn caller_stream(&mut self) -> Result<Option<String>> {
        self.workspace_stream()
    }
    /// Only the Pi child's native pass observes Herdr for a conversation.
    fn context_stream(&mut self) -> Option<String> {
        self.workspace_stream.clone().flatten()
    }
    fn workspace_stream(&mut self) -> Result<Option<String>> {
        if let Some(stream) = &self.workspace_stream {
            return Ok(stream.clone());
        }
        self.snapshot()?;
        let snapshot = self.snapshot.as_ref().expect("observed");
        let caller = self
            .native
            .as_ref()
            .expect("connected")
            .current_pane_from(snapshot.raw())?;
        let stream = selected_stream(
            snapshot,
            &caller.workspace_id,
            &self.caller.scope.catalog,
            &self.caller.scope.host,
        )?;
        self.workspace_stream = Some(stream.clone());
        Ok(stream)
    }
    fn associate_caller(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn notify_saved(&mut self) -> Result<()> {
        Ok(())
    }
    fn open(
        &mut self,
        runtime: &WorkspaceRuntime<'_>,
        stream: &Stream,
        sessions: &[Session],
        cwd: Option<&Path>,
    ) -> Result<Opened> {
        self.native()?;
        let native = self.native.as_ref().expect("connected");
        let observed = self.snapshot.as_ref();
        open_workspace(runtime, stream, sessions, |cancelled, selected| {
            runtime.open_herdr(native, cancelled, stream, selected, cwd, observed)
        })
    }
    fn running(&mut self, selected: &SelectedConversation<'_>) -> Result<bool> {
        Ok(!running(self.snapshot()?, selected).is_empty())
    }
}

/// cmux workspaces carry no Stream association, and cmux has no saved-record views.
pub struct CmuxFrontend {
    caller: Caller,
    native: Option<CmuxClient>,
}
impl Frontend for CmuxFrontend {
    fn caller_stream(&mut self) -> Result<Option<String>> {
        self.workspace_stream()
    }
    fn context_stream(&mut self) -> Option<String> {
        None
    }
    fn workspace_stream(&mut self) -> Result<Option<String>> {
        Err(unavailable(
            "cmux workspaces name no Stream; supply an explicit reference",
        ))
    }
    fn associate_caller(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn notify_saved(&mut self) -> Result<()> {
        Ok(())
    }
    fn open(
        &mut self,
        runtime: &WorkspaceRuntime<'_>,
        stream: &Stream,
        sessions: &[Session],
        cwd: Option<&Path>,
    ) -> Result<Opened> {
        if self.native.is_none() {
            self.native = Some(CmuxClient::connect(
                Arc::new(NativeProcessRunner),
                self.caller.environment.clone(),
                self.caller.timeout,
            )?);
        }
        let native = self.native.as_ref().expect("connected");
        open_workspace(runtime, stream, sessions, |cancelled, selected| {
            runtime.open_cmux(native, cancelled, stream, selected, cwd)
        })
    }
    /// cmux cannot observe a running conversation.
    fn running(&mut self, _: &SelectedConversation<'_>) -> Result<bool> {
        Ok(false)
    }
}

/// hc names a space's Stream in its association document and routes terminals with
/// `streams.work` labels. Views refresh on a scoped label on the caller's terminal.
pub struct HcFrontend {
    caller: Caller,
    /// `HC_TERMINAL_ID`, when it is a valid local terminal key.
    terminal: Option<String>,
    /// The endpoint a registered Pi source must keep talking to, and its cancellation.
    pinned: Option<(HcEndpoint, EndpointId, Cancellation)>,
    connection: Option<(HcEndpoint, HcOperation)>,
    presentation: Option<HcPresentation>,
}
impl HcFrontend {
    fn new(caller: Caller) -> Self {
        let terminal = caller
            .environment
            .get("HC_TERMINAL_ID")
            .filter(|value| !value.is_empty() && !value.contains('/'))
            .cloned();
        Self {
            caller,
            terminal,
            pinned: None,
            connection: None,
            presentation: None,
        }
    }

    /// The Pi child's registered source: its terminal, on the endpoint it registered with.
    pub fn registered(
        caller: Caller,
        terminal: &str,
        socket: HcEndpoint,
        endpoint: EndpointId,
        stopping: Cancellation,
    ) -> Self {
        Self {
            terminal: Some(terminal.into()),
            pinned: Some((socket, endpoint, stopping)),
            ..Self::new(caller)
        }
    }

    fn terminal(&self) -> Result<String> {
        self.terminal.clone().ok_or_else(|| {
            unavailable("hc --current requires HC_TERMINAL_ID or an explicit reference")
        })
    }

    fn connection(&mut self) -> Result<&mut (HcEndpoint, HcOperation)> {
        if self.connection.is_none() {
            let (socket, expected, stopping) = match &self.pinned {
                Some((socket, endpoint, stopping)) => {
                    (socket.clone(), Some(endpoint), Some(stopping.clone()))
                }
                None => (
                    HcEndpoint::resolve(&self.caller.environment, &self.caller.cwd)?,
                    None,
                    None,
                ),
            };
            let client =
                HcClient::connect(socket.clone(), expected, self.caller.timeout, stopping)?;
            self.connection = Some((socket, HcOperation::new(client)));
        }
        Ok(self.connection.as_mut().expect("connected"))
    }

    /// Each association or save notice is one hc operation on its own connection.
    fn effect_connection(&mut self) -> Result<&mut HcOperation> {
        self.connection = None;
        Ok(&mut self.connection()?.1)
    }
}
impl Frontend for HcFrontend {
    fn caller_stream(&mut self) -> Result<Option<String>> {
        let terminal = self.terminal()?;
        let scope = self.caller.scope.clone();
        Ok(self.connection()?.1.caller_stream_id(&scope, &terminal)?)
    }
    fn context_stream(&mut self) -> Option<String> {
        self.terminal.as_ref()?;
        self.caller_stream().ok().flatten()
    }
    fn workspace_stream(&mut self) -> Result<Option<String>> {
        let terminal = self.terminal()?;
        let scope = self.caller.scope.clone();
        let presentation = self
            .connection()?
            .1
            .presentation_for_caller(&scope, Some(&terminal))?;
        let space = presentation.caller_space().ok_or_else(|| {
            unavailable("calling space is not published in the layout; supply a reference")
        })?;
        let stream = presentation.stream_for_space(&space);
        self.presentation = Some(presentation);
        Ok(stream)
    }
    fn associate_caller(&mut self, stream_id: &str) -> Result<()> {
        let Some(terminal) = self.terminal.clone() else {
            return Ok(());
        };
        let scope = self.caller.scope.clone();
        effect(
            self.effect_connection()?
                .bind_caller_terminal(&scope, &terminal, stream_id),
        )
    }
    fn notify_saved(&mut self) -> Result<()> {
        let Some(terminal) = self.terminal.clone() else {
            return Ok(());
        };
        let scope = self.caller.scope.clone();
        effect(self.effect_connection()?.notify_saved(&scope, &terminal))
    }
    fn open(
        &mut self,
        runtime: &WorkspaceRuntime<'_>,
        stream: &Stream,
        sessions: &[Session],
        cwd: Option<&Path>,
    ) -> Result<Opened> {
        let presentation = self.presentation.take();
        let scope = self.caller.scope.clone();
        let (socket, operation) = self.connection()?;
        let report = runtime.open_hc(operation, socket, stream, sessions, cwd, presentation)?;
        Ok(Opened::Hc {
            backend: "hc",
            needs_attention: hc_needs_attention(&scope, &report),
            report,
        })
    }
    /// hc observes only Pi conversations, through their registered sources.
    fn running(&mut self, selected: &SelectedConversation<'_>) -> Result<bool> {
        if selected.session.harness != "pi" {
            return Ok(false);
        }
        let presence = (|| {
            let observer = OwnerProcesses::new().ok()?;
            let sources = self
                .connection()
                .ok()?
                .1
                .registered_sources(&observer)
                .ok()?;
            Some(sources.presence(&ConversationId::from(selected.session)))
        })();
        match presence {
            Some(Presence::Absent) => Ok(false),
            Some(Presence::Running { .. }) => Ok(true),
            Some(Presence::Unknown { .. }) | None => {
                Err(unavailable("hc Pi presence is unavailable"))
            }
        }
    }
}
