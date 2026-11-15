//! Core-owned association snapshots and one-write changes; the app owns layout.
pub mod labels;
pub mod launch;
pub mod sources;
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

/// The SDK's published selection. Process facts validate its route, not its contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PiSourceClaim {
    pub version: u32,
    pub endpoint: EndpointId,
    pub conversation: crate::integration::ConversationId,
    pub terminal_root: crate::frontend::hc::processes::ProcessKey,
    pub sdk: crate::frontend::hc::processes::ProcessKey,
    pub helper: crate::frontend::hc::processes::ProcessKey,
}

fn decode_claim(
    label_key: &str,
    label_value: &str,
    endpoint: &EndpointId,
    terminal_root_pid: u32,
) -> Result<PiSourceClaim, FrontendError> {
    let unavailable = || invalid("Pi source route is unavailable");
    let value = crate::codec::load_json(label_value).map_err(|_| unavailable())?;
    let claim: PiSourceClaim = serde_json::from_value(value).map_err(|_| unavailable())?;
    if claim.version != 1
        || claim.endpoint != *endpoint
        || claim.conversation.harness != "pi"
        || crate::model::check_text(&claim.conversation.native_id, "conversation", false).is_err()
        || claim.terminal_root.pid != terminal_root_pid
        || claim.helper.pid == claim.sdk.pid
        || claim.helper.pid == claim.terminal_root.pid
        || labels::source_key(&claim.helper).map_err(|_| unavailable())? != label_key
    {
        return Err(unavailable());
    }
    for key in [&claim.terminal_root, &claim.sdk, &claim.helper] {
        key.validate().map_err(|_| unavailable())?;
        if key.boot != claim.terminal_root.boot {
            return Err(unavailable());
        }
    }
    Ok(claim)
}

/// Validate a single claim against the retained terminal root and fresh owner facts.
/// This does not classify the SDK launcher, establish claim uniqueness or prove absence.
pub fn verify_claim_route(
    label_key: &str,
    label_value: &str,
    endpoint: &EndpointId,
    terminal_root_pid: u32,
    observer: &dyn crate::frontend::hc::processes::ProcessObserver,
) -> Result<PiSourceClaim, FrontendError> {
    let unavailable = || invalid("Pi source route is unavailable");
    let claim = decode_claim(label_key, label_value, endpoint, terminal_root_pid)?;
    let mut observed = BTreeMap::new();
    let helper = observer
        .observe(claim.helper.pid)
        .map_err(|_| unavailable())?;
    if helper.key != claim.helper || helper.parent_pid != claim.sdk.pid {
        return Err(unavailable());
    }
    observed.insert(helper.key.pid, helper);
    let mut pid = claim.sdk.pid;
    loop {
        if observed.len() >= 65_536 || observed.contains_key(&pid) {
            return Err(unavailable());
        }
        let process = observer.observe(pid).map_err(|_| unavailable())?;
        if process.key.pid != pid
            || process.key.validate().is_err()
            || process.key.boot != claim.terminal_root.boot
            || (pid == claim.sdk.pid && process.key != claim.sdk)
            || (pid == terminal_root_pid && process.key != claim.terminal_root)
        {
            return Err(unavailable());
        }
        let parent = process.parent_pid;
        observed.insert(pid, process);
        if pid == terminal_root_pid {
            break;
        }
        if parent == 0 {
            return Err(unavailable());
        }
        pid = parent;
    }
    for (pid, expected) in observed {
        if observer.observe(pid).map_err(|_| unavailable())? != expected {
            return Err(unavailable());
        }
    }
    Ok(claim)
}

/// Resolve only identities published on this terminal. Unregistered processes are outside this check.
pub fn registered_foreground(
    terminal: &NativeRef,
    value: Value,
    observer: &dyn crate::frontend::hc::processes::ProcessObserver,
) -> crate::integration::ForegroundConversation {
    use crate::integration::ForegroundConversation;
    let unknown = || ForegroundConversation::Unknown {
        reason: "registered foreground context is unavailable".into(),
    };
    #[derive(Deserialize)]
    struct Terminal {
        address: String,
        state: String,
        #[serde(deserialize_with = "crate::codec::required_nullable")]
        pid: Option<u32>,
        labels: BTreeMap<String, String>,
        #[serde(deserialize_with = "crate::codec::required_nullable")]
        foreground: Option<Foreground>,
    }
    #[derive(Deserialize)]
    struct Foreground {
        pid: u32,
    }
    let Ok(native) = serde_json::from_value::<Terminal>(value) else {
        return unknown();
    };
    if terminal.adapter != FrontendId::hc()
        || native.address != terminal.key
        || native.state != "live"
    {
        return unknown();
    }
    let claims: Vec<_> = native
        .labels
        .iter()
        .filter(|(key, _)| key.starts_with("streams.pi."))
        .collect();
    if claims.is_empty() {
        return ForegroundConversation::NotApplicable;
    }
    let (Some(root), Some(foreground)) = (native.pid, native.foreground) else {
        return unknown();
    };
    if root == 0 || foreground.pid == 0 {
        return unknown();
    }
    let mut sdk_keys = BTreeSet::new();
    let mut current = None;
    for (key, value) in claims {
        let Ok(candidate) = decode_claim(key, value, &terminal.endpoint, root) else {
            return unknown();
        };
        match observer.exited(&candidate.helper) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(_) => return unknown(),
        }
        let Ok(claim) = verify_claim_route(key, value, &terminal.endpoint, root, observer) else {
            return unknown();
        };
        if !sdk_keys.insert(claim.sdk.clone()) {
            return unknown();
        }
        let Ok(sdk) = observer.observe(claim.sdk.pid) else {
            return unknown();
        };
        if sdk.key != claim.sdk {
            return unknown();
        }
        if !sdk.stopped
            && sdk.process_group == foreground.pid
            && current.replace(claim.conversation).is_some()
        {
            return unknown();
        }
    }
    match current {
        Some(conversation) => ForegroundConversation::Current(conversation),
        None => ForegroundConversation::NotApplicable,
    }
}

/// A complete point-validated view of registered sources on one core. It does not
/// discover unregistered Pi processes or supply Stream placement evidence.
pub struct RegisteredSources {
    endpoint: EndpointId,
    running: BTreeMap<crate::integration::ConversationId, Vec<NativeRef>>,
    routing: crate::integration::Access<Vec<WorkRoute>>,
    placement_hints: BTreeSet<String>,
}

struct WorkRoute {
    scope: WorkScope,
    stream: String,
    terminal: NativeRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkLabel {
    version: u32,
    scope: WorkScope,
    stream: String,
}

/// The same retained source observation scoped to one admitted catalog and host.
pub struct HcWorkExecution {
    sources: RegisteredSources,
    scope: WorkScope,
}
impl RegisteredSources {
    pub fn decode(
        endpoint: &EndpointId,
        value: Value,
        observer: &dyn crate::frontend::hc::processes::ProcessObserver,
    ) -> Result<Self, FrontendError> {
        let unavailable = || invalid("registered Pi sources are unavailable");
        let terminals = value
            .get("terminals")
            .and_then(Value::as_array)
            .ok_or_else(unavailable)?;
        let mut seen_terminals = BTreeSet::new();
        let mut seen_sdks = BTreeSet::new();
        let mut running: BTreeMap<_, Vec<NativeRef>> = BTreeMap::new();
        let mut routes = Vec::new();
        let mut routing_available = true;
        let mut placement_hints = BTreeSet::new();
        for value in terminals {
            let address = value
                .get("address")
                .and_then(Value::as_str)
                .filter(|address| !address.is_empty())
                .ok_or_else(unavailable)?;
            // hc's list also includes configured machines. They are not this owner core.
            if address.contains('/') {
                continue;
            }
            #[derive(Deserialize)]
            struct Terminal {
                state: String,
                pid: Option<u32>,
                labels: BTreeMap<String, String>,
            }
            let terminal: Terminal =
                serde_json::from_value(value.clone()).map_err(|_| unavailable())?;
            if terminal.state != "live" || !seen_terminals.insert(address.to_owned()) {
                return Err(unavailable());
            }
            if terminal.labels.contains_key("herdr.space")
                || terminal.labels.contains_key("herdr.tab")
            {
                placement_hints.insert(address.to_owned());
            }
            if let Some(label) = terminal.labels.get("streams.work") {
                let route = crate::codec::load_json(label)
                    .ok()
                    .and_then(|value| serde_json::from_value::<WorkLabel>(value).ok())
                    .filter(|route| {
                        route.version == 1
                            && route.scope.validate().is_ok()
                            && crate::model::check_text(&route.stream, "stream", false).is_ok()
                    });
                match route {
                    Some(route) => routes.push(WorkRoute {
                        scope: route.scope,
                        stream: route.stream,
                        terminal: NativeRef {
                            adapter: FrontendId::hc(),
                            endpoint: endpoint.clone(),
                            key: address.into(),
                        },
                    }),
                    None => routing_available = false,
                }
            }
            for (key, label) in terminal
                .labels
                .iter()
                .filter(|(key, _)| key.starts_with("streams.pi."))
            {
                let root = terminal
                    .pid
                    .filter(|pid| *pid != 0)
                    .ok_or_else(unavailable)?;
                let candidate = decode_claim(key, label, endpoint, root)?;
                if observer
                    .exited(&candidate.helper)
                    .map_err(|_| unavailable())?
                {
                    continue;
                }
                let claim = verify_claim_route(key, label, endpoint, root, observer)?;
                if !seen_sdks.insert(claim.sdk) {
                    return Err(unavailable());
                }
                running
                    .entry(claim.conversation)
                    .or_default()
                    .push(NativeRef {
                        adapter: FrontendId::hc(),
                        endpoint: endpoint.clone(),
                        key: address.into(),
                    });
            }
        }
        let routing = if routing_available {
            crate::integration::Access::Available { value: routes }
        } else {
            crate::integration::Access::Unavailable {
                reason: "Stream routing is unavailable".into(),
            }
        };
        Ok(Self {
            endpoint: endpoint.clone(),
            running,
            routing,
            placement_hints,
        })
    }
    pub fn for_scope(self, scope: WorkScope) -> Result<HcWorkExecution, FrontendError> {
        scope
            .validate()
            .map_err(|_| invalid("work scope is unavailable"))?;
        Ok(HcWorkExecution {
            sources: self,
            scope,
        })
    }
    pub fn presence(
        &self,
        conversation: &crate::integration::ConversationId,
    ) -> crate::integration::Presence {
        use crate::integration::{NonEmpty, Presence};
        if conversation.harness != "pi" {
            return Presence::Unknown {
                reason: "hc supports registered Pi sources only".into(),
            };
        }
        match self.running.get(conversation) {
            Some(locations) => Presence::Running {
                locations: NonEmpty::try_from(locations.clone())
                    .expect("running locations are nonempty"),
            },
            None => Presence::Absent,
        }
    }
    pub fn verified_terminals(
        &self,
        conversation: &crate::integration::ConversationId,
    ) -> Vec<NativeRef> {
        self.running.get(conversation).cloned().unwrap_or_default()
    }
}
impl crate::integration::ExecutionSnapshot for RegisteredSources {
    fn presence(
        &self,
        conversation: &crate::integration::ConversationId,
    ) -> crate::integration::Presence {
        RegisteredSources::presence(self, conversation)
    }
    fn verified_terminals(
        &self,
        conversation: &crate::integration::ConversationId,
    ) -> Vec<NativeRef> {
        RegisteredSources::verified_terminals(self, conversation)
    }
    fn stream_terminals(&self, _: &str) -> crate::integration::Access<Vec<NativeRef>> {
        crate::integration::Access::Unavailable {
            reason: "Stream routing was not inspected".into(),
        }
    }
    fn foreground_conversation(&self, _: &NativeRef) -> crate::integration::ForegroundConversation {
        crate::integration::ForegroundConversation::Unknown {
            reason: "foreground context was not inspected".into(),
        }
    }
}

impl HcWorkExecution {
    pub fn scope(&self) -> &WorkScope {
        &self.scope
    }

    /// Select only verified requested conversations with neither published nor pending placement.
    pub fn unplaced_selected(
        &self,
        conversations: &[crate::integration::ConversationId],
        presentation: &HcPresentation,
    ) -> Result<Vec<NativeRef>, FrontendError> {
        use crate::integration::{Presence, PresentationSnapshot};
        if self.scope != presentation.scope
            || self.sources.endpoint != presentation.published.endpoint
        {
            return Err(invalid("work and presentation observations do not match"));
        }
        let mut seen = BTreeSet::new();
        let mut selected = Vec::new();
        for conversation in conversations {
            if let Presence::Running { locations } = self.sources.presence(conversation) {
                for terminal in locations.into_vec() {
                    if !self.sources.placement_hints.contains(&terminal.key)
                        && presentation.space_for_terminal(&terminal).is_none()
                        && seen.insert(terminal.key.clone())
                    {
                        selected.push(terminal);
                    }
                }
            }
        }
        Ok(selected)
    }
}

impl crate::integration::ExecutionSnapshot for HcWorkExecution {
    fn presence(
        &self,
        conversation: &crate::integration::ConversationId,
    ) -> crate::integration::Presence {
        self.sources.presence(conversation)
    }
    fn verified_terminals(
        &self,
        conversation: &crate::integration::ConversationId,
    ) -> Vec<NativeRef> {
        self.sources.verified_terminals(conversation)
    }
    fn stream_terminals(&self, stream: &str) -> crate::integration::Access<Vec<NativeRef>> {
        use crate::integration::Access;
        match &self.sources.routing {
            Access::Available { value } => Access::Available {
                value: value
                    .iter()
                    .filter(|route| route.scope == self.scope && route.stream == stream)
                    .map(|route| route.terminal.clone())
                    .collect(),
            },
            Access::Unavailable { reason } => Access::Unavailable {
                reason: reason.clone(),
            },
            Access::Unsupported => Access::Unsupported,
        }
    }
    fn foreground_conversation(&self, _: &NativeRef) -> crate::integration::ForegroundConversation {
        crate::integration::ForegroundConversation::Unknown {
            reason: "foreground context was not inspected".into(),
        }
    }
}

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
    pub fn opening_support() -> crate::integration::OpeningSupport {
        use crate::integration::{LaunchLimit, NonEmpty, OpeningMode, OpeningSupport, Placement};
        OpeningSupport {
            harnesses: NonEmpty::new("pi".into(), []),
            mode: OpeningMode::ReuseObserved,
            launch_limit: LaunchLimit::Many,
            placement: Placement::Independent,
        }
    }
    pub fn new(client: HcClient) -> Self {
        Self {
            client,
            commands: vec![],
        }
    }
    pub fn endpoint(&self) -> &EndpointId {
        self.client.endpoint()
    }
    pub fn foreground(
        &mut self,
        terminal: &NativeRef,
        observer: &dyn crate::frontend::hc::processes::ProcessObserver,
    ) -> crate::integration::ForegroundConversation {
        use crate::integration::ForegroundConversation;
        let unavailable = || ForegroundConversation::Unknown {
            reason: "registered foreground context is unavailable".into(),
        };
        if terminal.adapter != FrontendId::hc() || terminal.endpoint != *self.endpoint() {
            return unavailable();
        }
        let params = json!({"terminal": terminal.key});
        let (frame, result) = self.client.exchange("terminal.get", &params, false);
        match result {
            Ok(value) => {
                self.commands.push(CommandEvidence {
                    request: NativeRequest::Socket {
                        method: "terminal.get".into(),
                        params,
                        response: frame,
                    },
                    outcome: CommandOutcome::ReadOnly,
                });
                registered_foreground(terminal, value, observer)
            }
            Err(failure) => {
                self.commands.extend(failure.error.commands);
                unavailable()
            }
        }
    }
    pub fn registered_sources(
        &mut self,
        observer: &dyn crate::frontend::hc::processes::ProcessObserver,
    ) -> Result<RegisteredSources, FrontendError> {
        let params = json!({});
        let (frame, result) = self.client.exchange("terminal.list", &params, false);
        let value = match result {
            Ok(value) => {
                self.commands.push(CommandEvidence {
                    request: NativeRequest::Socket {
                        method: "terminal.list".into(),
                        params,
                        response: frame,
                    },
                    outcome: CommandOutcome::ReadOnly,
                });
                value
            }
            Err(failure) => {
                self.commands.extend(failure.error.commands);
                return Err(self.error("registered Pi sources are unavailable"));
            }
        };
        RegisteredSources::decode(self.endpoint(), value, observer)
            .map_err(|_| self.error("registered Pi sources are unavailable"))
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
    /// Check one requested placement once before using the retained association version.
    pub fn finish_requested_space(
        &mut self,
        before: &AssociationSnapshot,
        scope: &WorkScope,
        stream: &str,
        space: &NativeRef,
    ) -> crate::integration::PresentationResult {
        use crate::integration::{PresentationResult, PresentationState, SpaceEvidence};
        let unavailable = |reason: &str| PresentationResult {
            space: SpaceEvidence::Requested {
                value: space.clone(),
            },
            state: PresentationState::Unavailable {
                reason: reason.into(),
            },
        };
        if scope.validate().is_err()
            || crate::model::check_text(stream, "Stream", false).is_err()
            || space.validate().is_err()
            || space.adapter != FrontendId::hc()
            || space.key.contains('/')
            || space.endpoint != *self.endpoint()
            || before.endpoint != *self.endpoint()
        {
            return PresentationResult {
                space: SpaceEvidence::None,
                state: PresentationState::Unavailable {
                    reason: "hc placement does not match this operation".into(),
                },
            };
        }
        let published = match self
            .document("herdr-app")
            .and_then(|(_, value)| PublishedLayout::decode(self.endpoint().clone(), value))
        {
            Ok(layout) => layout,
            Err(_) => return unavailable("hc layout is unavailable"),
        };
        if !published.space_keys.contains(&space.key) {
            return PresentationResult {
                space: SpaceEvidence::Requested {
                    value: space.clone(),
                },
                state: PresentationState::Incomplete {
                    reason: "hc space is not yet published".into(),
                },
            };
        }
        let wanted = SpaceAssociation {
            scope: scope.clone(),
            stream: stream.into(),
            space: space.clone(),
        };
        let state = match self.finish_associations(before, &published, Some(&wanted)) {
            Effect::Applied { .. } => PresentationState::Complete,
            Effect::NotApplied { reason } => PresentationState::Incomplete { reason },
            Effect::Uncertain { reason } => PresentationState::Uncertain { reason },
        };
        PresentationResult {
            space: SpaceEvidence::Known {
                value: space.clone(),
            },
            state,
        }
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
pub struct HcForeground {
    endpoint: EndpointId,
    values: BTreeMap<String, crate::integration::ForegroundConversation>,
}
impl HcForeground {
    pub fn new(endpoint: EndpointId) -> Self {
        Self {
            endpoint,
            values: BTreeMap::new(),
        }
    }
    pub fn insert(
        &mut self,
        terminal: &NativeRef,
        value: crate::integration::ForegroundConversation,
    ) {
        if terminal.adapter == FrontendId::hc() && terminal.endpoint == self.endpoint {
            self.values.insert(terminal.key.clone(), value);
        }
    }
}
impl crate::integration::ExecutionSnapshot for HcForeground {
    fn presence(&self, _: &crate::integration::ConversationId) -> crate::integration::Presence {
        crate::integration::Presence::Unknown {
            reason: "work presence was not inspected".into(),
        }
    }
    fn verified_terminals(&self, _: &crate::integration::ConversationId) -> Vec<NativeRef> {
        vec![]
    }
    fn stream_terminals(&self, _: &str) -> crate::integration::Access<Vec<NativeRef>> {
        crate::integration::Access::Unavailable {
            reason: "Stream routing was not inspected".into(),
        }
    }
    fn foreground_conversation(
        &self,
        terminal: &NativeRef,
    ) -> crate::integration::ForegroundConversation {
        if terminal.adapter == FrontendId::hc()
            && terminal.endpoint == self.endpoint
            && let Some(value) = self.values.get(&terminal.key)
        {
            return value.clone();
        }
        crate::integration::ForegroundConversation::Unknown {
            reason: "foreground context was not inspected".into(),
        }
    }
}

pub struct HcPresentation {
    scope: WorkScope,
    before: AssociationSnapshot,
    published: PublishedLayout,
    caller: Option<NativeRef>,
}
impl HcPresentation {
    pub fn scope(&self) -> &WorkScope {
        &self.scope
    }

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
