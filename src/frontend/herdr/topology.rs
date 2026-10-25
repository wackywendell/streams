use super::{FrontendError, HerdrPane, HerdrWorkspace};
use crate::frontend::transport::string;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

/// Native tab identity and optional creation/inventory display fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrTab {
    pub id: String,
    pub workspace_id: String,
    /// Creation receipts need not include a label; complete topology must.
    pub label: Option<String>,
    pub number: Option<u64>,
}

impl HerdrTab {
    pub fn parse(value: &Value) -> Result<Self, FrontendError> {
        let label = match value.get("label") {
            None => None,
            Some(Value::String(v)) => Some(v.clone()),
            _ => return Err(FrontendError::invalid("invalid Herdr tab label")),
        };
        let number = match value.get("number") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_u64()
                    .ok_or_else(|| FrontendError::invalid("invalid Herdr tab number"))?,
            ),
        };
        Ok(Self {
            id: string(value, "tab_id")?,
            workspace_id: string(value, "workspace_id")?,
            label,
            number,
        })
    }
}

/// A structurally located pane and its exact native terminal identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrInventoryPane {
    pub pane: HerdrPane,
    pub terminal_id: String,
}

impl HerdrInventoryPane {
    pub fn parse(value: &Value) -> Result<Self, FrontendError> {
        Ok(Self {
            pane: HerdrPane::parse(value)?,
            terminal_id: string(value, "terminal_id")?,
        })
    }
}

/// Native pane ordering only; not saved conversation restoration intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrLayout {
    pub tab_id: String,
    pub workspace_id: String,
    pub panes: Vec<String>,
}

/// Preserves the native agent-session payload for later bounded identity checking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrAgent {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub terminal_id: String,
    pub native: Value,
}

/// A complete structural snapshot, not a pane/session attestation witness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrTopology {
    pub workspaces: Vec<HerdrWorkspace>,
    pub tabs: Vec<HerdrTab>,
    pub panes: Vec<HerdrInventoryPane>,
    pub layouts: Vec<HerdrLayout>,
    pub agents: Vec<HerdrAgent>,
}

impl HerdrTopology {
    /// Reject partial inventories before returning any workspace or pane subset.
    pub fn parse(snapshot: &Value) -> Result<Self, FrontendError> {
        let workspaces = collection(snapshot, "workspaces")?
            .iter()
            .map(HerdrWorkspace::parse)
            .collect::<Result<Vec<_>, _>>()?;
        let tabs = collection(snapshot, "tabs")?
            .iter()
            .map(HerdrTab::parse)
            .collect::<Result<Vec<_>, _>>()?;
        let panes = collection(snapshot, "panes")?
            .iter()
            .map(|v| {
                Ok(HerdrInventoryPane {
                    pane: HerdrPane::parse(v)?,
                    terminal_id: string(v, "terminal_id")?,
                })
            })
            .collect::<Result<Vec<_>, FrontendError>>()?;
        let layouts = collection(snapshot, "layouts")?
            .iter()
            .map(|v| {
                Ok(HerdrLayout {
                    tab_id: string(v, "tab_id")?,
                    workspace_id: string(v, "workspace_id")?,
                    panes: collection(v, "panes")?
                        .iter()
                        .map(|p| string(p, "pane_id"))
                        .collect::<Result<Vec<_>, _>>()?,
                })
            })
            .collect::<Result<Vec<_>, FrontendError>>()?;
        let agents = collection(snapshot, "agents")?
            .iter()
            .map(|v| {
                Ok(HerdrAgent {
                    pane_id: string(v, "pane_id")?,
                    tab_id: string(v, "tab_id")?,
                    workspace_id: string(v, "workspace_id")?,
                    terminal_id: string(v, "terminal_id")?,
                    native: v.clone(),
                })
            })
            .collect::<Result<Vec<_>, FrontendError>>()?;
        let workspace_ids: BTreeSet<_> = workspaces.iter().map(|v| &v.id).collect();
        let tab_index: BTreeMap<_, _> = tabs.iter().map(|v| (&v.id, v)).collect();
        let pane_index: BTreeMap<_, _> = panes.iter().map(|v| (&v.pane.id, v)).collect();
        let layout_ids: BTreeSet<_> = layouts.iter().map(|v| &v.tab_id).collect();
        let terminals: BTreeSet<_> = panes.iter().map(|v| &v.terminal_id).collect();
        if workspace_ids.len() != workspaces.len()
            || tab_index.len() != tabs.len()
            || pane_index.len() != panes.len()
            || layout_ids.len() != layouts.len()
            || terminals.len() != panes.len()
        {
            return Err(FrontendError::invalid(
                "ambiguous Herdr snapshot identities",
            ));
        }
        if tabs
            .iter()
            .any(|t| !workspace_ids.contains(&t.workspace_id) || t.label.is_none())
        {
            return Err(FrontendError::invalid(
                "invalid Herdr snapshot tab workspace/label",
            ));
        }
        for pane in &panes {
            if tab_index
                .get(&pane.pane.tab_id)
                .is_none_or(|tab| tab.workspace_id != pane.pane.workspace_id)
            {
                return Err(FrontendError::invalid(
                    "invalid Herdr snapshot pane ownership",
                ));
            }
        }
        if layout_ids != tab_index.keys().copied().collect() {
            return Err(FrontendError::invalid("incomplete Herdr snapshot layouts"));
        }
        let mut layout_panes = BTreeSet::new();
        for layout in &layouts {
            if tab_index[&layout.tab_id].workspace_id != layout.workspace_id {
                return Err(FrontendError::invalid(
                    "invalid Herdr snapshot layout workspace",
                ));
            }
            for pane_id in &layout.panes {
                if pane_index
                    .get(pane_id)
                    .is_none_or(|pane| pane.pane.tab_id != layout.tab_id)
                    || !layout_panes.insert(pane_id)
                {
                    return Err(FrontendError::invalid("invalid Herdr snapshot layout pane"));
                }
            }
        }
        if layout_panes != pane_index.keys().copied().collect() {
            return Err(FrontendError::invalid(
                "incomplete Herdr snapshot pane inventory",
            ));
        }
        for agent in &agents {
            if pane_index.get(&agent.pane_id).is_none_or(|pane| {
                pane.pane.workspace_id != agent.workspace_id
                    || pane.pane.tab_id != agent.tab_id
                    || pane.terminal_id != agent.terminal_id
            }) {
                return Err(FrontendError::invalid(
                    "invalid Herdr snapshot agent ownership",
                ));
            }
        }
        Ok(Self {
            workspaces,
            tabs,
            panes,
            layouts,
            agents,
        })
    }
}

fn collection<'a>(value: &'a Value, field: &str) -> Result<&'a Vec<Value>, FrontendError> {
    value
        .get(field)
        .and_then(Value::as_array)
        .filter(|v| v.len() <= 10000)
        .ok_or_else(|| {
            FrontendError::invalid(format!(
                "invalid Herdr snapshot {field}; complete positive inventory required"
            ))
        })
}
