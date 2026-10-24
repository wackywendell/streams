//! Native card effects consume fresh application attestation, not painted UI frames.
//!
//! Catalog ownership/enrollment remains in the application. This coordinator owns
//! the separate Herdr resource lifetime and its host-local native writer lock.

mod actions;
mod config;
mod display;
mod documents;
#[cfg(test)]
#[path = "../../tests/support/herdr.rs"]
mod fake_herdr;
mod handover;
mod inspection;
mod launch_context;
mod projection;
mod reset;
#[cfg(test)]
mod tests;

pub use config::NativeCardsConfig;
pub use display::{CardPlacement, DisplayLifetime};
pub use inspection::{DisplayCondition, NativeCardStatus, NativeOwnerContext, NativeOwnerView};
pub use launch_context::LaunchedCardContext;
pub use projection::{NameOutcome, NameReport};
pub use reset::{EvidenceResetReport, reset_evidence};

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::Value;

use super::native::NativeInventory;
use super::native_state::{
    NativeAttempt, NativePhase, NativePurpose, NativeState, NativeStateGuard, Resources,
};
use super::{Result, digest, invalid};
use crate::frontend::herdr::{HerdrClient, HerdrInventoryPane, HerdrNativeSnapshot, PairState};
use crate::frontend::{CommandEvidence, FrontendError};
use crate::tui::{CardSnapshot, DocumentTarget, EditorIdentity, Scope};

const PLUGIN_ID: &str = "streams-rust-work-cards";
const PAIR_KEYS: [&str; 6] = [
    "strm_agent",
    "strm_nonce",
    "strm_pi",
    "strm_card",
    "strm_role",
    "strm_native",
];
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

/// A trusted application adapter must obtain fresh effective authority and exact
/// native Pi attestation on EVERY read. Implementations must not enroll or save.
pub trait NativeCardSource {
    fn read(
        &mut self,
        client: &HerdrClient,
        snapshot: &HerdrNativeSnapshot,
        inventory: bool,
    ) -> Result<CardObservation>;
    fn resolve_document(
        &mut self,
        client: &HerdrClient,
        snapshot: &HerdrNativeSnapshot,
        target: &DocumentTarget,
    ) -> Result<ResolvedEditor>;
    fn launch_context(&self) -> SourceLaunch;
}

#[derive(Clone)]
pub struct SourceLaunch {
    pub environment: BTreeMap<String, String>,
    pub cwd: PathBuf,
}

#[derive(Clone, PartialEq, Eq)]
pub struct CardObservation {
    pub card: CardSnapshot,
    pub handover: Option<HandoverInventory>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct HandoverInventory {
    pub scopes: Vec<Scope>,
    pub native: NativeInventory,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayOutcome {
    Opened(Resources),
    Existing(Resources),
    Suppressed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct NativeTuple {
    pane_id: String,
    terminal_id: String,
    workspace_id: String,
    tab_id: String,
}
impl NativeTuple {
    fn from_pane(pane: &HerdrInventoryPane) -> Self {
        Self {
            pane_id: pane.pane.id.clone(),
            terminal_id: pane.terminal_id.clone(),
            workspace_id: pane.pane.workspace_id.clone(),
            tab_id: pane.pane.tab_id.clone(),
        }
    }
    fn resources(&self) -> Result<Resources> {
        Resources::new(&self.pane_id, &self.terminal_id, &self.tab_id)
    }
    fn matches_resources(&self, resources: &Resources) -> bool {
        self.pane_id == resources.pane_id()
            && self.terminal_id == resources.terminal_id()
            && self.tab_id == resources.tab_id()
    }
}

#[derive(Clone)]
struct CapturedOwner {
    scope: Scope,
    tuple: NativeTuple,
    binding: Option<String>,
}

struct Verified {
    saved: CardObservation,
    snapshot: HerdrNativeSnapshot,
    caller: NativeTuple,
    owner: NativeTuple,
    identity: String,
    key: String,
    binding: Tokens,
}
impl Verified {
    fn owner_pane(&self) -> &HerdrInventoryPane {
        self.snapshot
            .locate_terminal(&self.owner.terminal_id)
            .expect("verified owner tuple")
    }
    fn pair(&self) -> Result<PairState<'_>> {
        Ok(self
            .snapshot
            .classify_pair(&self.identity, &self.owner.terminal_id)?)
    }
    fn owner_tokens(&self) -> &Tokens {
        self.snapshot
            .pane_tokens(&self.owner.pane_id)
            .expect("verified owner tokens")
    }
    fn workspace_tokens(&self) -> &Tokens {
        self.snapshot
            .workspace_tokens(&self.caller.workspace_id)
            .expect("verified workspace tokens")
    }
    fn bound(&self) -> Result<&str> {
        let tokens = self.workspace_tokens();
        if self
            .binding
            .iter()
            .any(|(key, value)| tokens.get(key) != Some(value))
        {
            return Err(invalid("verified native Stream binding required"));
        }
        tokens
            .get("strm_binding")
            .filter(|value| !value.is_empty())
            .map(String::as_str)
            .ok_or_else(|| invalid("verified native binding lifetime required"))
    }
}

/// Field order is the existing JS identity wire contract, not sorted JSON keys.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScopeWire<'a> {
    catalog_source: &'a str,
    host_id: &'a str,
    stream_id: &'a str,
    session_id: &'a str,
    native_session_id: &'a str,
}
impl<'a> From<&'a Scope> for ScopeWire<'a> {
    fn from(scope: &'a Scope) -> Self {
        Self {
            catalog_source: &scope.catalog_source,
            host_id: &scope.host_id,
            stream_id: &scope.stream_id,
            session_id: &scope.session_id,
            native_session_id: &scope.native_session_id,
        }
    }
}
fn json_digest(value: &impl Serialize) -> Result<String> {
    Ok(digest(
        &serde_json::to_vec(value).map_err(|e| invalid(e.to_string()))?,
    ))
}
/// Stable pair/evidence address; membership, catalog IDs and native location are observations.
pub fn conversation_key(scope: &Scope) -> Result<String> {
    conversation_key_for(
        &scope.catalog_source,
        &scope.host_id,
        &scope.harness,
        &scope.native_session_id,
    )
}
fn conversation_key_for(
    catalog_source: &str,
    host_id: &str,
    harness: &str,
    native_session_id: &str,
) -> Result<String> {
    for value in [catalog_source, host_id, harness, native_session_id] {
        super::text(value)?;
    }
    json_digest(&(
        "streams-native-conversation",
        2,
        catalog_source,
        host_id,
        harness,
        native_session_id,
    ))
}
pub struct NativeCardEngine {
    client: HerdrClient,
    source: Box<dyn NativeCardSource>,
    state: Arc<NativeState>,
    config: NativeCardsConfig,
    environment: BTreeMap<String, String>,
    launched: Option<LaunchedCardContext>,
    cancelled: Arc<AtomicBool>,
    caller: Option<NativeTuple>,
    required_caller_terminal: Option<String>,
    expected_owner: Option<(Scope, NativeOwnerContext)>,
    captured: Option<CapturedOwner>,
    commands: Vec<CommandEvidence>,
    observation: Option<HerdrNativeSnapshot>,
    bootstrap: bool,
}
impl NativeCardEngine {
    /// The pinned endpoint, shared by same-process native work.
    pub fn client(&self) -> &HerdrClient {
        &self.client
    }
    pub fn new(
        client: HerdrClient,
        source: Box<dyn NativeCardSource>,
        state: Arc<NativeState>,
        config: NativeCardsConfig,
        environment: BTreeMap<String, String>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self> {
        let launched = LaunchedCardContext::from_environment(&environment)?;
        if let Some(context) = &launched
            && (context.state_directory() != state.directory()
                || std::fs::canonicalize(context.plugin_root())?
                    != std::fs::canonicalize(&config.plugin_root)?)
        {
            return Err(invalid(
                "launched-card state or plugin root differs from its protected context",
            ));
        }
        if launched.as_ref().is_some_and(|context| {
            environment.get("HERDR_PANE_ID") == Some(&context.owner().pane_id)
        }) {
            return Err(invalid("a launched card cannot run as the Pi owner pane"));
        }
        Ok(Self {
            client,
            source,
            state,
            config,
            environment,
            launched,
            cancelled,
            caller: None,
            required_caller_terminal: None,
            expected_owner: None,
            captured: None,
            commands: Vec::new(),
            observation: None,
            bootstrap: false,
        })
    }

    /// Reuse a caller's fresh bootstrap snapshot without spending another native read.
    pub fn from_snapshot(
        client: HerdrClient,
        source: Box<dyn NativeCardSource>,
        state: Arc<NativeState>,
        config: NativeCardsConfig,
        environment: BTreeMap<String, String>,
        cancelled: Arc<AtomicBool>,
        snapshot: HerdrNativeSnapshot,
    ) -> Result<Self> {
        let mut engine = Self::new(client, source, state, config, environment, cancelled)?;
        engine.verify_caller(&snapshot)?;
        engine.observation = Some(snapshot);
        engine.bootstrap = true;
        Ok(engine)
    }

    fn verify_caller(&mut self, snapshot: &HerdrNativeSnapshot) -> Result<NativeTuple> {
        let current = self.client.current_pane_from(snapshot.raw())?;
        let pane = snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| pane.pane.id == current.id)
            .ok_or_else(|| invalid("caller missing from native snapshot"))?;
        let caller = NativeTuple::from_pane(pane);
        if caller.workspace_id != current.workspace_id
            || caller.tab_id != current.tab_id
            || self.environment.get("HERDR_PANE_ID") != Some(&caller.pane_id)
            || self.caller.as_ref().is_some_and(|old| old != &caller)
            || self
                .required_caller_terminal
                .as_ref()
                .is_some_and(|terminal| terminal != &caller.terminal_id)
            || (self.launched.is_some()
                && (self
                    .environment
                    .get("HERDR_TAB_ID")
                    .is_some_and(|v| v != &caller.tab_id)
                    || self
                        .environment
                        .get("HERDR_WORKSPACE_ID")
                        .is_some_and(|v| v != &caller.workspace_id)))
        {
            return Err(super::RuntimeError::NativeContextChanged(
                "caller native tuple moved or changed".into(),
            ));
        }
        self.caller = Some(caller.clone());
        Ok(caller)
    }

    fn begin_observation(&mut self) {
        if !std::mem::take(&mut self.bootstrap) {
            self.observation = None;
        }
    }
    fn fresh_under_lock(&mut self) {
        self.bootstrap = false;
        self.observation = None;
    }
    fn snapshot(&mut self) -> Result<HerdrNativeSnapshot> {
        if self.observation.is_none() {
            self.observation = Some(HerdrNativeSnapshot::parse(
                &self.client.session_snapshot()?,
            )?);
        }
        Ok(self
            .observation
            .as_ref()
            .expect("observed snapshot")
            .clone())
    }
    fn read_attempt(&self, key: &str, purpose: NativePurpose) -> Result<Option<NativeAttempt>> {
        let record = self.state.read(key)?;
        if record
            .as_ref()
            .is_some_and(|record| record.purpose() != purpose)
        {
            return Err(self.refuse("native evidence has another purpose; fence retained"));
        }
        if record
            .as_ref()
            .is_some_and(|record| record.endpoint() != self.client.endpoint().socket)
        {
            return Err(
                self.refuse("native evidence belongs to another Herdr endpoint; fence retained")
            );
        }
        Ok(record)
    }
    fn check_cancelled(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(invalid("native operation cancelled"))
        } else {
            Ok(())
        }
    }
    fn refuse(&self, message: impl Into<String>) -> super::RuntimeError {
        FrontendError {
            message: message.into(),
            commands: self.commands.clone(),
        }
        .into()
    }
    fn call(
        &mut self,
        method: &str,
        params: Value,
        expected: Option<&str>,
        effect: bool,
    ) -> Result<Value> {
        self.check_cancelled()?;
        if effect {
            self.observation = None;
        }
        let (value, commands) = self
            .client
            .card_request(method, params, expected, effect)
            .map_err(|mut e| {
                let mut prior = self.commands.clone();
                prior.append(&mut e.commands);
                e.commands = prior;
                e
            })?;
        self.commands.extend(commands);
        Ok(value)
    }
    fn inspect(&mut self, inventory: bool) -> Result<Verified> {
        self.check_cancelled()?;
        let snapshot = self.snapshot()?;
        let caller = self.verify_caller(&snapshot)?;
        let before = self.source.read(&self.client, &snapshot, inventory)?;
        let owner = snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| pane.pane.id == before.card.owner().pane_id)
            .ok_or_else(|| self.refuse("attested owner missing from native snapshot"))?;
        let tuple = NativeTuple::from_pane(owner);
        let claimed = before.card.owner();
        if tuple.tab_id != claimed.tab_id
            || tuple.workspace_id != claimed.workspace_id
            || tuple.workspace_id != caller.workspace_id
            || tuple.tab_id != caller.tab_id
        {
            return Err(self.refuse("attested owner moved outside this card workspace/tab"));
        }
        let scope = before.card.scope();
        if scope.harness != "pi" {
            return Err(self.refuse("native cards require an attested Pi Session"));
        }
        let identity = conversation_key(scope)?;
        let key = identity.clone();
        let binding = BTreeMap::from([
            ("strm_catalog".into(), json_digest(&scope.catalog_source)?),
            ("strm_host".into(), scope.host_id.clone()),
            ("strm_stream".into(), scope.stream_id.clone()),
        ]);
        let tokens = snapshot
            .workspace_tokens(&caller.workspace_id)
            .expect("linked workspace");
        self.check_expected_owner(
            scope,
            &tuple,
            tokens.get("strm_binding").map(String::as_str),
        )?;
        if self.captured.as_ref().is_some_and(|old| {
            old.scope != *scope
                || old.tuple != tuple
                || old
                    .binding
                    .as_ref()
                    .is_some_and(|binding| tokens.get("strm_binding") != Some(binding))
        }) {
            return Err(super::RuntimeError::NativeContextChanged(
                "attested owner scope, tuple or binding lifetime changed".into(),
            ));
        }
        if tokens.keys().any(|key| key.starts_with("strm_"))
            && (tokens.get("strm_binding").is_none_or(String::is_empty)
                || binding
                    .iter()
                    .any(|(key, value)| tokens.get(key) != Some(value)))
        {
            return Err(self.refuse("conflicting native Stream workspace binding"));
        }
        self.captured = Some(CapturedOwner {
            scope: scope.clone(),
            tuple: tuple.clone(),
            binding: tokens.get("strm_binding").cloned(),
        });
        self.check_cancelled()?;
        Ok(Verified {
            saved: before,
            snapshot,
            caller,
            owner: tuple,
            identity,
            key,
            binding,
        })
    }
    fn guard(&mut self) -> Result<Verified> {
        let proof = self.inspect(false)?;
        self.validate_pair(proof)
    }
    fn validate_pair(&self, proof: Verified) -> Result<Verified> {
        let pair = proof.pair()?;
        let native = &proof.saved.card.scope().native_session_id;
        if let Some(context) = &self.launched
            && (proof.caller == proof.owner
                || context.scope() != proof.saved.card.scope()
                || context.owner() != proof.saved.card.owner()
                || context.owner_terminal() != proof.owner.terminal_id)
        {
            return Err(
                self.refuse("launched card scope or caller is not its protected native lifetime")
            );
        }
        match pair {
            PairState::Empty => {}
            PairState::CardOnly { card, .. } => {
                if proof
                    .snapshot
                    .pane_tokens(&card.pane.id)
                    .and_then(|t| t.get("strm_native"))
                    != Some(native)
                {
                    return Err(self.refuse("reserved card native Pi identity mismatch"));
                }
            }
            PairState::Absent { agent, .. }
            | PairState::Paired { agent, .. }
            | PairState::Separated { agent, .. } => {
                if NativeTuple::from_pane(agent) != proof.owner
                    || proof.owner_tokens().get("strm_native") != Some(native)
                {
                    return Err(self.refuse("native Pi identity or owner tuple mismatch"));
                }
            }
        }
        if proof.caller.terminal_id != proof.owner.terminal_id {
            let PairState::Paired { card, nonce, .. } = proof.pair()? else {
                return Err(self.refuse("caller is not the exact owned card"));
            };
            if card.terminal_id != proof.caller.terminal_id
                || self.launched.as_ref().is_none_or(|context| {
                    context.nonce() != nonce || context.owner_terminal() != proof.owner.terminal_id
                })
            {
                return Err(self.refuse("caller card launch nonce or owner changed"));
            }
        }
        Ok(proof)
    }
}

fn tuple_from_reply(value: &Value) -> Result<NativeTuple> {
    let field = |key: &str| -> Result<String> {
        let value = value
            .get(key)
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("missing native {key}")))?;
        super::text(value)?;
        Ok(value.into())
    };
    Ok(NativeTuple {
        pane_id: field("pane_id")?,
        terminal_id: field("terminal_id")?,
        workspace_id: field("workspace_id")?,
        tab_id: field("tab_id")?,
    })
}
