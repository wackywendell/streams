//! Herdr endpoint discovery, exact native identities and workspace effects.
//!
//! Topology and pane observations are structural native evidence only, not
//! catalog/session attestation. Runtime ownership proofs must remain separate.

mod launch;
mod ownership;
mod topology;
pub use launch::HerdrStartedAgent;
pub use ownership::{HerdrNativeSnapshot, PairState};
pub use topology::{HerdrAgent, HerdrInventoryPane, HerdrLayout, HerdrTab, HerdrTopology};

use super::transport::{Operation, Transport, args, launch_args, string};
use super::*;
use crate::process::ProcessRunner;
use serde_json::Value;
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

/// The native agent record is an observation, not catalog or saved-file authority.
#[derive(Debug, Clone)]
pub(crate) struct HerdrAgentInfo {
    pub pane_id: String,
    pub workspace_id: String,
    pub agent: String,
    pub metadata: Value,
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
    pub tokens: Vec<String>,
}

#[derive(Clone)]
pub struct HerdrClient {
    transport: Transport,
    endpoint: HerdrEndpoint,
    context: BTreeMap<String, String>,
}

impl HerdrClient {
    /// Read-only discovery rejects incompatible servers without updating/restarting.
    pub fn connect(
        runner: Arc<dyn ProcessRunner>,
        context: BTreeMap<String, String>,
        timeout: f64,
    ) -> Result<Self, FrontendError> {
        crate::process::deadline(timeout, "Herdr")
            .map_err(|e| FrontendError::invalid(e.message))?;
        let mut transport = Transport {
            program: context
                .get("HERDR_BIN_PATH")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| "herdr".into()),
            environment: context.iter().map(|(k, v)| (k.into(), v.into())).collect(),
            timeout,
            runner,
        };
        let endpoint = Self::read_endpoint(&transport, &mut Operation::default())?;
        // Pin subsequent commands to the discovered transport, not later ambient state.
        transport.environment.insert(
            "HERDR_SOCKET_PATH".into(),
            endpoint.socket.as_os_str().to_owned(),
        );
        Ok(Self {
            transport,
            endpoint,
            context,
        })
    }

    pub fn endpoint(&self) -> &HerdrEndpoint {
        &self.endpoint
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

    fn read_endpoint(
        transport: &Transport,
        op: &mut Operation,
    ) -> Result<HerdrEndpoint, FrontendError> {
        let response = op.json(transport, &args(&["status", "--json"]), false, false)?;
        let server = response
            .get("server")
            .filter(|v| v.is_object())
            .ok_or_else(|| op.error("invalid Herdr server status"))?;
        if server.get("running") != Some(&Value::Bool(true)) {
            return Err(op.error("Herdr server is not running"));
        }
        if server.get("compatible") != Some(&Value::Bool(true))
            || server.get("endpoint_compatible") != Some(&Value::Bool(true))
        {
            return Err(op.error("Herdr server is not API-compatible; no automatic update or restart will be attempted"));
        }
        let version = server
            .get("version")
            .and_then(Value::as_str)
            .and_then(parse_version)
            .ok_or_else(|| op.error("invalid Herdr server version"))?;
        if version < (0, 9, 3) {
            return Err(op.error("Herdr requires server version 0.9.3 or newer"));
        }
        let socket = PathBuf::from(op.capture(string(server, "socket"))?);
        if !socket.is_absolute() {
            return Err(op.error("invalid Herdr server socket path"));
        }
        let session = match server.get("session") {
            None | Some(Value::Null) => None,
            Some(Value::String(v)) => Some(v.clone()),
            _ => return Err(op.error("invalid Herdr server session")),
        };
        Ok(HerdrEndpoint { socket, session })
    }

    fn verify_endpoint(&self, op: &mut Operation) -> Result<(), FrontendError> {
        if Self::read_endpoint(&self.transport, op)? != self.endpoint {
            return Err(op.error("Herdr endpoint changed; refusing stale workspace reference"));
        }
        Ok(())
    }

    fn result(
        &self,
        op: &mut Operation,
        command: &[String],
        expected: &str,
        effect: bool,
    ) -> Result<Value, FrontendError> {
        let value = op.json(&self.transport, command, effect, false)?;
        if let Some(error) = value.get("error") {
            return Err(op.error(format!("Herdr command failed: {error}")));
        }
        let result = value
            .get("result")
            .filter(|v| v.is_object())
            .ok_or_else(|| op.error("invalid Herdr result shape"))?;
        if result.get("type").and_then(Value::as_str) != Some(expected) {
            return Err(op.error(format!("unexpected Herdr result; expected {expected}")));
        }
        Ok(result.clone())
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
            &args(&["pane", "current", "--current"]),
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
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        self.pane(&mut op)
    }

    pub(crate) fn agent_info(&self, pane_id: &str) -> Result<HerdrAgentInfo, FrontendError> {
        nonempty(pane_id, "Herdr pane ID")?;
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        let result = self.result(
            &mut op,
            &args(&["agent", "get", pane_id]),
            "agent_info",
            false,
        )?;
        let agent = &result["agent"];
        Ok(HerdrAgentInfo {
            pane_id: op.capture(string(agent, "pane_id"))?,
            workspace_id: op.capture(string(agent, "workspace_id"))?,
            agent: op.capture(string(agent, "agent"))?,
            metadata: agent
                .get("agent_session")
                .cloned()
                .ok_or_else(|| op.error("missing native agent_session"))?,
        })
    }

    pub(crate) fn session_snapshot(&self) -> Result<Value, FrontendError> {
        self.read_snapshot(&mut Operation::default())
    }

    fn read_snapshot(&self, op: &mut Operation) -> Result<Value, FrontendError> {
        self.verify_endpoint(op)?;
        let result = self.result(op, &args(&["api", "snapshot"]), "session_snapshot", false)?;
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
        let environment = launch_args(request)?;
        let cwd = request
            .cwd
            .to_str()
            .ok_or_else(|| FrontendError::invalid("Herdr cwd is not UTF-8"))?;
        let mut command = args(&[
            "workspace",
            "create",
            "--cwd",
            cwd,
            "--label",
            &request.title,
        ]);
        command.extend(environment);
        command.push("--focus".into());
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        let result = self.result(&mut op, &command, "workspace_created", true)?;
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
            &args(&["workspace", "rename", &reference.native_id, title]),
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
        for token in &presentation.tokens {
            nonempty(token, "metadata token")?;
        }
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        self.rename_native(&mut op, reference, &presentation.title)?;
        let mut command = args(&[
            "workspace",
            "report-metadata",
            &reference.native_id,
            "--source",
            "streams",
        ]);
        for token in &presentation.tokens {
            command.extend(["--token".into(), token.clone()]);
        }
        // report-metadata is intentionally silent on success.
        op.run(&self.transport, &command, true)?;
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
        self.verify_endpoint(&mut op)?;
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
            &args(&["workspace", "get", &id]),
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
        self.verify_endpoint(&mut op)?;
        op.run(
            &self.transport,
            &args(&["workspace", "focus", &reference.native_id]),
            true,
        )?;
        op.acknowledge();
        Ok(())
    }
    fn rename(&self, reference: &WorkspaceRef, title: &str) -> Result<(), FrontendError> {
        check_ref(self, reference)?;
        nonempty(title, "workspace title")?;
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        self.rename_native(&mut op, reference, title)
    }
}

fn parse_version(value: &str) -> Option<(u64, u64, u64)> {
    let core = if let Some((core, suffix)) = value.split_once(['-', '+']) {
        if suffix.is_empty()
            || !suffix
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '-'))
        {
            return None;
        }
        core
    } else {
        value
    };
    let parts: Vec<_> = core.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    Some((
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    ))
}
