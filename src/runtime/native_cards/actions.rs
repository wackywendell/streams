//! Native display effects. A failed acknowledgement never authorizes another launch.

use super::*;
use crate::frontend::herdr::HerdrWorkspace;
use crate::runtime::open_protocol::{Action, Protocol};
use serde_json::json;

/// Capture metadata separately from topology so an unrelated pane cannot become a write target.
#[derive(Clone, PartialEq, Eq)]
pub(super) enum MetadataTarget {
    Pane {
        tuple: NativeTuple,
        tokens: Tokens,
        label: Option<String>,
    },
    Workspace {
        record: HerdrWorkspace,
        tokens: Tokens,
        number: u64,
    },
}
impl MetadataTarget {
    pub(super) fn pane(v: &Verified, id: &str) -> Result<Self> {
        let pane = v
            .snapshot
            .topology()
            .panes
            .iter()
            .find(|p| p.pane.id == id)
            .ok_or_else(|| invalid("native metadata pane missing"))?;
        Ok(Self::Pane {
            tuple: NativeTuple::from_pane(pane),
            tokens: v
                .snapshot
                .pane_tokens(id)
                .ok_or_else(|| invalid("pane tokens missing"))?
                .clone(),
            label: v.snapshot.pane_label(id).map(str::to_owned),
        })
    }
    pub(super) fn workspace(v: &Verified, id: &str) -> Result<Self> {
        let record = v
            .snapshot
            .topology()
            .workspaces
            .iter()
            .find(|w| w.id == id)
            .ok_or_else(|| invalid("native metadata workspace missing"))?
            .clone();
        Ok(Self::Workspace {
            record,
            tokens: v
                .snapshot
                .workspace_tokens(id)
                .ok_or_else(|| invalid("workspace tokens missing"))?
                .clone(),
            number: v
                .snapshot
                .workspace_number(id)
                .ok_or_else(|| invalid("workspace number missing"))?,
        })
    }
    fn kind_id(&self) -> (&str, &str) {
        match self {
            Self::Pane { tuple, .. } => ("pane", &tuple.pane_id),
            Self::Workspace { record, .. } => ("workspace", &record.id),
        }
    }
    fn capture(&self, v: &Verified) -> Result<Self> {
        let (kind, id) = self.kind_id();
        if kind == "pane" {
            Self::pane(v, id)
        } else {
            Self::workspace(v, id)
        }
    }
    fn tokens(&self) -> &Tokens {
        match self {
            Self::Pane { tokens, .. } | Self::Workspace { tokens, .. } => tokens,
        }
    }
    fn same_lifetime(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Pane {
                    tuple: a,
                    label: al,
                    ..
                },
                Self::Pane {
                    tuple: b,
                    label: bl,
                    ..
                },
            ) => a == b && al == bl,
            (
                Self::Workspace {
                    record: a,
                    number: an,
                    ..
                },
                Self::Workspace {
                    record: b,
                    number: bn,
                    ..
                },
            ) => a == b && an == bn,
            _ => false,
        }
    }
}

pub(super) fn recorded_live(v: &Verified, attempt: &NativeAttempt) -> bool {
    attempt.resources().is_some_and(|r| {
        v.snapshot.topology().panes.iter().any(|p| {
            p.terminal_id == r.terminal_id()
                || matches!(attempt.phase(), NativePhase::Reserved(_)) && p.pane.id == r.pane_id()
        })
    })
}
fn paired_tuple(v: &Verified) -> Result<NativeTuple> {
    match v.pair()? {
        PairState::Paired { card, .. } => Ok(NativeTuple::from_pane(card)),
        _ => Err(invalid("Streams card is absent or not paired")),
    }
}
fn path_text(path: &Path) -> Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| invalid("native path must be UTF-8"))
}

impl NativeCardEngine {
    pub(super) fn evidence(&self, v: &Verified) -> Result<Option<NativeAttempt>> {
        self.validate_evidence(v, self.read_attempt(&v.key, NativePurpose::Card)?)
    }

    pub(super) fn validate_evidence(
        &self,
        v: &Verified,
        attempt: Option<NativeAttempt>,
    ) -> Result<Option<NativeAttempt>> {
        let pair = v.pair()?;
        if matches!(pair, PairState::CardOnly { .. }) {
            return Err(self.refuse(
                "uncertain card-only launch retained; owner must complete its exact reservation",
            ));
        }
        if let Some(record) = &attempt {
            if matches!(record.phase(), NativePhase::Unlaunched) {
                return Err(self.refuse("uncertain native launch retained; no retry permitted"));
            }
            if matches!(record.phase(), NativePhase::Reserved(_)) {
                let r = record.resources().expect("reserved resources");
                let mut derived = record.clone();
                match pair {
                    PairState::Absent { nonce, .. }
                        if nonce == record.nonce()
                            && v.owner_tokens().get("strm_card").map(String::as_str)
                                == Some(r.terminal_id())
                            && !recorded_live(v, record) =>
                    {
                        derived.mark_closed()?
                    }
                    PairState::Paired { card, nonce, .. }
                        if nonce == record.nonce()
                            && NativeTuple::from_pane(card).matches_resources(r) =>
                    {
                        derived.mark_ready()?
                    }
                    _ => {
                        return Err(self.refuse(
                            "uncertain reserved launch retained; exact pair proof required",
                        ));
                    }
                }
                return Ok(Some(derived));
            }
            if matches!(pair, PairState::Empty) && recorded_live(v, record) {
                return Err(self.refuse("recorded card is live without verified ownership; no duplicate or cleanup permitted"));
            }
        }
        let matches = match pair {
            PairState::Empty => true,
            PairState::Absent { nonce, .. } => attempt.as_ref().is_some_and(|a| {
                a.nonce() == nonce
                    && a.resources().is_some_and(|r| {
                        v.owner_tokens().get("strm_card").map(String::as_str)
                            == Some(r.terminal_id())
                    })
            }),
            PairState::Paired { card, nonce, .. } | PairState::Separated { card, nonce, .. } => {
                attempt.as_ref().is_some_and(|a| {
                    a.nonce() == nonce
                        && a.resources().is_some_and(|r| {
                            card.terminal_id == r.terminal_id()
                                && v.owner_tokens().get("strm_card").map(String::as_str)
                                    == Some(r.terminal_id())
                        })
                })
            }
            PairState::CardOnly { .. } => false,
        };
        if !matches {
            return Err(self.refuse("pair does not match durable native launch evidence"));
        }
        Ok(attempt)
    }

    pub(super) fn marked(&mut self) -> Result<Verified> {
        let v = self.guard()?;
        v.bound()?;
        self.evidence(&v)?;
        if !matches!(
            v.pair()?,
            PairState::Paired { .. } | PairState::Absent { .. }
        ) {
            return Err(self.refuse("verified paired or absent owner markers required"));
        }
        Ok(v)
    }

    pub(super) fn stamp(
        &mut self,
        target: MetadataTarget,
        values: BTreeMap<String, Option<String>>,
        mut verify: impl FnMut(&Verified) -> Result<()>,
    ) -> Result<Verified> {
        let before = self.guard()?;
        verify(&before)?;
        if target.capture(&before)? != target {
            return Err(self.refuse("native metadata target changed"));
        }
        let (kind, id) = target.kind_id();
        // A null token value clears that token.
        let params = json!({format!("{kind}_id"):id,"source":PLUGIN_ID,"tokens":values});
        self.call(&format!("{kind}.report_metadata"), params, None, true)?;
        let after = self.guard()?;
        verify(&after)?;
        let live = target.capture(&after)?;
        if !target.same_lifetime(&live)
            || values
                .iter()
                .any(|(key, value)| live.tokens().get(key) != value.as_ref())
        {
            return Err(
                self.refuse("metadata lifetime or token echo changed; effect may be partial")
            );
        }
        Ok(after)
    }

    pub(super) fn ensure_plugin(
        &mut self,
        mut verify: impl FnMut(&mut Self) -> Result<()>,
    ) -> Result<bool> {
        let root = std::fs::canonicalize(&self.config.plugin_root)?;
        let manifest = root.join("streams-rust-herdr-plugin.toml");
        let root = path_text(&root)?;
        let manifest = path_text(&manifest)?;
        let check = |p: &Value| -> Result<()> {
            let panes = json!([
                {"id":"card","title":"Streams work card","placement":"split","command":["/bin/sh","bin/open-rust-streams-pane.sh"]},
                {"id":"document","title":"Document","placement":"tab","command":["/bin/sh","bin/open-document.sh"]},
            ]);
            if p["plugin_id"] != PLUGIN_ID
                || p["enabled"] != true
                || p["plugin_root"] != root
                || p["manifest_path"] != manifest
                || p["source"]["kind"] != "local"
                || p["version"] != "0.1.0"
                || p["min_herdr_version"] != "0.9.3"
                || p["platforms"] != json!(["macos", "linux"])
                || p["panes"] != panes
            {
                return Err(invalid(
                    "conflicting Streams Rust plugin; no registration was replaced",
                ));
            }
            Ok(())
        };
        let listed = self.call(
            "plugin.list",
            json!({"plugin_id":PLUGIN_ID}),
            Some("plugin_list"),
            false,
        )?;
        let plugins = listed["plugins"]
            .as_array()
            .filter(|p| p.len() <= 1)
            .ok_or_else(|| self.refuse("unexpected Streams Rust plugin registrations"))?;
        if let Some(plugin) = plugins.first() {
            check(plugin)?;
            return Ok(false);
        }
        self.guard()?;
        verify(self)?;
        let linked = self.call(
            "plugin.link",
            json!({"path":manifest}),
            Some("plugin_linked"),
            true,
        )?;
        self.guard()?;
        verify(self)?;
        check(&linked["plugin"])?;
        Ok(true)
    }

    fn launch_proof(&mut self, expected: &Option<NativeAttempt>) -> Result<Verified> {
        let v = self.guard()?;
        if v.caller != v.owner
            || !matches!(v.pair()?, PairState::Empty | PairState::Absent { .. })
            || self.evidence(&v)? != *expected
            || expected.as_ref().is_some_and(|a| recorded_live(&v, a))
        {
            return Err(self.refuse("launch proof changed or recorded card is still live"));
        }
        Ok(v)
    }

    fn pending_launch(
        state: &NativeState,
        v: &Verified,
        pending: &NativeAttempt,
        old: Option<&NativeAttempt>,
    ) -> Result<()> {
        v.bound()?;
        if v.caller != v.owner
            || v.key != pending.key()
            || state.read(&v.key)?.as_ref() != Some(pending)
            || old.is_some_and(|a| recorded_live(v, a))
        {
            return Err(invalid("pending launch authority changed"));
        }
        match v.pair()? {
            PairState::Empty => Ok(()),
            PairState::Absent { nonce, .. }
                if old.is_some_and(|a| {
                    a.nonce() == nonce
                        && a.resources().is_some_and(|r| {
                            v.owner_tokens().get("strm_card").map(String::as_str)
                                == Some(r.terminal_id())
                        })
                }) =>
            {
                Ok(())
            }
            _ => Err(invalid(
                "pair changed before native dispatch; pending evidence retained",
            )),
        }
    }

    pub fn open_display(&mut self, mode: OpenMode) -> Result<DisplayOutcome> {
        self.begin_observation();
        let observed = self.guard_recovering()?;
        if observed.caller != observed.owner {
            return Err(self.refuse("only the attested Pi owner may open a display"));
        }
        let state = Arc::clone(&self.state);
        let lock = state.lock(self.config.timeout_seconds, &self.cancelled)?;
        self.fresh_under_lock();
        let mut v = self.guard()?;
        let raw = self.read_attempt(&v.key, NativePurpose::Card)?;
        if let Some(attempt) = raw
            .as_ref()
            .filter(|a| matches!(a.phase(), NativePhase::Reserved(_)))
            && matches!(v.pair()?, PairState::Empty | PairState::CardOnly { .. })
        {
            let resources = self.link_card(&lock, &v, attempt)?;
            return Ok(DisplayOutcome::Existing(resources));
        }
        let attempt = self.evidence(&v)?;
        if let Some(derived) = attempt.as_ref().filter(|a| Some(*a) != raw.as_ref()) {
            lock.write(raw.as_ref(), derived)?;
        }
        if matches!(v.pair()?, PairState::Paired { .. }) {
            v.bound()?;
            return Ok(DisplayOutcome::Existing(paired_tuple(&v)?.resources()?));
        }
        if matches!(v.pair()?, PairState::Separated { .. }) {
            return Err(self.refuse("owned card moved; no resource closed or replaced"));
        }
        if mode == OpenMode::Automatic && !self.config.startup_automatic() {
            return Ok(DisplayOutcome::Suppressed);
        }
        self.launch_proof(&attempt)?;
        self.ensure_plugin(|engine| engine.launch_proof(&attempt).map(|_| ()))?;
        self.projection_proof()?;
        v = self.launch_proof(&attempt)?;
        v.bound()?;
        let root = std::fs::canonicalize(&self.config.plugin_root)?;
        let launch = self.source.launch_context();
        if !launch.cwd.is_absolute() {
            return Err(self.refuse("card read cwd must be absolute"));
        }
        let binary = std::env::current_exe()?;
        if !binary.is_absolute() {
            return Err(self.refuse("Rust card executable must be absolute"));
        }
        let bounded: BTreeMap<String, String> = launch
            .environment
            .into_iter()
            .filter(|(k, _)| SOURCE_ENV_KEYS.contains(&k.as_str()))
            .collect();
        let mut vars = bounded.clone();
        let nonce = uuid::Uuid::new_v4().to_string();
        vars.extend(BTreeMap::from([
            ("STREAMS_CARDS_SCOPE".into(), serde_json::to_string(&ScopeWire::from(v.saved.card.scope())).map_err(|e| invalid(e.to_string()))?),
            ("STREAMS_CARDS_OWNER".into(), json!({"pane_id":v.owner.pane_id,"workspace_id":v.owner.workspace_id,"tab_id":v.owner.tab_id}).to_string()),
            ("STREAMS_CARDS_OWNER_TERMINAL".into(), v.owner.terminal_id.clone()),
            ("STREAMS_CARDS_NONCE".into(), nonce.clone()),
            ("STREAMS_CARDS_READ_ENV".into(), serde_json::to_string(&bounded).map_err(|e| invalid(e.to_string()))?),
            ("STREAMS_CARDS_READ_CWD".into(), path_text(&launch.cwd)?),
            ("WORK_CARDS_CONFIG_PATH".into(), path_text(&self.config.source_path)?),
            ("STREAMS_RUST_BIN".into(), path_text(&binary)?),
            ("HERDR_SOCKET_PATH".into(), path_text(&self.client.endpoint().socket)?),
            ("STREAMS_CARDS_STATE_DIR".into(), path_text(state.directory())?),
            ("STREAMS_CARDS_PLUGIN_ROOT".into(), path_text(&root)?),
        ]));
        let pending = NativeAttempt::new(
            &v.key,
            nonce,
            &self.client.endpoint().socket,
            NativePurpose::Card,
        )?;
        lock.write(attempt.as_ref(), &pending)?;
        // Pending is deliberately retained on every later error, including a
        // cancellation before dispatch. Conservative fencing beats an inferred retry.
        let protocol = Protocol::new()?;
        // Even a partially acknowledged marker clear retains the new pending gate.
        if matches!(v.pair()?, PairState::Absent { .. }) {
            let target = MetadataTarget::pane(&v, &v.owner.pane_id)?;
            self.stamp(
                target,
                PAIR_KEYS.iter().map(|k| ((*k).into(), None)).collect(),
                |fresh| Self::pending_launch(&state, fresh, &pending, attempt.as_ref()),
            )?;
        }
        v = self.guard()?;
        Self::pending_launch(&state, &v, &pending, attempt.as_ref())?;
        if !matches!(v.pair()?, PairState::Empty) {
            return Err(self.refuse("owner markers remain before launch"));
        }
        let params = json!({"plugin_id":PLUGIN_ID,"entrypoint":"card","placement":"split",
            "target_pane_id":v.owner.pane_id,"direction":"down","cwd":path_text(&root)?,
            "focus":false,"env":vars});
        let protocol = protocol.next(Action::Dispatch)?;
        let opened = self.call("plugin.pane.open", params, Some("plugin_pane_opened"), true)?;
        let info = &opened["plugin_pane"];
        let returned = tuple_from_reply(&info["pane"])?;
        if info["plugin_id"] != PLUGIN_ID
            || info["entrypoint"] != "card"
            || returned.workspace_id != v.owner.workspace_id
            || returned.tab_id != v.owner.tab_id
            || v.snapshot
                .topology()
                .panes
                .iter()
                .any(|p| p.pane.id == returned.pane_id || p.terminal_id == returned.terminal_id)
        {
            return Err(self
                .refuse("unexpected or replayed native card response; pending evidence retained"));
        }
        let protocol = protocol.next(Action::Acknowledge)?;
        let mut reserved = pending.clone();
        reserved.reserve(returned.resources()?)?;
        lock.write(Some(&pending), &reserved)?;
        let after = self.guard()?;
        let live = after.snapshot.locate_terminal(&returned.terminal_id)?;
        if NativeTuple::from_pane(live) != returned
            || PAIR_KEYS.iter().any(|k| {
                after
                    .snapshot
                    .pane_tokens(&returned.pane_id)
                    .is_some_and(|t| t.contains_key(*k))
            })
        {
            return Err(self.refuse("new card tuple or markers changed; pending evidence retained"));
        }
        let resources = self.link_card(&lock, &after, &reserved)?;
        protocol.next(Action::Bind)?.next(Action::Complete)?;
        self.resize_ready(&returned)?;
        Ok(DisplayOutcome::Opened(resources))
    }

    fn resize_ready(&mut self, card: &NativeTuple) -> Result<()> {
        let fraction = self.config.pane_height_fraction;
        if fraction == 0.5 {
            return Ok(());
        }
        if !fraction.is_finite() || fraction <= 0.0 || fraction >= 1.0 {
            return Err(self.refuse("invalid bounded card height fraction"));
        }
        let v = self.marked()?;
        if paired_tuple(&v)? != *card {
            return Err(self.refuse("card changed before sizing"));
        }
        let target = if fraction < 0.5 { &v.owner } else { card };
        let resized = self.call(
            "pane.resize",
            json!({"pane_id":target.pane_id,"direction":if fraction < 0.5 { "down" } else { "up" },
                "amount":(0.5 - fraction).abs()}),
            Some("pane_resize"),
            true,
        )?;
        let after = self.marked()?;
        if resized["resize"]["pane_id"] != target.pane_id
            || paired_tuple(&after)? != *card
            || after.owner != v.owner
            || after.bound()? != v.bound()?
        {
            return Err(self.refuse(
                "sizing acknowledgement changed; ready display retained and sizing will not retry",
            ));
        }
        Ok(())
    }

    pub fn verify_display(&mut self) -> Result<CardSnapshot> {
        self.begin_observation();
        let v = self.marked()?;
        paired_tuple(&v)?;
        Ok(v.saved.card)
    }

    pub fn focus_display(&mut self) -> Result<()> {
        self.begin_observation();
        self.guard()?;
        let state = Arc::clone(&self.state);
        let _lock = state.lock(self.config.timeout_seconds, &self.cancelled)?;
        self.fresh_under_lock();
        let v = self.marked()?;
        let card = paired_tuple(&v)?;
        let focused = self.call(
            "plugin.pane.focus",
            json!({"pane_id":card.pane_id}),
            Some("plugin_pane_focused"),
            true,
        )?;
        let info = &focused["plugin_pane"];
        let after = self.marked()?;
        if info["plugin_id"] != PLUGIN_ID
            || info["entrypoint"] != "card"
            || tuple_from_reply(&info["pane"])? != card
            || paired_tuple(&after)? != card
            || after.owner != v.owner
            || after.bound()? != v.bound()?
            || after.pair()? != v.pair()?
        {
            return Err(self.refuse("focus tuple acknowledgement changed; effect may be partial"));
        }
        Ok(())
    }

    pub fn close_display(&mut self) -> Result<()> {
        self.begin_observation();
        self.guard()?;
        let state = Arc::clone(&self.state);
        let lock = state.lock(self.config.timeout_seconds, &self.cancelled)?;
        self.fresh_under_lock();
        let v = self.marked()?;
        if matches!(v.pair()?, PairState::Absent { .. }) {
            return Ok(());
        }
        let card = paired_tuple(&v)?;
        let owner_target = MetadataTarget::pane(&v, &v.owner.pane_id)?;
        let card_target = MetadataTarget::pane(&v, &card.pane_id)?;
        let raw = lock
            .read(&v.key)?
            .ok_or_else(|| self.refuse("missing close evidence"))?;
        let mut closed = raw.clone();
        if !matches!(closed.phase(), NativePhase::Closed(_)) {
            closed.mark_closed()?;
        }
        lock.write(Some(&raw), &closed)?;
        let fresh = self.marked()?;
        if owner_target.capture(&fresh)? != owner_target
            || card_target.capture(&fresh)? != card_target
            || fresh.bound()? != v.bound()?
            || fresh.pair()? != v.pair()?
        {
            return Err(self.refuse("pair changed before close; nothing closed"));
        }
        let reply = self.call(
            "plugin.pane.close",
            json!({"pane_id":card.pane_id}),
            Some("plugin_pane_closed"),
            true,
        )?;
        // A card caller has just disappeared: guard would reject it, so bracket the snapshot directly.
        let snapshot = self.snapshot()?;
        let saved = self.source.read(&self.client, &snapshot, false)?;
        let latest = self.source.read(&self.client, &snapshot, false)?;
        let owner = snapshot.locate_terminal(&v.owner.terminal_id)?;
        self.check_cancelled()?;
        if reply["pane_id"] != card.pane_id
            || saved.card.scope() != v.saved.card.scope()
            || saved.card.owner() != v.saved.card.owner()
            || latest.card.scope() != saved.card.scope()
            || latest.card.owner() != saved.card.owner()
            || NativeTuple::from_pane(owner) != v.owner
            || snapshot.pane_tokens(&v.owner.pane_id) != Some(v.owner_tokens())
            || snapshot.pane_label(&v.owner.pane_id) != v.snapshot.pane_label(&v.owner.pane_id)
            || snapshot
                .topology()
                .panes
                .iter()
                .any(|p| p.pane.id == card.pane_id || p.terminal_id == card.terminal_id)
            || snapshot
                .workspace_tokens(&v.owner.workspace_id)
                .and_then(|t| t.get("strm_binding"))
                .map(String::as_str)
                != Some(v.bound()?)
            || v.binding.iter().any(|(key, value)| {
                snapshot
                    .workspace_tokens(&v.owner.workspace_id)
                    .and_then(|t| t.get(key))
                    != Some(value)
            })
            || lock.read(&v.key)?.as_ref() != Some(&closed)
        {
            return Err(self.refuse(
                "close acknowledgement or owner changed; closed intent and markers retained",
            ));
        }
        Ok(())
    }
}
