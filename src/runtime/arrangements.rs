//! Saved restoration intent and identity-verified observations, not effect authority.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::codec::{object, objects};
use crate::frontend::herdr::{HerdrAgent, HerdrTopology};
use crate::model::{NamingMode, Session, Stream};

use super::conversations::{Harness, NativeIdentity, NativeSessionMetadata, native_identity};
use super::{Result, RuntimeError, decode, invalid, text};

/// A snapshot observation only; this does not authorize native effects.
#[derive(Debug)]
pub struct VerifiedAgent<'a> {
    pub native: &'a HerdrAgent,
    pub harness: Harness,
    pub identity: NativeIdentity,
}

/// Verify the entire requested scope before returning any observations.
/// `None` checks every supported host conversation, not just catalog members.
pub fn verified_inventory<'a>(
    topology: &'a HerdrTopology,
    workspace: Option<&str>,
) -> Result<Vec<VerifiedAgent<'a>>> {
    verified_inventory_with_caller(topology, workspace, None)
}

pub(super) fn verified_inventory_from_caller<'a>(
    caller: &'a super::native::PiCallerObservation,
    workspace: &str,
) -> Result<Vec<VerifiedAgent<'a>>> {
    verified_inventory_with_caller(
        caller.snapshot().topology(),
        Some(workspace),
        Some(caller.caller()),
    )
}

fn verified_inventory_with_caller<'a>(
    topology: &'a HerdrTopology,
    workspace: Option<&str>,
    cached: Option<&super::native::NativeCaller>,
) -> Result<Vec<VerifiedAgent<'a>>> {
    let mut verified = Vec::new();
    let mut seen = BTreeSet::new();
    for native in &topology.agents {
        if workspace.is_some_and(|id| native.workspace_id != id) {
            continue;
        }
        let Some(harness) = native
            .native
            .get("agent")
            .and_then(|v| v.as_str())
            .and_then(|name| name.parse::<Harness>().ok())
        else {
            continue;
        };
        let identity = (|| {
            if let Some(caller) = cached.filter(|caller| caller.pane_id() == native.pane_id) {
                if harness != Harness::Pi
                    || caller.terminal_id() != native.terminal_id
                    || caller.workspace_id() != native.workspace_id
                    || caller.tab_id() != native.tab_id
                {
                    return Err(invalid("cached caller differs from its snapshot agent"));
                }
                return Ok(NativeIdentity {
                    id: caller.native_session_id().to_owned(),
                    locator: caller.locator().map(std::path::Path::to_path_buf),
                });
            }
            let metadata = native
                .native
                .get("agent_session")
                .ok_or_else(|| invalid("missing native agent_session"))?;
            let metadata: NativeSessionMetadata =
                decode(&serde_json::to_vec(metadata).map_err(|error| invalid(error.to_string()))?)?;
            native_identity(&metadata, harness)
        })()
        .map_err(|error| {
            invalid(format!(
                "Herdr pane {} (workspace {}, tab {}): {error}",
                native.pane_id, native.workspace_id, native.tab_id
            ))
        })?;
        if !seen.insert((harness.to_string(), identity.id.clone())) {
            return Err(invalid("ambiguous live native conversation identity"));
        }
        verified.push(VerifiedAgent {
            native,
            harness,
            identity,
        });
    }
    Ok(verified)
}

/// Only plan fields are represented here; persistence owns the outer version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Arrangement {
    stream_id: String,
    workspace_id: String,
    tabs: Vec<ArrangementTab>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArrangementWire {
    stream_id: String,
    workspace_id: String,
    #[serde(deserialize_with = "objects")]
    tabs: Vec<ArrangementTab>,
}

impl<'de> Deserialize<'de> for Arrangement {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::try_from(object::<ArrangementWire, _>(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

impl TryFrom<ArrangementWire> for Arrangement {
    type Error = RuntimeError;
    fn try_from(value: ArrangementWire) -> Result<Self> {
        Self::new(value.stream_id, value.workspace_id, value.tabs)
    }
}

impl Arrangement {
    pub fn new(stream_id: String, workspace_id: String, tabs: Vec<ArrangementTab>) -> Result<Self> {
        text(&stream_id)?;
        text(&workspace_id)?;
        let mut seen = BTreeSet::new();
        for session in tabs.iter().flat_map(|tab| &tab.sessions) {
            if !seen.insert(&session.session_id) {
                return Err(invalid("duplicate saved arrangement session identity"));
            }
        }
        Ok(Self {
            stream_id,
            workspace_id,
            tabs,
        })
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
    pub fn tabs(&self) -> &[ArrangementTab] {
        &self.tabs
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArrangementTab {
    tab_id: String,
    label: Option<String>,
    label_mode: NamingMode,
    sessions: Vec<ArrangedSession>,
}

#[derive(Deserialize)]
struct ArrangementTabWire {
    tab_id: String,
    label: Option<String>,
    label_mode: NamingMode,
    #[serde(deserialize_with = "objects")]
    sessions: Vec<ArrangedSession>,
}

impl<'de> Deserialize<'de> for ArrangementTab {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::try_from(object::<ArrangementTabWire, _>(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

impl TryFrom<ArrangementTabWire> for ArrangementTab {
    type Error = RuntimeError;
    fn try_from(value: ArrangementTabWire) -> Result<Self> {
        Self::new(value.tab_id, value.label, value.label_mode, value.sessions)
    }
}

impl ArrangementTab {
    pub fn new(
        tab_id: String,
        label: Option<String>,
        label_mode: NamingMode,
        sessions: Vec<ArrangedSession>,
    ) -> Result<Self> {
        text(&tab_id)?;
        if label
            .as_ref()
            .is_some_and(|label| label.chars().any(|c| c < ' ' || c == '\u{7f}'))
        {
            return Err(invalid("invalid saved arrangement tab label"));
        }
        Ok(Self {
            tab_id,
            label,
            label_mode,
            sessions,
        })
    }

    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }
    pub fn label(&self) -> Option<&str> {
        self.label.as_deref()
    }
    pub fn label_mode(&self) -> NamingMode {
        self.label_mode
    }
    pub fn sessions(&self) -> &[ArrangedSession] {
        &self.sessions
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArrangedSession {
    session_id: String,
    harness: String,
    native_id: String,
    locator: Option<String>,
    pane_id: Option<String>,
    terminal_id: Option<String>,
}

#[derive(Deserialize)]
struct ArrangedSessionWire {
    session_id: String,
    harness: String,
    native_id: String,
    locator: Option<String>,
    pane_id: Option<String>,
    terminal_id: Option<String>,
}

impl<'de> Deserialize<'de> for ArrangedSession {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::try_from(object::<ArrangedSessionWire, _>(deserializer)?)
            .map_err(serde::de::Error::custom)
    }
}

impl TryFrom<ArrangedSessionWire> for ArrangedSession {
    type Error = RuntimeError;
    fn try_from(value: ArrangedSessionWire) -> Result<Self> {
        Self::new(
            value.session_id,
            value.harness,
            value.native_id,
            value.locator,
            value.pane_id,
            value.terminal_id,
        )
    }
}

impl ArrangedSession {
    pub fn new(
        session_id: String,
        harness: String,
        native_id: String,
        locator: Option<String>,
        pane_id: Option<String>,
        terminal_id: Option<String>,
    ) -> Result<Self> {
        text(&session_id)?;
        text(&harness)?;
        text(&native_id)?;
        if let Some(locator) = &locator {
            text(locator)?;
        }
        Ok(Self {
            session_id,
            harness,
            native_id,
            locator,
            pane_id,
            terminal_id,
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn harness(&self) -> &str {
        &self.harness
    }
    pub fn native_id(&self) -> &str {
        &self.native_id
    }
    pub fn locator(&self) -> Option<&str> {
        self.locator.as_deref()
    }
    pub fn pane_id(&self) -> Option<&str> {
        self.pane_id.as_deref()
    }
    pub fn terminal_id(&self) -> Option<&str> {
        self.terminal_id.as_deref()
    }
}

/// Capture only observed catalog conversations in the bound workspace, in native order.
pub fn capture(
    topology: &HerdrTopology,
    workspace_id: &str,
    stream: &Stream,
    sessions: &[Session],
    host_id: &str,
) -> Result<Arrangement> {
    if !topology
        .workspaces
        .iter()
        .any(|workspace| workspace.id == workspace_id)
    {
        return Err(invalid("bound workspace absent from verified snapshot"));
    }
    let agents = verified_inventory(topology, Some(workspace_id))?;
    capture_observed(topology, workspace_id, stream, sessions, host_id, &agents)
}

/// Assemble only from the retained inventory; no identity files are opened here.
pub(super) fn capture_observed<'s>(
    topology: &HerdrTopology,
    workspace_id: &str,
    stream: &Stream,
    sessions: impl IntoIterator<Item = &'s Session>,
    host_id: &str,
    agents: &[VerifiedAgent<'_>],
) -> Result<Arrangement> {
    let mut known = BTreeMap::new();
    for session in sessions
        .into_iter()
        .filter(|s| s.stream_id == stream.id && s.host_id == host_id)
    {
        if known
            .insert(
                (
                    session.harness.as_str(),
                    session.harness_session_id.as_str(),
                ),
                session,
            )
            .is_some()
        {
            return Err(invalid("ambiguous catalog native conversation identity"));
        }
    }
    let mut tabs = Vec::new();
    for tab in topology
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == workspace_id)
    {
        let layout = topology
            .layouts
            .iter()
            .find(|layout| layout.tab_id == tab.id)
            .ok_or_else(|| invalid("missing Herdr tab layout"))?;
        let mut entries = Vec::new();
        for pane in &layout.panes {
            for agent in agents.iter().filter(|agent| &agent.native.pane_id == pane) {
                let Some(session) =
                    known.get(&(agent.harness.as_ref(), agent.identity.id.as_str()))
                else {
                    continue;
                };
                let locator = agent
                    .identity
                    .locator
                    .as_ref()
                    .map(|path| {
                        path.to_str()
                            .map(str::to_owned)
                            .ok_or_else(|| invalid("saved conversation locator is not UTF-8"))
                    })
                    .transpose()?;
                entries.push(ArrangedSession::new(
                    session.id.clone(),
                    session.harness.clone(),
                    session.harness_session_id.clone(),
                    locator,
                    Some(agent.native.pane_id.clone()),
                    Some(agent.native.terminal_id.clone()),
                )?);
            }
        }
        if entries.is_empty() {
            continue;
        }
        let label = tab
            .label
            .as_deref()
            .ok_or_else(|| invalid("missing Herdr tab label"))?;
        let automatic =
            label.is_empty() || tab.number.is_some_and(|number| label == number.to_string());
        tabs.push(ArrangementTab::new(
            tab.id.clone(),
            if automatic {
                None
            } else {
                Some(label.to_owned())
            },
            if automatic {
                NamingMode::Automatic
            } else {
                NamingMode::Manual
            },
            entries,
        )?);
    }
    Arrangement::new(stream.id.clone(), workspace_id.to_owned(), tabs)
}
