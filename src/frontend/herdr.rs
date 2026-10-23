//! Herdr endpoint discovery, exact native identities and workspace effects.
//!
//! Topology and pane observations are structural native evidence only, not
//! catalog/session attestation. Runtime ownership proofs must remain separate.

pub mod events;
mod launch;
mod ownership;
pub mod socket;
mod topology;
pub use launch::HerdrStartedAgent;
pub use ownership::{HerdrNativeSnapshot, PairState};
pub use topology::{HerdrAgent, HerdrInventoryPane, HerdrLayout, HerdrTab, HerdrTopology};

use super::transport::{NativeRequest, Operation, launch_args, string};
use super::*;
use serde_json::{Value, json};
use socket::SocketClient;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

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

/// Raw, caller-prepared native metadata tokens. Projection/normalization policy
/// belongs to the runtime; the adapter neither invents nor truncates values.
#[derive(Debug, Clone)]
pub struct HerdrPresentation {
    pub title: String,
    pub tokens: BTreeMap<String, String>,
}

/// One pinned protocol-22 endpoint per process. Herdr closes ordinary request
/// sockets after each reply, so each request connects; only subscriptions persist.
#[derive(Clone)]
pub struct HerdrClient {
    socket: Arc<SocketClient>,
    endpoint: HerdrEndpoint,
    context: BTreeMap<String, String>,
}

impl HerdrClient {
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
        })
    }

    /// Persistent event subscriptions share the pinned endpoint.
    pub fn socket(&self) -> &Arc<SocketClient> {
        &self.socket
    }

    /// Bounded coordinator transport. Effect authority and round-trip validation
    /// stay in the native-card engine; this adapter never retries an operation.
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

    /// Select a protected card's owner without changing its pinned endpoint.
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

    /// Exact caller observation; it does not attest a saved conversation or owner.
    pub fn current_pane(&self) -> Result<HerdrPane, FrontendError> {
        self.pane(&mut Operation::default())
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
        launch_args(request)?;
        let cwd = request
            .cwd
            .to_str()
            .ok_or_else(|| FrontendError::invalid("Herdr cwd is not UTF-8"))?;
        let params =
            json!({"cwd":cwd,"label":request.title,"env":request.environment,"focus":true});
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

    /// Rename then publish the native metadata hint, retaining partial evidence.
    pub fn update_presentation(
        &self,
        reference: &WorkspaceRef,
        presentation: &HerdrPresentation,
    ) -> Result<(), FrontendError> {
        check_ref(self, reference)?;
        nonempty(&presentation.title, "workspace title")?;
        for (key, value) in &presentation.tokens {
            nonempty(key, "metadata token")?;
            nonempty(value, "metadata token")?;
        }
        let mut op = Operation::default();
        self.rename_native(&mut op, reference, &presentation.title)?;
        let params = json!({"workspace_id":reference.native_id,"source":"streams",
            "tokens":presentation.tokens});
        self.call(&mut op, "workspace.report_metadata", params, true)?;
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
