//! Fresh native caller observations. These do not grant launch or ownership permits.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    codec::object,
    frontend::{
        FrontendId, WorkspaceFrontend, WorkspaceSelector,
        cmux::CmuxClient,
        herdr::{HerdrClient, HerdrEndpoint, HerdrNativeSnapshot, HerdrTopology},
    },
    model::{Session, Stream},
    operations::Authority,
    process::ProcessRunner,
};

use super::{
    Result,
    conversations::{
        Harness, NativeIdentity, NativeIdentityKind, NativeSessionMetadata, native_identity,
    },
    invalid,
    state::{RuntimeState, cmux_scope, herdr_scope},
    text,
};

#[cfg(test)]
mod tests;

/// Built-in connection policy, not a replacement for the extensible frontend trait.
#[derive(Clone)]
pub enum BuiltinFrontend {
    Herdr(HerdrClient),
    Cmux(CmuxClient),
}

impl BuiltinFrontend {
    pub fn connect(
        id: &FrontendId,
        runner: Arc<dyn ProcessRunner>,
        environment: BTreeMap<String, String>,
        timeout: f64,
    ) -> Result<Self> {
        match id.0.as_str() {
            "herdr" => Ok(Self::Herdr(HerdrClient::connect(environment, timeout)?)),
            "cmux" => Ok(Self::Cmux(CmuxClient::connect(
                runner,
                environment,
                timeout,
            )?)),
            _ => Err(invalid(format!("unsupported native frontend: {}", id.0))),
        }
    }

    pub fn herdr(&self) -> Option<&HerdrClient> {
        match self {
            Self::Herdr(client) => Some(client),
            Self::Cmux(_) => None,
        }
    }

    pub fn adapter(&self) -> &dyn WorkspaceFrontend {
        match self {
            Self::Herdr(client) => client,
            Self::Cmux(client) => client,
        }
    }

    pub fn scope(&self) -> String {
        match self {
            Self::Herdr(client) => herdr_scope(
                &client.endpoint().socket,
                client.endpoint().session.as_deref(),
            ),
            Self::Cmux(client) => cmux_scope(&client.endpoint().socket),
        }
    }

    /// Resolve only the exact inherited caller, never GUI focus or a display label.
    pub fn caller_from_snapshot(&self, snapshot: &Value) -> Result<CallingWorkspace> {
        let client = self
            .herdr()
            .ok_or_else(|| invalid("Herdr snapshot requires Herdr"))?;
        Ok(CallingWorkspace {
            scope: self.scope(),
            workspace_id: client.current_pane_from(snapshot)?.workspace_id,
        })
    }

    pub fn caller(&self) -> Result<CallingWorkspace> {
        let scope = self.scope();
        let workspace = self.adapter().resolve(WorkspaceSelector::Caller)?;
        Ok(CallingWorkspace {
            scope,
            workspace_id: workspace.reference.native_id,
        })
    }
}

/// A native workspace ID and its pinned endpoint scope cannot be supplied separately.
#[derive(Debug, Clone)]
pub struct CallingWorkspace {
    scope: String,
    workspace_id: String,
}

impl CallingWorkspace {
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    /// Reporting checks claims as well as ready associations, without taking a writer lock.
    pub fn context_stream_id(
        &self,
        state: &RuntimeState,
        explicit: Option<&str>,
    ) -> Result<Option<String>> {
        if let Some(id) = explicit {
            crate::model::check_text(id, "STREAMS_STREAM_ID", false)
                .map_err(|error| invalid(error.to_string()))?;
        }
        let binding = state.association(&self.scope, &self.workspace_id)?;
        if binding.as_ref().is_some_and(|binding| !binding.ready()) {
            return Err(invalid(
                "calling workspace has a partial open; inspect runtime state before reporting",
            ));
        }
        let claim = state.enrollment_claim(&self.scope, &self.workspace_id)?;
        let identities: BTreeSet<_> = [
            explicit,
            binding.as_ref().map(|binding| binding.stream_id()),
            claim.as_ref().map(|claim| claim.stream_id()),
        ]
        .into_iter()
        .flatten()
        .collect();
        if identities.len() > 1 {
            return Err(invalid(
                "calling workspace association, enrollment claim and explicit identity disagree",
            ));
        }
        Ok(identities.first().map(|id| (*id).to_owned()))
    }

    /// Inspection may identify a partial binding; claims alone are not current work.
    pub fn current_stream_id(
        &self,
        state: &RuntimeState,
        explicit: Option<&str>,
    ) -> Result<String> {
        if let Some(id) = explicit {
            crate::model::check_text(id, "STREAMS_STREAM_ID", false)
                .map_err(|error| invalid(error.to_string()))?;
        }
        let binding = state.association(&self.scope, &self.workspace_id)?;
        if let Some(id) = explicit {
            if binding
                .as_ref()
                .is_some_and(|binding| binding.ready() && binding.stream_id() != id)
            {
                return Err(invalid(
                    "explicit STREAMS_STREAM_ID and ready workspace association disagree; inspect runtime state",
                ));
            }
            return Ok(id.into());
        }
        binding
            .map(|binding| binding.stream_id().to_owned())
            .ok_or_else(|| {
                invalid(
                    "calling workspace has no catalog association; open an explicit stream first",
                )
            })
    }
}

/// Exact current Pi caller, not the pane with GUI focus or an inherited parent identity.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "frontend", rename = "herdr")]
pub struct NativeCaller {
    pane_id: String,
    tab_id: String,
    workspace_id: String,
    #[serde(skip)]
    identity: NativeIdentity,
    #[serde(skip)]
    endpoint: HerdrEndpoint,
    #[serde(skip)]
    terminal_id: String,
}

impl NativeCaller {
    pub fn pane_id(&self) -> &str {
        &self.pane_id
    }
    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
    pub fn native_session_id(&self) -> &str {
        &self.identity.id
    }
    pub fn locator(&self) -> Option<&Path> {
        self.identity.locator.as_deref()
    }
    pub fn endpoint(&self) -> &HerdrEndpoint {
        &self.endpoint
    }
    pub fn terminal_id(&self) -> &str {
        &self.terminal_id
    }
    pub fn calling_workspace(&self) -> CallingWorkspace {
        CallingWorkspace {
            scope: herdr_scope(&self.endpoint.socket, self.endpoint.session.as_deref()),
            workspace_id: self.workspace_id.clone(),
        }
    }
}

/// One observation owns its resolved caller; a new snapshot requires a new resolution.
#[derive(Debug)]
pub struct PiCallerObservation {
    snapshot: HerdrNativeSnapshot,
    caller: NativeCaller,
}

impl PiCallerObservation {
    pub fn from_snapshot(
        client: &HerdrClient,
        snapshot: HerdrNativeSnapshot,
        native_session_id: &str,
    ) -> Result<Self> {
        let caller = resolve_pi_caller(
            client,
            snapshot.raw(),
            snapshot.topology(),
            native_session_id,
        )?;
        Ok(Self { snapshot, caller })
    }

    pub fn snapshot(&self) -> &HerdrNativeSnapshot {
        &self.snapshot
    }
    pub fn caller(&self) -> &NativeCaller {
        &self.caller
    }
    pub fn calling_workspace(&self) -> CallingWorkspace {
        self.caller.calling_workspace()
    }
}

fn pi_identity(metadata: &Value) -> Result<NativeIdentity> {
    let metadata: NativeSessionMetadata = object(metadata.clone())
        .map_err(|error| invalid(format!("invalid native Pi metadata: {error}")))?;
    native_identity(&metadata, Harness::Pi)
}

/// Verify the conversation registered in this exact caller pane. No state is written.
pub fn verify_pi_caller(client: &HerdrClient, native_session_id: &str) -> Result<NativeCaller> {
    text(native_session_id)?;
    verify_pi_caller_from_snapshot(client, &client.session_snapshot()?, native_session_id)
}

pub fn verify_pi_caller_from_snapshot(
    client: &HerdrClient,
    snapshot: &Value,
    native_session_id: &str,
) -> Result<NativeCaller> {
    let topology = HerdrTopology::parse(snapshot)?;
    resolve_pi_caller(client, snapshot, &topology, native_session_id)
}

fn resolve_pi_caller(
    client: &HerdrClient,
    snapshot: &Value,
    topology: &HerdrTopology,
    native_session_id: &str,
) -> Result<NativeCaller> {
    text(native_session_id)?;
    let pane = client.current_pane_from(snapshot)?;
    let agents: Vec<_> = topology
        .agents
        .iter()
        .filter(|agent| agent.pane_id == pane.id)
        .collect();
    if agents.len() != 1 {
        return Err(invalid("native caller requires exactly one agent record"));
    }
    let row = snapshot["panes"]
        .as_array()
        .and_then(|panes| panes.iter().find(|row| row["pane_id"] == pane.id))
        .ok_or_else(|| invalid("native caller missing from snapshot"))?;
    if row["agent"] != "pi"
        || agents[0].native["agent"] != "pi"
        || row["agent_session"] != agents[0].native["agent_session"]
    {
        return Err(invalid("native agent pane, workspace or harness mismatch"));
    }
    let identity = pi_identity(&row["agent_session"])?;
    if identity.id != native_session_id {
        return Err(invalid(
            "requested Pi session differs from the native pane session; a background helper cannot automatically enroll its parent's workspace",
        ));
    }
    Ok(NativeCaller {
        pane_id: pane.id,
        tab_id: pane.tab_id,
        workspace_id: pane.workspace_id,
        identity,
        endpoint: client.endpoint().clone(),
        terminal_id: agents[0].terminal_id.clone(),
    })
}

/// Add catalog association and effective-host checks to the fresh native caller proof.
pub fn attest_context(
    client: &HerdrClient,
    state: &RuntimeState,
    stream: &Stream,
    session: &Session,
    authority: Option<&Authority>,
) -> Result<NativeCaller> {
    attest_context_from_snapshot(
        client,
        &client.session_snapshot()?,
        state,
        stream,
        session,
        authority,
    )
}

pub fn attest_context_from_snapshot(
    client: &HerdrClient,
    snapshot: &Value,
    state: &RuntimeState,
    stream: &Stream,
    session: &Session,
    authority: Option<&Authority>,
) -> Result<NativeCaller> {
    check_context_records(state, stream, session, authority)?;
    let caller = verify_pi_caller_from_snapshot(client, snapshot, &session.harness_session_id)?;
    attest_verified_caller(caller, state, stream, session)
}

/// Add catalog checks to the same observed caller without resolving its identity again.
pub fn attest_context_from_caller(
    observed: &PiCallerObservation,
    state: &RuntimeState,
    stream: &Stream,
    session: &Session,
    authority: Option<&Authority>,
) -> Result<NativeCaller> {
    check_context_records(state, stream, session, authority)?;
    attest_verified_caller(observed.caller.clone(), state, stream, session)
}

fn check_context_records(
    state: &RuntimeState,
    stream: &Stream,
    session: &Session,
    authority: Option<&Authority>,
) -> Result<()> {
    if authority.is_none_or(|authority| {
        authority.host_id != state.host_id()
            || crate::model::check_text(&authority.token, "authority token", false).is_err()
    }) {
        return Err(invalid(
            "effective owner authority for this host is required",
        ));
    }
    if session.host_id != state.host_id()
        || session.stream_id != stream.id
        || session.harness != "pi"
    {
        return Err(invalid(
            "native projection requires the exact local Pi Session",
        ));
    }
    Ok(())
}

fn attest_verified_caller(
    caller: NativeCaller,
    state: &RuntimeState,
    stream: &Stream,
    session: &Session,
) -> Result<NativeCaller> {
    if caller.native_session_id() != session.harness_session_id {
        return Err(invalid(
            "observed Pi caller differs from the catalog Session",
        ));
    }
    let workspace = caller.calling_workspace();
    if !state
        .association(workspace.scope(), workspace.workspace_id())?
        .is_some_and(|binding| binding.ready() && binding.stream_id() == stream.id)
    {
        return Err(invalid(
            "native projection requires a ready workspace association for this Stream",
        ));
    }
    if state
        .enrollment_claim(workspace.scope(), workspace.workspace_id())?
        .is_some_and(|claim| claim.stream_id() != stream.id)
    {
        return Err(invalid(
            "native workspace association and enrollment claim disagree",
        ));
    }
    Ok(caller)
}

// Herdr advertises harnesses beyond the closed set Streams can resume.
#[derive(Deserialize)]
struct InventorySessionMetadata {
    agent: String,
    kind: NativeIdentityKind,
    value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeInventoryPane {
    pane_id: String,
    terminal_id: String,
    workspace_id: String,
    tab_id: String,
    native_session_id: Option<String>,
}

impl NativeInventoryPane {
    pub fn pane_id(&self) -> &str {
        &self.pane_id
    }
    pub fn terminal_id(&self) -> &str {
        &self.terminal_id
    }
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }
    pub fn native_session_id(&self) -> Option<&str> {
        self.native_session_id.as_deref()
    }
}

/// Inventory is observation only. A native owner must refresh and match it under its lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NativeInventory {
    socket: PathBuf,
    panes: Vec<NativeInventoryPane>,
}

impl NativeInventory {
    pub fn socket(&self) -> &std::path::Path {
        &self.socket
    }
    pub fn panes(&self) -> &[NativeInventoryPane] {
        &self.panes
    }
}

/// Never infer that a conversation died from a missing saved transcript.
pub fn native_inventory(client: &HerdrClient) -> Result<NativeInventory> {
    native_inventory_from_snapshot(client, &client.session_snapshot()?)
}

pub fn native_inventory_from_snapshot(
    client: &HerdrClient,
    snapshot: &Value,
) -> Result<NativeInventory> {
    read_native_inventory(client.endpoint(), snapshot, None)
}

/// Explicit inventory reuses the observed owner's identity, not its transcript path.
pub fn native_inventory_from_caller(observed: &PiCallerObservation) -> Result<NativeInventory> {
    read_native_inventory(
        observed.caller().endpoint(),
        observed.snapshot().raw(),
        Some(observed.caller()),
    )
}

fn read_native_inventory(
    endpoint: &HerdrEndpoint,
    snapshot: &Value,
    caller: Option<&NativeCaller>,
) -> Result<NativeInventory> {
    let rows = snapshot
        .get("panes")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("native conversation inventory unavailable"))?;
    let mut panes = Vec::new();
    let mut pane_ids = BTreeSet::new();
    let mut terminal_ids = BTreeSet::new();
    for row in rows {
        let field = |name| -> Result<String> {
            let value = row
                .get(name)
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("invalid native conversation inventory tuple"))?;
            text(value)?;
            Ok(value.into())
        };
        let tokens = match row.get("tokens") {
            None | Some(Value::Null) => None,
            Some(Value::Object(tokens)) if tokens.values().all(Value::is_string) => Some(tokens),
            _ => return Err(invalid("invalid native pane tokens")),
        };
        let agent = match row.get("agent") {
            None | Some(Value::Null) => None,
            Some(Value::String(agent)) => {
                text(agent)?;
                Some(agent.as_str())
            }
            _ => return Err(invalid("invalid native pane harness")),
        };
        let metadata = row
            .get("agent_session")
            .filter(|value| !value.is_null())
            .map(|value| -> Result<InventorySessionMetadata> {
                let metadata: InventorySessionMetadata =
                    object(value.clone()).map_err(|error| {
                        invalid(format!("invalid native session metadata: {error}"))
                    })?;
                text(&metadata.agent)?;
                text(&metadata.value)?;
                Ok(metadata)
            })
            .transpose()?;
        if let (Some(agent), Some(metadata)) = (agent, &metadata)
            && agent != metadata.agent
        {
            return Err(invalid("native pane and session harness disagree"));
        }
        let is_pi = agent == Some("pi")
            || metadata
                .as_ref()
                .is_some_and(|metadata| metadata.agent == "pi")
            || tokens
                .and_then(|tokens| tokens.get("strm_role"))
                .and_then(Value::as_str)
                == Some("agent");
        if is_pi && agent.is_some_and(|agent| agent != "pi") {
            return Err(invalid(
                "Streams agent marker contradicts native pane harness",
            ));
        }
        let native_session_id = if is_pi {
            let metadata = metadata.ok_or_else(|| invalid("native Pi metadata is missing"))?;
            if metadata.agent != "pi" {
                return Err(invalid("native Pi metadata harness mismatch"));
            }
            Some(
                match caller.filter(|caller| row["pane_id"] == caller.pane_id()) {
                    Some(caller) => caller.native_session_id().into(),
                    None => {
                        native_identity(
                            &NativeSessionMetadata {
                                agent: Harness::Pi,
                                kind: metadata.kind,
                                value: metadata.value,
                            },
                            Harness::Pi,
                        )?
                        .id
                    }
                },
            )
        } else {
            None
        };
        let pane_id = field("pane_id")?;
        let terminal_id = field("terminal_id")?;
        if !pane_ids.insert(pane_id.clone()) || !terminal_ids.insert(terminal_id.clone()) {
            return Err(invalid("duplicate native conversation inventory identity"));
        }
        panes.push(NativeInventoryPane {
            pane_id,
            terminal_id,
            workspace_id: field("workspace_id")?,
            tab_id: field("tab_id")?,
            native_session_id,
        });
    }
    Ok(NativeInventory {
        socket: endpoint.socket.clone(),
        panes,
    })
}
