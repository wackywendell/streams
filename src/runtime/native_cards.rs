//! One native pass: supplied records, one snapshot, direct effects. No launch history.

mod actions;
mod artifacts;
mod card_lock;
mod config;
pub mod control;
mod display;
mod documents;
mod launch_context;
mod owner;
mod projection;
#[cfg(test)]
mod tests;

pub use card_lock::CardLock;
pub use config::NativeCardsConfig;
pub use display::{CardPlacement, DisplayLifetime};
pub use launch_context::LaunchedCardContext;
pub use owner::PiOwner;
pub use projection::{
    AssociationIntent, CompanionDisposition, CompanionIntent, DesiredProjection, InactiveReason,
    LifecycleEvent, LifecycleReport, NameIntent, NameOutcome, NameReport, NativeStatus,
    ProjectionResult, SessionProjection, StreamDisplay, publish_card_marker,
};

use super::{Result, digest, invalid, native_markers::stream_tokens};
use crate::{
    frontend::{
        CommandEvidence, FrontendError,
        herdr::{HerdrClient, HerdrInventoryPane, HerdrNativeSnapshot},
    },
    tui::{CardRecords, CardSnapshot, DocumentTarget, EditorIdentity, Scope},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const PLUGIN_ID: &str = "streams-rust-work-cards";
pub const SOURCE_ENV_KEYS: [&str; 14] = [
    "EDITOR",
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "PI_CODING_AGENT_DIR",
    "CODEX_HOME",
    "CLAUDE_CONFIG_DIR",
    "PI_SESSION_DIR",
    "PATH",
    "HERDR_BIN_PATH",
    "HERDR_SOCKET_PATH",
    "HERDR_SESSION",
    "PI_OFFLINE",
];
type Tokens = BTreeMap<String, String>;

pub trait NativeCardSource {
    fn read_records(&mut self, owner: &HerdrInventoryPane) -> Result<CardRecords>;
    fn read(&mut self, owner: &HerdrInventoryPane) -> Result<CardSnapshot>;
    fn resolve_document(&mut self, target: &DocumentTarget) -> Result<ResolvedEditor>;
    fn launch_context(&self) -> SourceLaunch;
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLaunch {
    pub environment: BTreeMap<String, String>,
    pub cwd: PathBuf,
}
#[derive(Clone, PartialEq, Eq)]
pub struct ResolvedEditor {
    pub root: PathBuf,
    pub identity: EditorIdentity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenMode {
    Automatic,
    Explicit,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayOutcome {
    Opened,
    Existing,
    Suppressed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DisplayCondition {
    Absent,
    Ready,
    Separated,
}
#[derive(Debug, Serialize)]
pub struct NativeCardStatus {
    pub condition: DisplayCondition,
    pub owner_terminal_id: String,
    pub session_id: String,
    pub pane_id: Option<String>,
}
pub struct NativeOwnerView {
    pub card: CardRecords,
    pub owner: PiOwner,
}

/// Catalog and Session key for lifetime exclusion, never a launch receipt.
pub fn conversation_key(scope: &Scope) -> Result<String> {
    for value in [&scope.catalog_source, &scope.host_id, &scope.session_id] {
        super::text(value)?;
    }
    json_digest(&(&scope.catalog_source, &scope.host_id, &scope.session_id))
}
fn json_digest(value: &impl Serialize) -> Result<String> {
    Ok(digest(
        &serde_json::to_vec(value).map_err(|error| invalid(error.to_string()))?,
    ))
}
fn card_tokens(scope: &Scope) -> Tokens {
    let mut tokens = stream_tokens(&scope.catalog_source, &scope.host_id, &scope.stream_id);
    tokens.remove("strm_stream");
    tokens.insert("strm_session".into(), scope.session_id.clone());
    tokens.insert("strm_role".into(), "card".into());
    tokens
}
fn marked_card<'a>(
    snapshot: &'a HerdrNativeSnapshot,
    scope: &Scope,
) -> Option<&'a HerdrInventoryPane> {
    let desired = card_tokens(scope);
    snapshot.topology().panes.iter().find(|pane| {
        snapshot.pane_tokens(&pane.pane.id).is_some_and(|tokens| {
            desired
                .iter()
                .all(|(key, value)| tokens.get(key) == Some(value))
        })
    })
}

/// Created for one pass and then discarded. Effects never fetch another native snapshot.
pub struct NativeCardEngine {
    client: HerdrClient,
    source: Box<dyn NativeCardSource>,
    snapshot: HerdrNativeSnapshot,
    owner: PiOwner,
    owner_pane: HerdrInventoryPane,
    card: CardRecords,
    display: Option<HerdrInventoryPane>,
    config: NativeCardsConfig,
    environment: BTreeMap<String, String>,
    cancelled: Arc<AtomicBool>,
    commands: Vec<CommandEvidence>,
}
impl NativeCardEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: HerdrClient,
        mut source: Box<dyn NativeCardSource>,
        snapshot: HerdrNativeSnapshot,
        owner: PiOwner,
        config: NativeCardsConfig,
        environment: BTreeMap<String, String>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self> {
        let owner_pane = owner
            .locate(&snapshot)?
            .ok_or_else(|| invalid("Pi exited or changed conversation"))?
            .clone();
        let card = source.read_records(&owner_pane)?;
        if card.scope().harness != "pi"
            || card.scope().native_session_id != owner.native_session_id
            || card.owner() != &owner::location(&owner_pane)
        {
            return Err(invalid(
                "card source selected another conversation or location",
            ));
        }
        let display = marked_card(&snapshot, card.scope()).cloned();
        Ok(Self {
            client,
            source,
            snapshot,
            owner,
            owner_pane,
            card,
            display,
            config,
            environment,
            cancelled,
            commands: Vec::new(),
        })
    }
    pub fn client(&self) -> &HerdrClient {
        &self.client
    }
    fn card_caption(&self) -> String {
        format!(
            "{}: {}",
            self.card.stream().title,
            self.card.session().title
        )
    }
    pub fn observe_owner(&self) -> Result<NativeOwnerView> {
        Ok(NativeOwnerView {
            card: self.card.clone(),
            owner: self.owner.clone(),
        })
    }
    pub fn watched_ids(
        &self,
    ) -> (
        std::collections::BTreeSet<String>,
        std::collections::BTreeSet<String>,
        std::collections::BTreeSet<String>,
    ) {
        let panes = std::iter::once(&self.owner_pane).chain(self.display.iter());
        let mut ids = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        for pane in panes {
            ids.0.insert(pane.pane.id.clone());
            ids.1.insert(pane.pane.tab_id.clone());
            ids.2.insert(pane.pane.workspace_id.clone());
        }
        ids
    }
    pub fn verify_display(&self) -> Result<CardRecords> {
        self.display
            .as_ref()
            .ok_or_else(|| invalid("Session's card is not observed"))?;
        Ok(self.card.clone())
    }
    pub fn status(&self) -> Result<NativeCardStatus> {
        let pane = self.display.as_ref();
        Ok(NativeCardStatus {
            condition: match pane {
                None => DisplayCondition::Absent,
                Some(pane) if pane.pane.tab_id == self.owner_pane.pane.tab_id => {
                    DisplayCondition::Ready
                }
                Some(_) => DisplayCondition::Separated,
            },
            owner_terminal_id: self.owner.terminal_id.clone(),
            session_id: self.card.scope().session_id.clone(),
            pane_id: pane.map(|pane| pane.pane.id.clone()),
        })
    }
    fn call(&mut self, method: &str, params: Value, expected: &str, effect: bool) -> Result<Value> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(invalid("native operation cancelled"));
        }
        let (value, commands) = self
            .client
            .card_request(method, params, Some(expected), effect)
            .map_err(|mut error| {
                let mut prior = self.commands.clone();
                prior.append(&mut error.commands);
                error.commands = prior;
                error
            })?;
        self.commands.extend(commands);
        Ok(value)
    }
    fn refuse(&self, message: &str) -> super::RuntimeError {
        FrontendError {
            message: message.into(),
            commands: self.commands.clone(),
        }
        .into()
    }
}
