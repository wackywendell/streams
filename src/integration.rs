//! Shared integration values and pure native planning. Native effects never replay a plan.

use crate::{
    artifacts::{ArtifactKey, ArtifactSummary},
    codec::required_nullable,
    frontend::{EndpointId, FrontendId},
    model::{Session, Status, ValidationError, check_text},
    runtime::Result,
    web_url::SafeHttpUrl,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkScope {
    pub catalog: String,
    pub host: String,
}
impl WorkScope {
    pub fn validate(&self) -> std::result::Result<(), ValidationError> {
        check_text(&self.catalog, "catalog source", false)?;
        check_text(&self.host, "record-owner host", false)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationId {
    pub harness: String,
    pub native_id: String,
}
impl From<&Session> for ConversationId {
    fn from(value: &Session) -> Self {
        Self {
            harness: value.harness.clone(),
            native_id: value.harness_session_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeRef {
    pub adapter: FrontendId,
    pub endpoint: EndpointId,
    pub key: String,
}
impl NativeRef {
    pub fn validate(&self) -> std::result::Result<(), ValidationError> {
        for value in [&self.adapter.0, &self.endpoint.0, &self.key] {
            check_text(value, "native reference", false)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NonEmpty<T>(Vec<T>);
impl<T> NonEmpty<T> {
    pub fn new(first: T, rest: impl IntoIterator<Item = T>) -> Self {
        Self(std::iter::once(first).chain(rest).collect())
    }
    pub fn as_slice(&self) -> &[T] {
        &self.0
    }
    pub fn into_vec(self) -> Vec<T> {
        self.0
    }
}
impl<T> TryFrom<Vec<T>> for NonEmpty<T> {
    type Error = ValidationError;
    fn try_from(values: Vec<T>) -> std::result::Result<Self, Self::Error> {
        if values.is_empty() {
            Err(ValidationError::new("expected a nonempty list"))
        } else {
            Ok(Self(values))
        }
    }
}
impl<T: Serialize> Serialize for NonEmpty<T> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de, T: Deserialize<'de>> Deserialize<'de> for NonEmpty<T> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Self::try_from(Vec::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Access<T> {
    Available { value: T },
    Unsupported,
    Unavailable { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect<T> {
    Applied { value: T },
    NotApplied { reason: String },
    Uncertain { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Presence {
    Running { locations: NonEmpty<NativeRef> },
    Absent,
    Unknown { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentKey {
    pub repository: String,
    pub path: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum LinkKind {
    Issue,
    Pr,
}
crate::codec::deserialize_string_enum!(LinkKind);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkKey {
    pub kind: LinkKind,
    pub url: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentReference {
    pub key: DocumentKey,
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkReference {
    pub key: LinkKey,
    pub description: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub status: Status,
    pub harness: String,
    #[serde(deserialize_with = "required_nullable")]
    pub state_short: Option<String>,
}
impl From<&Session> for SessionSummary {
    fn from(value: &Session) -> Self {
        Self {
            id: value.id.clone(),
            title: value.title.clone(),
            status: value.status,
            harness: value.harness.clone(),
            state_short: value.state_short.clone(),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamArtifacts {
    pub producer: String,
    pub entries: Vec<ArtifactSummary>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamView {
    pub id: String,
    pub title: String,
    #[serde(deserialize_with = "required_nullable")]
    pub goal: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub goal_short: Option<String>,
    pub status: Status,
    pub sessions: Vec<SessionSummary>,
    pub documents: Vec<DocumentReference>,
    pub links: Vec<LinkReference>,
    pub artifacts: Vec<StreamArtifacts>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionView {
    pub id: String,
    pub stream: String,
    pub title: String,
    pub status: Status,
    pub recap: String,
    #[serde(deserialize_with = "required_nullable")]
    pub recap_short: Option<String>,
    pub state: String,
    #[serde(deserialize_with = "required_nullable")]
    pub state_short: Option<String>,
    pub remaining: String,
    #[serde(deserialize_with = "required_nullable")]
    pub remaining_short: Option<String>,
}
impl From<&Session> for SessionView {
    fn from(value: &Session) -> Self {
        Self {
            id: value.id.clone(),
            stream: value.stream_id.clone(),
            title: value.title.clone(),
            status: value.status,
            recap: value.recap.clone(),
            recap_short: value.recap_short.clone(),
            state: value.state.clone(),
            state_short: value.state_short.clone(),
            remaining: value.remaining.clone(),
            remaining_short: value.remaining_short.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reference {
    Document {
        stream: String,
        document: DocumentKey,
        viewing_session: String,
    },
    Artifact {
        key: ArtifactKey,
    },
    Link {
        stream: String,
        link: LinkKey,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationContext {
    Explicit,
    FollowFrame {
        terminal: NativeRef,
        expected_session: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewerAction {
    SelectSpace { space: NativeRef },
    FocusTerminal { terminal: NativeRef },
    RenameSpace { space: NativeRef, name: String },
    RenameTab { tab: NativeRef, name: String },
    OpenUrl { url: SafeHttpUrl },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadReply<T> {
    pub view: T,
    pub viewer_actions: Vec<ViewerAction>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpaceEvidence {
    None,
    Requested { value: NativeRef },
    Known { value: NativeRef },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PresentationState {
    Complete,
    Incomplete { reason: String },
    Unavailable { reason: String },
    Unsupported,
    Uncertain { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentationResult {
    pub space: SpaceEvidence,
    pub state: PresentationState,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionEffect {
    pub terminal: NativeRef,
    pub result: Effect<NativeRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionOpenResult {
    AlreadyRunning { locations: NonEmpty<NativeRef> },
    Launched { location: NativeRef },
    Skipped { reason: String },
    Uncertain { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenReport {
    pub stream: String,
    pub sessions: BTreeMap<String, SessionOpenResult>,
    pub adoptions: Vec<AdoptionEffect>,
    #[serde(deserialize_with = "required_nullable")]
    pub initial_shell: Option<Effect<NativeRef>>,
    pub presentation: PresentationResult,
    pub viewer_actions: Vec<ViewerAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WantedConversation {
    pub conversation: ConversationId,
    pub locator: Option<PathBuf>,
}
pub enum ExecutionQuery {
    Work {
        stream: String,
        wanted: Vec<WantedConversation>,
    },
    Terminals {
        terminals: Vec<NativeRef>,
        foreground: Option<NativeRef>,
    },
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForegroundConversation {
    Current(ConversationId),
    NotApplicable,
    Unknown { reason: String },
}
pub trait ExecutionSnapshot {
    fn presence(&self, conversation: &ConversationId) -> Presence;
    fn verified_terminals(&self, conversation: &ConversationId) -> Vec<NativeRef>;
    fn stream_terminals(&self, stream: &str) -> Access<Vec<NativeRef>>;
    fn foreground_conversation(&self, terminal: &NativeRef) -> ForegroundConversation;
}
pub trait PresentationSnapshot {
    fn contains_space(&self, space: &NativeRef) -> bool;
    fn associated_spaces(&self, stream: &str) -> Vec<NativeRef>;
    fn stream_for_space(&self, space: &NativeRef) -> Option<String>;
    fn caller_space(&self) -> Option<NativeRef>;
    fn space_for_terminal(&self, terminal: &NativeRef) -> Option<NativeRef>;
}
pub trait NativeObservation {
    type Presentation: PresentationSnapshot;
    fn presentation(&self) -> Access<&Self::Presentation>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpeningMode {
    ReuseObserved,
    CreateOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchLimit {
    One,
    Many,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    Independent,
    RequiresSpace,
}
#[derive(Debug, Clone)]
pub struct OpeningSupport {
    pub harnesses: NonEmpty<String>,
    pub mode: OpeningMode,
    pub launch_limit: LaunchLimit,
    pub placement: Placement,
}

impl OpeningSupport {
    pub fn known_harnesses(
        mode: OpeningMode,
        launch_limit: LaunchLimit,
        placement: Placement,
    ) -> Self {
        use strum::VariantNames;
        let harnesses = crate::runtime::conversations::Harness::VARIANTS
            .iter()
            .map(|value| (*value).into())
            .collect::<Vec<_>>();
        Self {
            harnesses: NonEmpty::try_from(harnesses).expect("built-in harness list is nonempty"),
            mode,
            launch_limit,
            placement,
        }
    }
}

/// Classify only the requested conversations. No transcript, directory or native effects occur here.
pub fn classify_open(
    support: &OpeningSupport,
    requested: &[(String, ConversationId)],
    execution: Access<&dyn ExecutionSnapshot>,
) -> Result<(Vec<String>, BTreeMap<String, SessionOpenResult>)> {
    if support.launch_limit == LaunchLimit::One && requested.len() > 1 {
        return Err(crate::runtime::RuntimeError::Invalid(
            "frontend supports one selected Session".into(),
        ));
    }
    let mut candidates = Vec::new();
    let mut results = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for (session, conversation) in requested {
        if !seen.insert(session) {
            return Err(crate::runtime::RuntimeError::Invalid(
                "duplicate requested Session".into(),
            ));
        }
        let result = if !support.harnesses.as_slice().contains(&conversation.harness) {
            Some(SessionOpenResult::Skipped {
                reason: format!("unsupported harness {}", conversation.harness),
            })
        } else if support.mode == OpeningMode::CreateOnly {
            None
        } else {
            let presence = match &execution {
                Access::Available { value } => value.presence(conversation),
                Access::Unsupported => Presence::Unknown {
                    reason: "execution observation is unsupported".into(),
                },
                Access::Unavailable { reason } => Presence::Unknown {
                    reason: reason.clone(),
                },
            };
            match presence {
                Presence::Running { locations } => {
                    Some(SessionOpenResult::AlreadyRunning { locations })
                }
                Presence::Absent => None,
                Presence::Unknown { reason } => Some(SessionOpenResult::Skipped { reason }),
            }
        };
        if let Some(result) = result {
            results.insert(session.clone(), result);
        } else {
            candidates.push(session.clone());
        }
    }
    Ok((candidates, results))
}

#[derive(Debug, Clone)]
pub struct VerifiedLaunch {
    session: String,
    conversation: ConversationId,
    command: crate::runtime::launch::SessionCommand,
    cwd: PathBuf,
    environment: BTreeMap<String, String>,
}
impl VerifiedLaunch {
    /// The shared workflow supplies explicitly approved environment values.
    pub fn prepare(
        session: &Session,
        locator: &std::path::Path,
        roots: &crate::runtime::conversations::ConversationRoots,
        cwd: &std::path::Path,
        environment: BTreeMap<String, String>,
    ) -> Result<Self> {
        let command =
            crate::runtime::launch::SessionCommand::verified(session, Some(locator), roots)?;
        let cwd = crate::runtime::checkouts::canonical_directory(cwd)?;
        Ok(Self::from_verified(session, command, cwd, environment))
    }
    /// Preserve previously verified inputs without reopening the transcript or directory.
    pub(crate) fn from_verified(
        session: &Session,
        command: crate::runtime::launch::SessionCommand,
        cwd: PathBuf,
        environment: BTreeMap<String, String>,
    ) -> Self {
        Self {
            session: session.id.clone(),
            conversation: ConversationId::from(session),
            command,
            cwd,
            environment,
        }
    }
    pub fn session(&self) -> &str {
        &self.session
    }
    pub fn conversation(&self) -> &ConversationId {
        &self.conversation
    }
    pub fn command(&self) -> &crate::runtime::launch::SessionCommand {
        &self.command
    }
    pub fn cwd(&self) -> &std::path::Path {
        &self.cwd
    }
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }
}

#[derive(Debug, Clone)]
pub enum LaunchItem {
    Conversation(VerifiedLaunch),
    Shell { cwd: PathBuf },
}
#[derive(Debug, Clone)]
pub enum SpaceDecision {
    None,
    Reuse { space: NativeRef },
    Create { title: String, cwd: PathBuf },
    Unavailable { reason: String },
}
#[derive(Debug, Clone)]
pub struct PreparedOpen {
    pub stream: String,
    pub space: SpaceDecision,
    pub launches: Vec<LaunchItem>,
    pub adopt: Vec<NativeRef>,
    pub focus: bool,
}
#[derive(Debug, Clone)]
pub struct OpenEffects {
    pub launches: Vec<Effect<NativeRef>>,
    pub adoptions: Vec<AdoptionEffect>,
    pub presentation: PresentationResult,
    pub viewer_actions: Vec<ViewerAction>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameIntent {
    LeaveUnchanged,
    Set(String),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionIntent {
    Keep,
    Show,
    Hide,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssociationIntent {
    Keep,
    Bind { space: NativeRef },
}
#[derive(Debug, Clone)]
pub struct StreamDisplay {
    pub title: String,
    pub goal_short: Option<String>,
}
#[derive(Debug, Clone)]
pub struct SessionProjection {
    pub view: SessionView,
    pub conversation: ConversationId,
    pub name: NameIntent,
    pub companion: CompanionIntent,
}
#[derive(Debug, Clone)]
pub struct DesiredProjection {
    pub stream: String,
    pub space: Option<NativeRef>,
    pub stream_display: StreamDisplay,
    pub association: AssociationIntent,
    pub stream_name: NameIntent,
    pub sessions: Vec<SessionProjection>,
}
impl DesiredProjection {
    /// Saved data and explicit outgoing switches only; no observation or inference.
    pub fn for_session(
        stream: &crate::model::Stream,
        session: &Session,
        space: NativeRef,
        association: AssociationIntent,
        stream_naming: bool,
        session_naming: bool,
        companion: CompanionIntent,
    ) -> Self {
        Self {
            stream: stream.id.clone(),
            space: Some(space),
            stream_display: StreamDisplay {
                title: stream.title.clone(),
                goal_short: stream.goal_short.clone(),
            },
            association,
            stream_name: if stream_naming {
                NameIntent::Set(stream.title.clone())
            } else {
                NameIntent::LeaveUnchanged
            },
            sessions: vec![SessionProjection {
                view: SessionView::from(session),
                conversation: ConversationId::from(session),
                name: if session_naming {
                    NameIntent::Set(session.title.clone())
                } else {
                    NameIntent::LeaveUnchanged
                },
                companion,
            }],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CompanionDisposition {
    Opened,
    Existing,
    Suppressed,
    Deferred,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum InactiveReason {
    OutsideFrontend,
    NotEnrolled,
    Unavailable,
    Stopped,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CompanionVisibility {
    Shown,
    Hidden,
}
crate::codec::deserialize_string_enum!(CompanionDisposition, InactiveReason, CompanionVisibility);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionResult {
    #[serde(deserialize_with = "required_nullable")]
    pub companion: Option<CompanionDisposition>,
    #[serde(deserialize_with = "required_nullable")]
    pub desired_pi_name: Option<String>,
    pub warnings: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeStatus {
    Active,
    Inactive { reason: InactiveReason },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleReport {
    pub native: NativeStatus,
    pub projection: ProjectionResult,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeChangeKind {
    PlacementChanged,
    DisplayChanged,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleEvent {
    Started,
    NewlyBound,
    Reattached,
    Stopping,
    NativeChanged { kind: NativeChangeKind },
}

impl LifecycleEvent {
    pub fn companion_intent(self, automatic: bool) -> CompanionIntent {
        if automatic && matches!(self, Self::Started | Self::NewlyBound) {
            CompanionIntent::Show
        } else {
            CompanionIntent::Keep
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FrameContext {
    Enrolled {
        session: Box<SessionView>,
        companion: CompanionVisibility,
    },
    Unenrolled,
    NotApplicable,
    Unavailable {
        reason: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewingChoice {
    Follow {
        #[serde(deserialize_with = "required_nullable")]
        last_agent: Option<NativeRef>,
    },
    Pinned {
        session: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewingSession {
    Selected { session: String },
    None,
    Unavailable { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SidebarContext {
    Associated {
        stream: Box<StreamView>,
        viewing_session: ViewingSession,
    },
    Unassociated,
    Unavailable {
        reason: String,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReferenceOpenResult {
    Applied,
    NeedsViewer { value: NonEmpty<ViewerAction> },
    NotApplied { reason: String },
    Uncertain { reason: String },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceOpenReport {
    pub known: Vec<NativeRef>,
    pub result: ReferenceOpenResult,
}

pub const MOD_REQUEST_BYTES: usize = 16 * 1024;
pub const MOD_REPLY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewSelection {
    SavedStream {
        stream: String,
    },
    SavedSession {
        session: String,
    },
    Frame {
        terminal: NativeRef,
    },
    Sidebar {
        space: NativeRef,
        viewing: ViewingChoice,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionSelection {
    AllEligible,
    Only { value: NonEmpty<String> },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenRequest {
    pub stream: String,
    pub sessions: SessionSelection,
    #[serde(deserialize_with = "required_nullable")]
    pub cwd: Option<PathBuf>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModAction {
    OpenReference {
        reference: Reference,
        context: ActivationContext,
        #[serde(deserialize_with = "required_nullable")]
        at: Option<NativeRef>,
    },
    OpenWork {
        request: OpenRequest,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModBody {
    Read { selections: NonEmpty<ViewSelection> },
    Act { action: Box<ModAction> },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModRequest {
    pub version: u8,
    pub scope: WorkScope,
    pub endpoint: EndpointId,
    pub body: ModBody,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModReadValue {
    Stream { value: StreamView },
    Session { value: SessionView },
    Frame { context: FrameContext },
    Sidebar { value: SidebarContext },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireError {
    pub kind: String,
    pub message: String,
}
impl WireError {
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self {
            kind: "unavailable".into(),
            message: message.into(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum WireResult<T> {
    Ok(T),
    Error(WireError),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModResult {
    Read {
        values: Vec<WireResult<ModReadValue>>,
        notification_key: String,
    },
    ReferenceOpened {
        value: ReferenceOpenReport,
    },
    WorkOpened {
        value: Box<OpenReport>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModReply {
    pub version: u8,
    pub scope: WorkScope,
    pub endpoint: EndpointId,
    #[serde(deserialize_with = "required_nullable")]
    pub verified_endpoint: Option<EndpointId>,
    pub result: WireResult<ModResult>,
    pub viewer_projection: Vec<ViewerAction>,
    pub warnings: Vec<String>,
}

fn decode_mod_wire<T: serde::de::DeserializeOwned + Serialize>(
    text: &str,
    limit: usize,
) -> std::result::Result<T, ValidationError> {
    let invalid = || ValidationError::new("invalid integration JSON envelope");
    if text.len() > limit {
        return Err(ValidationError::new(
            "integration JSON exceeds its byte limit",
        ));
    }
    let mut decoder = serde_json::Deserializer::from_str(text);
    let data: serde_json::Value = crate::codec::object(&mut decoder).map_err(|_| invalid())?;
    decoder.end().map_err(|_| invalid())?;
    let value: T = serde_json::from_value(data.clone()).map_err(|_| invalid())?;
    // Serde also accepts positional structs and some alternate enum forms. The
    // wire accepts only the named JSON representation emitted by these DTOs.
    if serde_json::to_value(&value).map_err(|_| invalid())? != data {
        return Err(invalid());
    }
    Ok(value)
}
impl ModRequest {
    pub fn decode(text: &str) -> std::result::Result<Self, ValidationError> {
        let value: Self = decode_mod_wire(text, MOD_REQUEST_BYTES)?;
        value.scope.validate()?;
        check_text(&value.endpoint.0, "native endpoint", false)?;
        if value.version != 1 {
            return Err(ValidationError::new("unsupported integration version"));
        }
        Ok(value)
    }
}
impl ModReply {
    pub fn decode(text: &str) -> std::result::Result<Self, ValidationError> {
        let value: Self = decode_mod_wire(text, MOD_REPLY_BYTES)?;
        value.scope.validate()?;
        check_text(&value.endpoint.0, "native endpoint", false)?;
        if value.version != 1 {
            return Err(ValidationError::new("unsupported integration version"));
        }
        Ok(value)
    }
    pub fn encode(&self) -> std::result::Result<String, ValidationError> {
        struct Bounded(Vec<u8>);
        impl std::io::Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                // The CLI adds one trailing newline.
                if self.0.len() + bytes.len() >= MOD_REPLY_BYTES {
                    return Err(std::io::Error::other("integration reply exceeds 4 MiB"));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut output = Bounded(Vec::new());
        serde_json::to_writer(&mut output, self)
            .map_err(|_| ValidationError::new("integration reply exceeds 4 MiB"))?;
        Ok(String::from_utf8(output.0).expect("JSON is UTF-8"))
    }
}
impl WorkScope {
    pub fn notification_key(&self) -> String {
        use sha2::{Digest, Sha256};
        let bytes = format!(
            "[{},{}]",
            crate::codec::ascii_json(&self.catalog),
            crate::codec::ascii_json(&self.host)
        );
        format!("streams.saved.{:x}", Sha256::digest(bytes.as_bytes()))
    }
}

/// Owner-only resolution data; this type is never a desktop request or reply.
pub enum ResolvedTarget {
    Document {
        stream: String,
        document: DocumentKey,
        viewing_session: String,
        path: PathBuf,
        checkout_id: String,
        checkout_root: PathBuf,
    },
    Artifact {
        key: ArtifactKey,
    },
    Link {
        url: SafeHttpUrl,
    },
}

/// Validate a producing key against one freshly supplied Session, even after a move.
pub fn resolve_artifact_summary(
    key: &crate::artifacts::ArtifactKey,
    session: &Session,
) -> Result<crate::artifacts::ArtifactSummary> {
    key.validate()
        .map_err(|error| crate::runtime::invalid(error.to_string()))?;
    if session.id != key.session || session.host_id != key.host {
        return Err(crate::runtime::invalid(
            "saved artifact reference is unavailable",
        ));
    }
    crate::artifacts::summaries(session, &key.stream)
        .into_iter()
        .find(|summary| &summary.key == key)
        .ok_or_else(|| crate::runtime::invalid("saved artifact reference is unavailable"))
}

/// Each operation retains its adapter observation. Accessors do no I/O.
pub trait NativeIntegration {
    type Observation: NativeObservation;
    type Execution: ExecutionSnapshot;
    fn opening_support(&self) -> Option<OpeningSupport>;
    fn observe(&mut self) -> Result<Self::Observation>;
    fn inspect_execution(
        &mut self,
        observed: &Self::Observation,
        query: &ExecutionQuery,
    ) -> Access<Self::Execution>;
    fn execute_open(&mut self, plan: &PreparedOpen) -> OpenEffects;
    fn open_target(
        &mut self,
        target: &ResolvedTarget,
        at: Option<&NativeRef>,
    ) -> ReferenceOpenReport;
}

/// Submit one already-validated plan. Preserve each effect without observing or retrying it.
pub fn apply_prepared_open(
    native: &mut impl NativeIntegration,
    plan: PreparedOpen,
    sessions: BTreeMap<String, SessionOpenResult>,
) -> OpenReport {
    let effects = native.execute_open(&plan);
    complete_open(plan, effects, sessions)
}

/// Map one native submission, including adapters with legacy CLI error reporting.
pub fn complete_open(
    plan: PreparedOpen,
    effects: OpenEffects,
    mut sessions: BTreeMap<String, SessionOpenResult>,
) -> OpenReport {
    assert_eq!(
        effects.launches.len(),
        plan.launches.len(),
        "adapter must return one effect per launch input"
    );
    let mut initial_shell = None;
    for (launch, effect) in plan.launches.iter().zip(effects.launches) {
        match launch {
            LaunchItem::Conversation(launch) => {
                let result = match effect {
                    Effect::Applied { value } => SessionOpenResult::Launched { location: value },
                    Effect::NotApplied { reason } => SessionOpenResult::Skipped { reason },
                    Effect::Uncertain { reason } => SessionOpenResult::Uncertain { reason },
                };
                sessions.insert(launch.session().into(), result);
            }
            LaunchItem::Shell { .. } => {
                assert!(
                    initial_shell.is_none(),
                    "prepared opening has at most one initial shell"
                );
                initial_shell = Some(effect);
            }
        }
    }
    OpenReport {
        stream: plan.stream,
        sessions,
        adoptions: effects.adoptions,
        initial_shell,
        presentation: effects.presentation,
        viewer_actions: effects.viewer_actions,
    }
}
