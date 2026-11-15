//! Herdr endpoint discovery, exact native identities and workspace effects.
//!
//! Observations select pass-local targets. The subscription-ended check is not
//! an atomic frontend precondition.

pub mod events;
mod launch;
mod snapshot;
pub mod socket;
mod topology;
pub use launch::HerdrStartedAgent;
pub use snapshot::HerdrNativeSnapshot;
pub use topology::{HerdrAgent, HerdrInventoryPane, HerdrLayout, HerdrTab, HerdrTopology};

use super::transport::{NativeRequest, Operation, launch_args, string};
use super::*;
use serde_json::{Value, json};
use socket::SocketClient;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Work-query evidence from one retained Herdr snapshot; no foreground check is implied.
pub struct HerdrWorkExecution<'a> {
    snapshot: &'a HerdrNativeSnapshot,
    endpoint: EndpointId,
    scope: crate::integration::WorkScope,
    stream: &'a str,
    wanted: &'a [crate::integration::WantedConversation],
}
impl HerdrNativeSnapshot {
    pub fn work_execution<'a>(
        &'a self,
        endpoint: EndpointId,
        scope: crate::integration::WorkScope,
        stream: &'a str,
        wanted: &'a [crate::integration::WantedConversation],
    ) -> HerdrWorkExecution<'a> {
        HerdrWorkExecution {
            snapshot: self,
            endpoint,
            scope,
            stream,
            wanted,
        }
    }
    pub fn matching_agents<'a>(
        &'a self,
        wanted: &crate::integration::WantedConversation,
    ) -> Vec<&'a HerdrAgent> {
        self.topology()
            .agents
            .iter()
            .filter(|agent| {
                let metadata = &agent.native["agent_session"];
                agent.native["agent"] == wanted.conversation.harness
                    && metadata["agent"] == wanted.conversation.harness
                    && match metadata["kind"].as_str() {
                        Some("id") => metadata["value"] == wanted.conversation.native_id,
                        Some("path") => wanted
                            .locator
                            .as_deref()
                            .and_then(std::path::Path::to_str)
                            .is_some_and(|path| metadata["value"] == path),
                        _ => false,
                    }
            })
            .collect()
    }
}
impl crate::integration::ExecutionSnapshot for HerdrWorkExecution<'_> {
    fn presence(
        &self,
        conversation: &crate::integration::ConversationId,
    ) -> crate::integration::Presence {
        use crate::integration::{NativeRef, NonEmpty, Presence};
        let Some(wanted) = self
            .wanted
            .iter()
            .find(|wanted| &wanted.conversation == conversation)
        else {
            return Presence::Unknown {
                reason: "conversation was not part of the retained work query".into(),
            };
        };
        let locations: Vec<_> = self
            .snapshot
            .matching_agents(wanted)
            .into_iter()
            .map(|agent| NativeRef {
                adapter: FrontendId("herdr".into()),
                endpoint: self.endpoint.clone(),
                key: agent.pane_id.clone(),
            })
            .collect();
        if locations.is_empty() {
            Presence::Absent
        } else {
            Presence::Running {
                locations: NonEmpty::try_from(locations).expect("observed locations are nonempty"),
            }
        }
    }
    fn verified_terminals(
        &self,
        conversation: &crate::integration::ConversationId,
    ) -> Vec<crate::integration::NativeRef> {
        match self.presence(conversation) {
            crate::integration::Presence::Running { locations } => locations.into_vec(),
            _ => vec![],
        }
    }
    fn stream_terminals(
        &self,
        stream: &str,
    ) -> crate::integration::Access<Vec<crate::integration::NativeRef>> {
        use crate::integration::Access;
        if stream != self.stream {
            return Access::Unavailable {
                reason: "Stream routing was not inspected".into(),
            };
        }
        let tokens = crate::runtime::native_markers::stream_tokens(
            &self.scope.catalog,
            &self.scope.host,
            stream,
        );
        let value = self
            .snapshot
            .topology()
            .panes
            .iter()
            .filter(|pane| {
                self.snapshot
                    .workspace_tokens(&pane.pane.workspace_id)
                    .is_some_and(|observed| {
                        tokens
                            .iter()
                            .all(|(key, value)| observed.get(key) == Some(value))
                    })
            })
            .map(|pane| crate::integration::NativeRef {
                adapter: FrontendId("herdr".into()),
                endpoint: self.endpoint.clone(),
                key: pane.pane.id.clone(),
            })
            .collect();
        Access::Available { value }
    }
    fn foreground_conversation(
        &self,
        _: &crate::integration::NativeRef,
    ) -> crate::integration::ForegroundConversation {
        crate::integration::ForegroundConversation::Unknown {
            reason: "foreground was not inspected by this work query".into(),
        }
    }
}

impl HerdrClient {
    /// Apply prepared inputs only: no catalog reads, directory selection or native discovery.
    /// Herdr uses pane IDs as portable terminal locations, including replies without terminal_id.
    pub fn execute_prepared_open(
        &self,
        plan: &crate::integration::PreparedOpen,
        scope: &crate::integration::WorkScope,
        environment: &BTreeMap<String, String>,
        cancelled: &AtomicBool,
    ) -> PreparedOpenOutcome {
        use crate::integration::*;
        let mut effects = unattempted_open(plan);
        let check = || {
            if cancelled.load(Ordering::Acquire) {
                Err(FrontendError::invalid("native operation cancelled"))
            } else {
                Ok(())
            }
        };
        let reference = |key: &str| NativeRef {
            adapter: self.frontend_id(),
            endpoint: self.endpoint_id(),
            key: key.into(),
        };
        let result = (|| -> Result<(), FrontendError> {
            if !plan.adopt.is_empty() {
                return Err(FrontendError::invalid("Herdr adoption is unsupported"));
            }
            let mut root = None;
            let workspace = match &plan.space {
                SpaceDecision::Reuse { space } => {
                    let workspace = WorkspaceRef {
                        frontend: space.adapter.clone(),
                        endpoint: space.endpoint.clone(),
                        native_id: space.key.clone(),
                    };
                    super::check_ref(self, &workspace)?;
                    effects.presentation.space = SpaceEvidence::Known {
                        value: space.clone(),
                    };
                    workspace
                }
                SpaceDecision::Create { title, cwd } => {
                    let (initial_cwd, initial_environment) = match plan.launches.first() {
                        Some(LaunchItem::Conversation(launch)) => {
                            (launch.cwd(), launch.environment())
                        }
                        Some(LaunchItem::Shell { cwd }) => (cwd.as_path(), environment),
                        None => (cwd.as_path(), environment),
                    };
                    if initial_cwd != cwd {
                        return Err(FrontendError::invalid(
                            "Herdr root cwd differs from the prepared launch",
                        ));
                    }
                    check()?;
                    let created = self
                        .create_native_with_focus(
                            &CreateWorkspace {
                                title: title.clone(),
                                cwd: cwd.clone(),
                                environment: initial_environment.clone(),
                            },
                            plan.focus,
                        )
                        .inspect_err(|error| {
                            effects.presentation.state = failed_presentation(error)
                        })?;
                    let workspace = created.workspace.reference;
                    effects.presentation.space = SpaceEvidence::Known {
                        value: reference(&workspace.native_id),
                    };
                    if matches!(plan.launches.first(), Some(LaunchItem::Shell { .. })) {
                        effects.launches[0] = Effect::Applied {
                            value: reference(&created.root_pane.id),
                        };
                    }
                    root = Some(created.root_pane);
                    check()?;
                    self.card_request("workspace.report_metadata", json!({"workspace_id":workspace.native_id,
                        "source":"streams", "tokens":crate::runtime::native_markers::stream_tokens(&scope.catalog, &scope.host, &plan.stream)}),
                        Some("ok"), true).inspect_err(|error| effects.presentation.state = failed_presentation(error))?;
                    workspace
                }
                SpaceDecision::None => {
                    effects.presentation.state = PresentationState::Complete;
                    return Ok(());
                }
                SpaceDecision::Unavailable { reason } => {
                    return Err(FrontendError::invalid(reason));
                }
            };
            for (index, launch) in plan.launches.iter().enumerate() {
                check()?;
                let (cwd, environment) = match launch {
                    LaunchItem::Conversation(launch) => (launch.cwd(), launch.environment()),
                    LaunchItem::Shell { cwd } => (cwd.as_path(), environment),
                };
                let pane = match root.take() {
                    Some(pane) => pane,
                    None => {
                        self.create_tab(&workspace, cwd, environment, None)
                            .inspect_err(|error| {
                                effects.presentation.state = failed_presentation(error)
                            })?
                            .1
                    }
                };
                check()?;
                if let LaunchItem::Conversation(launch) = launch {
                    let name = format!(
                        "streams-{}",
                        &uuid::Uuid::new_v4().simple().to_string()[..24]
                    );
                    self.start_agent(
                        &pane,
                        &name,
                        launch.command().harness().as_ref(),
                        &launch.command().argv()[1..],
                    )
                    .inspect_err(|error| effects.launches[index] = failed_launch(error))?;
                }
                effects.launches[index] = Effect::Applied {
                    value: reference(&pane.id),
                };
            }
            check()?;
            if plan.focus {
                self.focus(&workspace)
                    .inspect_err(|error| effects.presentation.state = failed_presentation(error))?;
            }
            effects.presentation.state = PresentationState::Complete;
            Ok(())
        })();
        if let Err(error) = &result
            && let PresentationState::Incomplete { reason } = &mut effects.presentation.state
        {
            reason.clone_from(&error.message);
        }
        PreparedOpenOutcome {
            effects,
            error: result.err(),
        }
    }
}

/// The server's reported socket and optional native session namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrEndpoint {
    pub socket: PathBuf,
    pub session: Option<String>,
}

impl HerdrEndpoint {
    /// Herdr's own resolution order: `HERDR_SOCKET_PATH`, then the
    /// `HERDR_SESSION` namespace, then the default session socket.
    pub fn resolve(context: &BTreeMap<String, String>) -> Result<Self, FrontendError> {
        let value = |key: &str| context.get(key).filter(|v| !v.is_empty());
        let session = value("HERDR_SESSION").cloned();
        if let Some(name) = &session
            && (matches!(name.as_str(), "." | "..")
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        {
            return Err(FrontendError::invalid("invalid HERDR_SESSION"));
        }
        let socket = match value("HERDR_SOCKET_PATH") {
            Some(path) => PathBuf::from(path),
            None => {
                let config = match value("XDG_CONFIG_HOME") {
                    Some(path) => PathBuf::from(path),
                    None => PathBuf::from(
                        value("HOME").ok_or_else(|| FrontendError::invalid("HOME is required"))?,
                    )
                    .join(".config"),
                }
                .join("herdr");
                match &session {
                    Some(name) => config.join("sessions").join(name).join("herdr.sock"),
                    None => config.join("herdr.sock"),
                }
            }
        };
        if !socket.is_absolute() {
            return Err(FrontendError::invalid("invalid Herdr server socket path"));
        }
        Ok(Self { socket, session })
    }

    pub fn identity(&self) -> EndpointId {
        EndpointId(format!(
            "herdr:{}",
            serde_json::json!({"socket": self.socket, "session": self.session})
        ))
    }
}

/// Herdr's workspace record; caller-derived cwd is supplied only by exact pane lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrWorkspace {
    pub id: String,
    pub label: String,
    pub cwd: String,
}

impl HerdrWorkspace {
    pub fn parse(value: &Value) -> Result<Self, FrontendError> {
        Ok(Self {
            id: string(value, "workspace_id")?,
            label: value
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| FrontendError::invalid("invalid Herdr workspace label"))?
                .into(),
            cwd: String::new(),
        })
    }
}

/// Exact pane/tab/workspace tuple, never a generic frontend reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrPane {
    pub id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub cwd: String,
}

impl HerdrPane {
    pub fn parse(value: &Value) -> Result<Self, FrontendError> {
        let mut directories = Vec::new();
        for key in ["foreground_cwd", "cwd"] {
            match value.get(key) {
                None | Some(Value::Null) => {}
                Some(Value::String(v)) => directories.push(v.as_str()),
                _ => return Err(FrontendError::invalid("invalid Herdr pane cwd")),
            }
        }
        Ok(Self {
            id: string(value, "pane_id")?,
            tab_id: string(value, "tab_id")?,
            workspace_id: string(value, "workspace_id")?,
            cwd: directories
                .into_iter()
                .find(|v| !v.is_empty())
                .unwrap_or_default()
                .into(),
        })
    }
}

/// Herdr's richer creation result is deliberately not imposed on CMUX.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrCreatedWorkspace {
    pub workspace: Workspace,
    pub tab: HerdrTab,
    pub root_pane: HerdrPane,
}

/// One pinned protocol-22 endpoint per process. Herdr closes ordinary request
/// sockets after each reply, so each request connects; only subscriptions persist.
#[derive(Clone)]
pub struct HerdrClient {
    socket: Arc<SocketClient>,
    endpoint: HerdrEndpoint,
    context: BTreeMap<String, String>,
    subscription_ended: Option<Arc<AtomicBool>>,
    _pass_subscription: Option<Arc<events::PassSubscription>>,
}

impl HerdrClient {
    pub fn opening_support(&self) -> crate::integration::OpeningSupport {
        use crate::integration::{LaunchLimit, OpeningMode, OpeningSupport, Placement};
        OpeningSupport::known_harnesses(
            OpeningMode::ReuseObserved,
            LaunchLimit::Many,
            Placement::RequiresSpace,
        )
    }
    /// The single protocol check rejects incompatible servers without
    /// updating or restarting them, and pins the endpoint for this process.
    pub fn connect(context: BTreeMap<String, String>, timeout: f64) -> Result<Self, FrontendError> {
        Self::connect_inner(context, timeout, None)
    }

    pub fn connect_cancellable(
        context: BTreeMap<String, String>,
        timeout: f64,
        cancellation: crate::cancellation::Cancellation,
    ) -> Result<Self, FrontendError> {
        Self::connect_inner(context, timeout, Some(cancellation))
    }

    fn connect_inner(
        context: BTreeMap<String, String>,
        timeout: f64,
        cancellation: Option<crate::cancellation::Cancellation>,
    ) -> Result<Self, FrontendError> {
        let endpoint = HerdrEndpoint::resolve(&context)?;
        let socket = Arc::new(match cancellation {
            Some(cancel) => {
                SocketClient::connect_cancellable(endpoint.socket.clone(), timeout, cancel)?
            }
            None => SocketClient::connect(endpoint.socket.clone(), timeout)?,
        });
        Ok(Self {
            socket,
            endpoint,
            context,
            subscription_ended: None,
            _pass_subscription: None,
        })
    }

    /// Start a bounded foreground observation without an event controller.
    /// Register before taking its snapshot; an ended pass is never revived here.
    pub fn observing(&self) -> Result<Self, FrontendError> {
        if self.subscription_ended.is_some() {
            return Ok(self.clone());
        }
        let subscription = Arc::new(
            events::PassSubscription::start(&self.socket)
                .map_err(|error| FrontendError::invalid(error.to_string()))?,
        );
        Ok(Self {
            subscription_ended: Some(subscription.ended()),
            _pass_subscription: Some(subscription),
            ..self.clone()
        })
    }

    /// Bind this pass to the current subscription. This cheap disconnect check
    /// does not atomically protect against a restart after the check.
    pub fn with_subscription(&self, subscription: &events::SubscriptionCancel) -> Self {
        Self {
            subscription_ended: Some(subscription.ended()),
            ..self.clone()
        }
    }

    /// A delivered disconnect ends this observation, including native-derived catalog actions.
    pub fn check_subscription(&self) -> Result<(), FrontendError> {
        if self
            .subscription_ended
            .as_ref()
            .is_some_and(|ended| ended.load(Ordering::Acquire))
        {
            Err(FrontendError::invalid(
                "Herdr subscription ended; native pass abandoned",
            ))
        } else {
            Ok(())
        }
    }

    /// Persistent event subscriptions share the pinned endpoint.
    pub fn socket(&self) -> &Arc<SocketClient> {
        &self.socket
    }

    /// Bounded native transport. Selection and checked replies stay in the
    /// native-card engine; this adapter never retries an operation.
    pub(crate) fn card_request(
        &self,
        method: &str,
        params: Value,
        expected: Option<&str>,
        effect: bool,
    ) -> Result<(Value, Vec<super::CommandEvidence>), FrontendError> {
        let mut op = Operation::default();
        let value = if let Some(expected) = expected {
            self.result(&mut op, method, params, expected, effect)?
        } else {
            self.call(&mut op, method, params, effect)?;
            Value::Null
        };
        if effect {
            op.acknowledge();
        }
        Ok((value, op.commands))
    }

    pub fn endpoint(&self) -> &HerdrEndpoint {
        &self.endpoint
    }

    /// Select Pi's observation context without changing the endpoint.
    pub fn with_context(&self, context: BTreeMap<String, String>) -> Result<Self, FrontendError> {
        if HerdrEndpoint::resolve(&context)? != self.endpoint {
            return Err(FrontendError::invalid("Herdr reader endpoint changed"));
        }
        Ok(Self {
            context,
            ..self.clone()
        })
    }

    pub fn current_pane_from(&self, snapshot: &Value) -> Result<HerdrPane, FrontendError> {
        if self.context.get("HERDR_ENV").map(String::as_str) != Some("1") {
            return Err(FrontendError::invalid(
                "current Herdr targeting requires HERDR_ENV=1 and HERDR_PANE_ID",
            ));
        }
        let id = self
            .context
            .get("HERDR_PANE_ID")
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                FrontendError::invalid(
                    "current Herdr targeting requires HERDR_ENV=1 and HERDR_PANE_ID",
                )
            })?;
        let panes = snapshot["panes"]
            .as_array()
            .ok_or_else(|| FrontendError::invalid("missing Herdr pane inventory"))?;
        let mut matches = panes
            .iter()
            .filter(|pane| pane["pane_id"].as_str() == Some(id));
        let pane = HerdrPane::parse(
            matches
                .next()
                .ok_or_else(|| FrontendError::invalid("Herdr caller missing from snapshot"))?,
        )?;
        if matches.next().is_some() {
            return Err(FrontendError::invalid("ambiguous Herdr caller"));
        }
        let workspaces = snapshot["workspaces"]
            .as_array()
            .ok_or_else(|| FrontendError::invalid("missing Herdr workspace inventory"))?;
        let mut matches = workspaces
            .iter()
            .filter(|workspace| workspace["workspace_id"].as_str() == Some(&pane.workspace_id));
        HerdrWorkspace::parse(
            matches
                .next()
                .ok_or_else(|| FrontendError::invalid("Herdr caller workspace missing"))?,
        )?;
        if matches.next().is_some() {
            return Err(FrontendError::invalid("ambiguous Herdr caller workspace"));
        }
        Ok(pane)
    }

    fn reference(&self, id: String) -> WorkspaceRef {
        WorkspaceRef {
            frontend: self.frontend_id(),
            endpoint: self.endpoint_id(),
            native_id: id,
        }
    }

    fn workspace(&self, native: HerdrWorkspace) -> Workspace {
        Workspace {
            reference: self.reference(native.id),
            title: native.label,
            cwd: native.cwd,
        }
    }

    /// One socket request, recorded with its raw reply before it is decoded.
    fn call(
        &self,
        op: &mut Operation,
        method: &str,
        params: Value,
        effect: bool,
    ) -> Result<Value, FrontendError> {
        if effect {
            self.check_subscription()
                .map_err(|error| op.error(error.message))?;
        }
        let (response, result) = self.socket.exchange(method, params.clone());
        op.commands.push(CommandEvidence {
            request: NativeRequest::Socket {
                method: method.into(),
                params,
                response,
            },
            outcome: if effect {
                CommandOutcome::Uncertain
            } else {
                CommandOutcome::ReadOnly
            },
        });
        op.capture(result)
    }

    fn result(
        &self,
        op: &mut Operation,
        method: &str,
        params: Value,
        expected: &str,
        effect: bool,
    ) -> Result<Value, FrontendError> {
        let result = self.call(op, method, params, effect)?;
        if result.get("type").and_then(Value::as_str) != Some(expected) {
            return Err(op.error(format!("unexpected Herdr result; expected {expected}")));
        }
        Ok(result)
    }

    fn pane(&self, op: &mut Operation) -> Result<HerdrPane, FrontendError> {
        let caller = self
            .context
            .get("HERDR_PANE_ID")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| {
                op.error("current Herdr targeting requires HERDR_ENV=1 and HERDR_PANE_ID")
            })?;
        if self.context.get("HERDR_ENV").map(String::as_str) != Some("1") {
            return Err(op.error("current Herdr targeting requires HERDR_ENV=1 and HERDR_PANE_ID"));
        }
        let result = self.result(
            op,
            "pane.current",
            json!({"caller_pane_id":caller}),
            "pane_current",
            false,
        )?;
        let pane = op.capture(HerdrPane::parse(&result["pane"]))?;
        if pane.id != *caller {
            return Err(op.error("Herdr caller pane ID mismatch"));
        }
        Ok(pane)
    }

    pub fn session_snapshot(&self) -> Result<Value, FrontendError> {
        self.read_snapshot(&mut Operation::default())
    }

    fn read_snapshot(&self, op: &mut Operation) -> Result<Value, FrontendError> {
        let result = self.result(op, "session.snapshot", json!({}), "session_snapshot", false)?;
        result
            .get("snapshot")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| op.error("invalid Herdr snapshot"))
    }

    /// Complete structural topology for the later arrangement/attestation layer.
    /// Missing collections, duplicate IDs and contradictory ownership fail closed.
    pub fn topology(&self) -> Result<HerdrTopology, FrontendError> {
        let mut op = Operation::default();
        let snapshot = self.read_snapshot(&mut op)?;
        op.capture(HerdrTopology::parse(&snapshot))
    }

    /// Create only the native shell workspace and verify its exact root tuple.
    pub fn create_native(
        &self,
        request: &CreateWorkspace,
    ) -> Result<HerdrCreatedWorkspace, FrontendError> {
        self.create_native_with_focus(request, true)
    }

    fn create_native_with_focus(
        &self,
        request: &CreateWorkspace,
        focus: bool,
    ) -> Result<HerdrCreatedWorkspace, FrontendError> {
        launch_args(request)?;
        let cwd = request
            .cwd
            .to_str()
            .ok_or_else(|| FrontendError::invalid("Herdr cwd is not UTF-8"))?;
        let params =
            json!({"cwd":cwd,"label":request.title,"env":request.environment,"focus":focus});
        let mut op = Operation::default();
        let result = self.result(
            &mut op,
            "workspace.create",
            params,
            "workspace_created",
            true,
        )?;
        let workspace = op.capture(HerdrWorkspace::parse(&result["workspace"]))?;
        let tab = op.capture(HerdrTab::parse(&result["tab"]))?;
        let pane = op.capture(HerdrPane::parse(&result["root_pane"]))?;
        if tab.workspace_id != workspace.id
            || pane.workspace_id != workspace.id
            || pane.tab_id != tab.id
        {
            return Err(op.error("Herdr created pane/tab/workspace ID mismatch"));
        }
        op.acknowledge();
        Ok(HerdrCreatedWorkspace {
            workspace: self.workspace(workspace),
            tab,
            root_pane: pane,
        })
    }

    fn rename_native(
        &self,
        op: &mut Operation,
        reference: &WorkspaceRef,
        title: &str,
    ) -> Result<(), FrontendError> {
        let result = self.result(
            op,
            "workspace.rename",
            json!({"workspace_id":reference.native_id,"label":title}),
            "workspace_info",
            true,
        )?;
        let renamed = op.capture(HerdrWorkspace::parse(&result["workspace"]))?;
        if renamed.id != reference.native_id {
            return Err(op.error("Herdr renamed workspace ID mismatch"));
        }
        op.acknowledge();
        Ok(())
    }
}

impl WorkspaceFrontend for HerdrClient {
    fn frontend_id(&self) -> FrontendId {
        FrontendId::herdr()
    }
    fn endpoint_id(&self) -> EndpointId {
        self.endpoint.identity()
    }
    fn workspaces(&self) -> Result<Vec<Workspace>, FrontendError> {
        Ok(self
            .topology()?
            .workspaces
            .into_iter()
            .map(|w| self.workspace(w))
            .collect())
    }
    fn resolve(&self, selector: WorkspaceSelector<'_>) -> Result<Workspace, FrontendError> {
        match &selector {
            WorkspaceSelector::Exact(reference) => check_ref(self, reference)?,
            WorkspaceSelector::Explicit(id) => nonempty(id, "Herdr workspace ID")?,
            WorkspaceSelector::Caller => {}
        }
        let mut op = Operation::default();
        let (id, cwd) = match selector {
            WorkspaceSelector::Caller => {
                let pane = self.pane(&mut op)?;
                (pane.workspace_id, pane.cwd)
            }
            WorkspaceSelector::Explicit(id) => (id.into(), String::new()),
            WorkspaceSelector::Exact(reference) => (reference.native_id.clone(), String::new()),
        };
        let result = self.result(
            &mut op,
            "workspace.get",
            json!({"workspace_id":id}),
            "workspace_info",
            false,
        )?;
        let mut workspace = op.capture(HerdrWorkspace::parse(&result["workspace"]))?;
        if workspace.id != id {
            return Err(op.error("Herdr workspace ID mismatch"));
        }
        workspace.cwd = cwd;
        Ok(self.workspace(workspace))
    }
    fn create(&self, request: &CreateWorkspace) -> Result<Workspace, FrontendError> {
        Ok(self.create_native(request)?.workspace)
    }
    fn focus(&self, reference: &WorkspaceRef) -> Result<(), FrontendError> {
        check_ref(self, reference)?;
        let mut op = Operation::default();
        self.call(
            &mut op,
            "workspace.focus",
            json!({"workspace_id":reference.native_id}),
            true,
        )?;
        op.acknowledge();
        Ok(())
    }
    fn rename(&self, reference: &WorkspaceRef, title: &str) -> Result<(), FrontendError> {
        check_ref(self, reference)?;
        nonempty(title, "workspace title")?;
        self.rename_native(&mut Operation::default(), reference, title)
    }
}
