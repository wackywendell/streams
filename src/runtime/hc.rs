//! Core-owned association snapshots and one-write changes; the app owns layout.
use crate::{
    frontend::{
        CommandEvidence, CommandOutcome, EndpointId, FrontendError, FrontendId, NativeRequest,
        hc::HcClient,
    },
    integration::{Effect, NativeRef, WorkScope},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

const ASSOCIATION_BYTES: usize = 1024 * 1024;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceAssociation {
    pub scope: WorkScope,
    pub stream: String,
    pub space: NativeRef,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssociationDocument {
    pub version: u32,
    pub spaces: BTreeMap<String, SpaceAssociation>,
}
pub struct AssociationSnapshot {
    pub endpoint: EndpointId,
    pub version: u64,
    pub rows: BTreeMap<String, SpaceAssociation>,
}
impl AssociationSnapshot {
    pub fn decode(endpoint: EndpointId, version: u64, value: Value) -> Result<Self, FrontendError> {
        bounded_associations(&value)?;
        let document = if value.is_null() {
            AssociationDocument {
                version: 1,
                spaces: BTreeMap::new(),
            }
        } else {
            serde_json::from_value(value).map_err(|_| invalid("invalid hc association document"))?
        };
        if document.version != 1 {
            return Err(invalid("unsupported hc association version"));
        }
        for (key, row) in &document.spaces {
            row.scope
                .validate()
                .map_err(|_| invalid("invalid association scope"))?;
            crate::model::check_text(&row.stream, "associated Stream", false)
                .map_err(|_| invalid("invalid associated Stream"))?;
            row.space
                .validate()
                .map_err(|_| invalid("invalid associated space"))?;
            if row.space.adapter != FrontendId::hc()
                || row.space.endpoint != endpoint
                || row.space.key != *key
            {
                return Err(invalid("association key or endpoint does not match"));
            }
        }
        Ok(Self {
            endpoint,
            version,
            rows: document.spaces,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishedPane {
    Terminal { key: String, terminal: String },
    Mod { key: String },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedTab {
    pub key: String,
    pub name: String,
    pub panes: Vec<PublishedPane>,
    pub floating_terminals: BTreeSet<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedSpace {
    pub key: String,
    pub name: String,
    pub tabs: Vec<PublishedTab>,
}
#[derive(Debug, Clone)]
pub struct PublishedLayout {
    pub endpoint: EndpointId,
    pub spaces: Vec<PublishedSpace>,
    pub space_keys: BTreeSet<String>,
}
#[derive(Deserialize)]
struct Layout {
    #[serde(default)]
    spaces: Vec<Space>,
}
#[derive(Deserialize, Default)]
#[serde(default)]
struct Space {
    id: String,
    name: String,
    tabs: Vec<Tab>,
}
#[derive(Deserialize)]
struct Tab {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default = "empty_root")]
    root: Node,
}
#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    id: String,
    #[serde(flatten)]
    kind: Kind,
    #[serde(default)]
    floating: Vec<Float>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Kind {
    Block { content: Content },
    Row { children: Vec<Node> },
    Column { children: Vec<Node> },
    Stack { children: Vec<Node> },
}
#[derive(Deserialize)]
struct Float {
    #[serde(rename = "id")]
    _id: String,
    content: Content,
}
#[derive(Deserialize)]
#[serde(untagged)]
enum Content {
    Terminal {
        terminal: String,
    },
    Mod {
        #[serde(rename = "mod")]
        _name: String,
    },
    Context {
        #[serde(rename = "tab", alias = "context")]
        _tab: String,
    },
    Agent {
        #[serde(rename = "agent")]
        _agent: String,
    },
}
fn empty_root() -> Node {
    Node {
        id: String::new(),
        kind: Kind::Row { children: vec![] },
        floating: vec![],
    }
}
impl PublishedLayout {
    pub fn decode(endpoint: EndpointId, value: Value) -> Result<Self, FrontendError> {
        let layout: Layout = if value.is_null() {
            Layout { spaces: vec![] }
        } else {
            serde_json::from_value(value)
                .map_err(|_| invalid("published hc layout is unavailable"))?
        };
        let mut spaces = Vec::new();
        let mut space_keys = BTreeSet::new();
        let mut tab_keys = BTreeSet::new();
        let mut placement = BTreeMap::new();
        for space in layout.spaces {
            if space.id.is_empty() || !space_keys.insert(space.id.clone()) {
                return Err(invalid("invalid or duplicate published space"));
            }
            let mut tabs = Vec::new();
            for tab in space.tabs {
                if tab.id.is_empty() || !tab_keys.insert(tab.id.clone()) {
                    return Err(invalid("invalid or duplicate published tab"));
                }
                let mut published = PublishedTab {
                    key: tab.id,
                    name: tab.name,
                    panes: vec![],
                    floating_terminals: BTreeSet::new(),
                };
                collect_node(tab.root, &mut published)?;
                let terminals = published
                    .panes
                    .iter()
                    .filter_map(|pane| match pane {
                        PublishedPane::Terminal { terminal, .. } => Some(terminal),
                        _ => None,
                    })
                    .chain(published.floating_terminals.iter());
                for terminal in terminals {
                    let location = (space.id.clone(), published.key.clone());
                    if placement
                        .insert(terminal.clone(), location.clone())
                        .is_some_and(|previous| previous != location)
                    {
                        return Err(invalid("conflicting published terminal placements"));
                    }
                }
                tabs.push(published);
            }
            spaces.push(PublishedSpace {
                key: space.id,
                name: space.name,
                tabs,
            });
        }
        Ok(Self {
            endpoint,
            spaces,
            space_keys,
        })
    }
}
fn collect_node(node: Node, tab: &mut PublishedTab) -> Result<(), FrontendError> {
    for float in node.floating {
        if let Content::Terminal { terminal } = float.content {
            if terminal.is_empty() {
                return Err(invalid("empty published terminal"));
            }
            tab.floating_terminals.insert(terminal);
        }
    }
    match node.kind {
        Kind::Block { content } => match content {
            Content::Terminal { terminal } => {
                if terminal.is_empty() {
                    return Err(invalid("empty published terminal"));
                }
                tab.panes.push(PublishedPane::Terminal {
                    key: node.id,
                    terminal,
                });
            }
            Content::Mod { .. } => tab.panes.push(PublishedPane::Mod { key: node.id }),
            Content::Context { .. } | Content::Agent { .. } => {}
        },
        Kind::Row { children } | Kind::Column { children } | Kind::Stack { children } => {
            for child in children {
                collect_node(child, tab)?;
            }
        }
    }
    Ok(())
}

pub fn prepare_associations(
    before: &AssociationSnapshot,
    published: &PublishedLayout,
    wanted: Option<&SpaceAssociation>,
) -> Result<AssociationDocument, FrontendError> {
    if before.endpoint != published.endpoint {
        return Err(invalid("layout belongs to another endpoint"));
    }
    if let Some(wanted) = wanted {
        if wanted.space.adapter != FrontendId::hc() || wanted.space.endpoint != published.endpoint {
            return Err(invalid("space belongs to another endpoint"));
        }
        if !published.space_keys.contains(&wanted.space.key) {
            return Err(invalid("space is absent from the published layout"));
        }
        if before
            .rows
            .get(&wanted.space.key)
            .is_some_and(|existing| existing != wanted)
        {
            return Err(invalid("space is associated with other work"));
        }
    }
    let mut spaces: BTreeMap<_, _> = before
        .rows
        .iter()
        .filter(|(key, _)| published.space_keys.contains(*key))
        .map(|(key, row)| (key.clone(), row.clone()))
        .collect();
    if let Some(wanted) = wanted {
        spaces.insert(wanted.space.key.clone(), wanted.clone());
    }
    Ok(AssociationDocument { version: 1, spaces })
}

pub struct HcOperation {
    client: HcClient,
    pub commands: Vec<CommandEvidence>,
}
impl HcOperation {
    pub fn new(client: HcClient) -> Self {
        Self {
            client,
            commands: vec![],
        }
    }
    pub fn endpoint(&self) -> &EndpointId {
        self.client.endpoint()
    }
    fn error(&self, message: impl Into<String>) -> FrontendError {
        FrontendError {
            message: message.into(),
            commands: self.commands.clone(),
        }
    }
    fn document(&mut self, name: &str) -> Result<(u64, Value), FrontendError> {
        let params = json!({"name":name});
        let (frame, result) = self.client.exchange("doc.get", &params, false);
        let value = match result {
            Ok(value) => {
                self.commands.push(CommandEvidence {
                    request: NativeRequest::Socket {
                        method: "doc.get".into(),
                        params,
                        response: frame,
                    },
                    outcome: CommandOutcome::ReadOnly,
                });
                value
            }
            Err(failure) => {
                self.commands.extend(failure.error.commands);
                return Err(self.error(failure.error.message));
            }
        };
        #[derive(Deserialize)]
        struct Document {
            version: u64,
            value: Value,
        }
        let document: Document =
            serde_json::from_value(value).map_err(|_| self.error("invalid hc document reply"))?;
        Ok((document.version, document.value))
    }
    pub fn snapshots(&mut self) -> Result<(AssociationSnapshot, PublishedLayout), FrontendError> {
        let (version, value) = self.document("streams")?;
        let before = AssociationSnapshot::decode(self.endpoint().clone(), version, value)
            .map_err(|error| self.error(error.message))?;
        let (_, value) = self.document("herdr-app")?;
        let published = PublishedLayout::decode(self.endpoint().clone(), value)
            .map_err(|error| self.error(error.message))?;
        Ok((before, published))
    }
    pub fn finish_associations(
        &mut self,
        before: &AssociationSnapshot,
        published: &PublishedLayout,
        wanted: Option<&SpaceAssociation>,
    ) -> Effect<()> {
        let not_applied = |reason: String| Effect::NotApplied { reason };
        if before.endpoint != *self.endpoint() {
            return not_applied("association belongs to another endpoint".into());
        }
        let desired = match prepare_associations(before, published, wanted) {
            Ok(value) => value,
            Err(error) => return not_applied(error.message),
        };
        if desired.spaces == before.rows {
            return Effect::Applied { value: () };
        }
        let Some(next_version) = before.version.checked_add(1) else {
            return not_applied("association version is unavailable".into());
        };
        let value = match serde_json::to_value(&desired) {
            Ok(value) => value,
            Err(_) => return not_applied("invalid association change".into()),
        };
        if let Err(error) = bounded_associations(&value) {
            return not_applied(error.message);
        }
        let params = json!({"name":"streams","version":before.version,"value":value});
        let (frame, result) = self.client.exchange("doc.set", &params, true);
        match result {
            Ok(value) => {
                let acknowledged =
                    value.get("version").and_then(Value::as_u64) == Some(next_version);
                self.commands.push(CommandEvidence {
                    request: NativeRequest::Socket {
                        method: "doc.set".into(),
                        params,
                        response: frame,
                    },
                    outcome: if acknowledged {
                        CommandOutcome::Acknowledged
                    } else {
                        CommandOutcome::Uncertain
                    },
                });
                if acknowledged {
                    Effect::Applied { value: () }
                } else {
                    Effect::Uncertain {
                        reason: "association write unconfirmed".into(),
                    }
                }
            }
            Err(failure) => {
                let submitted = !failure.error.commands.is_empty();
                self.commands.extend(failure.error.commands);
                if !submitted {
                    not_applied("association write did not start".into())
                } else if failure.rejection.as_deref() == Some("conflict") {
                    not_applied("association conflict".into())
                } else if matches!(
                    failure.rejection.as_deref(),
                    Some("invalid_params" | "invalid_request" | "method_not_found")
                ) {
                    not_applied("association write rejected".into())
                } else {
                    Effect::Uncertain {
                        reason: "association write unconfirmed".into(),
                    }
                }
            }
        }
    }
}
fn bounded_associations(value: &Value) -> Result<(), FrontendError> {
    if serde_json::to_vec(value)
        .map_err(|_| invalid("invalid association document"))?
        .len()
        > ASSOCIATION_BYTES
    {
        return Err(invalid("association document exceeds 1 MiB"));
    }
    Ok(())
}
pub struct HcPresentation {
    scope: WorkScope,
    before: AssociationSnapshot,
    published: PublishedLayout,
    caller: Option<NativeRef>,
}
impl HcPresentation {
    pub fn new(
        scope: WorkScope,
        before: AssociationSnapshot,
        published: PublishedLayout,
        caller: Option<NativeRef>,
    ) -> Result<Self, FrontendError> {
        scope
            .validate()
            .map_err(|_| invalid("invalid presentation scope"))?;
        if before.endpoint != published.endpoint {
            return Err(invalid("presentation belongs to another endpoint"));
        }
        Ok(Self {
            scope,
            before,
            published,
            caller,
        })
    }
    fn local(&self, resource: &NativeRef) -> bool {
        resource.adapter == FrontendId::hc() && resource.endpoint == self.published.endpoint
    }
    fn space_ref(&self, key: &str) -> NativeRef {
        NativeRef {
            adapter: FrontendId::hc(),
            endpoint: self.published.endpoint.clone(),
            key: key.into(),
        }
    }
}
impl crate::integration::PresentationSnapshot for HcPresentation {
    fn contains_space(&self, space: &NativeRef) -> bool {
        self.local(space) && self.published.space_keys.contains(&space.key)
    }
    fn associated_spaces(&self, stream: &str) -> Vec<NativeRef> {
        self.published
            .spaces
            .iter()
            .filter(|space| {
                self.before
                    .rows
                    .get(&space.key)
                    .is_some_and(|row| row.scope == self.scope && row.stream == stream)
            })
            .map(|space| self.space_ref(&space.key))
            .collect()
    }
    fn stream_for_space(&self, space: &NativeRef) -> Option<String> {
        if !self.contains_space(space) {
            return None;
        }
        self.before
            .rows
            .get(&space.key)
            .filter(|row| row.scope == self.scope)
            .map(|row| row.stream.clone())
    }
    fn caller_space(&self) -> Option<NativeRef> {
        self.caller
            .as_ref()
            .filter(|space| self.contains_space(space))
            .cloned()
    }
    fn space_for_terminal(&self, terminal: &NativeRef) -> Option<NativeRef> {
        if !self.local(terminal) {
            return None;
        }
        self.published.spaces.iter().find(|space|space.tabs.iter().any(|tab|
            tab.floating_terminals.contains(&terminal.key)||tab.panes.iter().any(|pane|matches!(pane,PublishedPane::Terminal{terminal:found,..} if found==&terminal.key))
        )).map(|space|self.space_ref(&space.key))
    }
}
impl HcOperation {
    pub fn presentation(
        &mut self,
        scope: &WorkScope,
        caller: Option<NativeRef>,
    ) -> Result<HcPresentation, FrontendError> {
        let (before, published) = self.snapshots()?;
        HcPresentation::new(scope.clone(), before, published, caller)
    }
}

fn invalid(message: impl Into<String>) -> FrontendError {
    FrontendError::invalid(message)
}
