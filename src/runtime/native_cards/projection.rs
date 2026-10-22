use super::super::native_names::{
    NameDecision, NameInput, NameIntent, NameOwner, NameStore, SurfaceKind, decide_name,
};
use super::actions::MetadataTarget;
use super::config::validate_title;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameProjectionIntent {
    Automatic,
    Explicit,
    Workspace,
    Tab,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NameOutcome {
    Disabled,
    NotFirst,
    Unchanged,
    Manual,
    Renamed,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NameReport {
    pub workspace: NameOutcome,
    pub tab: NameOutcome,
}
#[derive(PartialEq, Eq)]
struct Label {
    id: String,
    label: String,
    number: u64,
}

impl NativeCardEngine {
    fn projection_observation(&mut self) -> Result<Verified> {
        let v = self.guard()?;
        match v.pair()? {
            PairState::Empty => {
                if v.caller != v.owner {
                    return Err(self.refuse("only the owning Pi may initialize projections"));
                }
                self.evidence(&v)?;
                if v.workspace_tokens().contains_key("strm_binding") {
                    v.bound()?;
                }
            }
            PairState::Separated { .. } => {
                return Err(self.refuse("owned card moved; projections suspended"));
            }
            _ => {
                v.bound()?;
                self.evidence(&v)?;
            }
        }
        Ok(v)
    }
    pub(super) fn projection_proof(&mut self) -> Result<Verified> {
        let mut v = self.projection_observation()?;
        if matches!(v.pair()?, PairState::Empty)
            && !v.workspace_tokens().contains_key("strm_binding")
        {
            let mut values: BTreeMap<_, _> = v
                .binding
                .iter()
                .map(|(k, v)| (k.clone(), Some(v.clone())))
                .collect();
            values.insert(
                "strm_binding".into(),
                Some(uuid::Uuid::new_v4().to_string()),
            );
            let target = MetadataTarget::workspace(&v, &v.caller.workspace_id)?;
            self.stamp(target, values, |fresh| {
                if fresh.caller != fresh.owner || !matches!(fresh.pair()?, PairState::Empty) {
                    return Err(invalid("pair changed while establishing native binding"));
                }
                Ok(())
            })?;
            v = self.guard()?;
        }
        v.bound()?;
        self.evidence(&v)?;
        Ok(v)
    }
    pub fn project_summaries(&mut self) -> Result<()> {
        let initial = self.guard_recovering()?;
        if initial.caller != initial.owner {
            return Err(self.refuse("only owning Pi may publish Session progress"));
        }
        let state = self.state.clone();
        let _lock = state.lock(
            &self.client.endpoint().socket,
            &self.caller.workspace_id,
            self.config.timeout_seconds,
            &self.cancelled,
        )?;
        for workspace in [true, false] {
            let v = self.projection_proof()?;
            let desired = if workspace {
                BTreeMap::from([(
                    "strm_goal_short".into(),
                    compact(v.saved.card.stream().goal_short.as_deref()),
                )])
            } else {
                BTreeMap::from([
                    (
                        "strm_status".into(),
                        Some(v.saved.card.session().status.to_string()),
                    ),
                    (
                        "strm_state_short".into(),
                        compact(v.saved.card.session().state_short.as_deref()),
                    ),
                ])
            };
            let current = if workspace {
                v.workspace_tokens()
            } else {
                v.owner_tokens()
            };
            let changed = desired
                .into_iter()
                .filter(|(key, value)| current.get(key) != value.as_ref())
                .collect::<BTreeMap<_, _>>();
            if changed.is_empty() {
                continue;
            }
            let target = if workspace {
                MetadataTarget::workspace(&v, &v.caller.workspace_id)?
            } else {
                MetadataTarget::pane(&v, &v.owner.pane_id)?
            };
            self.stamp(target, changed, |fresh| {
                if fresh.caller != fresh.owner {
                    return Err(invalid("progress owner changed"));
                }
                fresh.bound().map(|_| ())
            })?;
        }
        Ok(())
    }
    pub fn project_caption(&mut self) -> Result<()> {
        let state = self.state.clone();
        let _lock = state.lock(
            &self.client.endpoint().socket,
            &self.caller.workspace_id,
            self.config.timeout_seconds,
            &self.cancelled,
        )?;
        let before = self.projection_observation()?;
        let PairState::Paired { card, .. } = before.pair()? else {
            return Ok(());
        };
        let tuple = NativeTuple::from_pane(card);
        let desired = before.saved.card.stream().title.clone();
        validate_title(&desired)?;
        let verify = |fresh: &Verified| -> Result<()> {
            fresh.bound()?;
            let PairState::Paired { card, nonce, .. } = fresh.pair()? else {
                return Err(invalid("caption display is no longer paired"));
            };
            let record = state
                .read(&fresh.key)?
                .ok_or_else(|| invalid("caption lacks ready evidence"))?;
            if fresh.saved.card.stream().title != desired
                || NativeTuple::from_pane(card) != tuple
                || record.nonce() != nonce
                || !matches!(record.phase(), NativePhase::Ready(_))
                || record
                    .resources()
                    .is_none_or(|resources| !tuple.matches_resources(resources))
            {
                return Err(invalid("caption ready lifetime changed"));
            }
            Ok(())
        };
        let tokens = before
            .snapshot
            .pane_tokens(&tuple.pane_id)
            .expect("paired tokens");
        if tokens.get("strm_caption_manual").map(String::as_str) == Some("1") {
            return Ok(());
        }
        let expected = tokens
            .get("strm_caption")
            .map(String::as_str)
            .unwrap_or("Streams work card");
        if before.snapshot.pane_label(&tuple.pane_id) != Some(expected) {
            let target = MetadataTarget::pane(&before, &tuple.pane_id)?;
            self.stamp(
                target,
                BTreeMap::from([("strm_caption_manual".into(), Some("1".into()))]),
                &verify,
            )?;
            return Ok(());
        }
        if tokens.get("strm_caption").map(String::as_str) == Some(desired.as_str()) {
            return Ok(());
        }
        if expected == desired {
            let target = MetadataTarget::pane(&before, &tuple.pane_id)?;
            self.stamp(
                target,
                BTreeMap::from([("strm_caption".into(), Some(desired.clone()))]),
                &verify,
            )?;
            return Ok(());
        }
        let renamed = self.call(
            &argv(&["pane", "rename", &tuple.pane_id, &desired]),
            Some("pane_info"),
            true,
        )?;
        if tuple_from_reply(&renamed["pane"])? != tuple || renamed["pane"]["label"] != desired {
            return Err(self.refuse("caption tuple acknowledgement mismatch"));
        }
        let after = self.marked()?;
        if after.saved.card.stream().title != desired
            || after.snapshot.pane_label(&tuple.pane_id) != Some(desired.as_str())
            || !matches!(after.pair()?,PairState::Paired {card,..} if NativeTuple::from_pane(card)==tuple)
        {
            return Err(self.refuse("caption source changed; effect may be partial"));
        }
        let target = MetadataTarget::pane(&after, &tuple.pane_id)?;
        self.stamp(
            target,
            BTreeMap::from([("strm_caption".into(), Some(desired.clone()))]),
            &verify,
        )?;
        Ok(())
    }

    pub fn project_names(&mut self, intent: NameProjectionIntent) -> Result<NameReport> {
        self.guard_recovering()?;
        let state = self.state.clone();
        let lock = state.lock(
            &self.client.endpoint().socket,
            &self.caller.workspace_id,
            self.config.timeout_seconds,
            &self.cancelled,
        )?;
        let mut report = NameReport {
            workspace: NameOutcome::Disabled,
            tab: NameOutcome::Disabled,
        };
        for kind in [SurfaceKind::Workspace, SurfaceKind::Tab] {
            let explicit = matches!(intent, NameProjectionIntent::Explicit)
                || matches!(
                    (intent, kind),
                    (NameProjectionIntent::Workspace, SurfaceKind::Workspace)
                        | (NameProjectionIntent::Tab, SurfaceKind::Tab)
                );
            if !explicit
                && !match kind {
                    SurfaceKind::Workspace => self.config.workspace_naming,
                    SurfaceKind::Tab => self.config.tab_naming,
                }
            {
                continue;
            }
            let before = self.projection_proof()?;
            if before.caller != before.owner {
                return Err(self.refuse("only owning Pi may name agent surfaces"));
            }
            if kind == SurfaceKind::Tab && !before.snapshot.first_pane(before.owner_pane()) {
                report.tab = NameOutcome::NotFirst;
                continue;
            }
            let label = surface(&before, kind)?;
            let desired = desired_name(&before, kind);
            validate_title(&desired)?;
            let scope = NameStore::prepare_scope(
                &lock,
                path_text(&self.client.endpoint().socket)?,
                &self.caller.workspace_id,
                before.bound()?,
                kind,
                &label.id,
            )?;
            let prior = NameStore::read(&lock, &scope)?;
            let nonce = match before.pair()? {
                PairState::Empty => before.saved.card.scope().native_session_id.as_str(),
                PairState::Absent { nonce, .. } | PairState::Paired { nonce, .. } => nonce,
                _ => return Err(self.refuse("naming requires a verified owner")),
            };
            let owner = NameOwner::new(&before.owner.terminal_id, nonce)?;
            let fresh = self.projection_proof()?;
            if desired_name(&fresh, kind) != desired
                || surface(&fresh, kind)? != label
                || fresh.owner != before.owner
                || fresh.pair()? != before.pair()?
                || fresh.bound()? != before.bound()?
            {
                return Err(self.refuse("naming source, owner or label changed"));
            }
            let default_label_proven = kind == SurfaceKind::Tab
                && before
                    .snapshot
                    .topology()
                    .tabs
                    .iter()
                    .filter(|tab| tab.workspace_id == before.owner.workspace_id)
                    .position(|tab| tab.id == label.id)
                    .is_some_and(|index| label.label == (index + 1).to_string());
            let decision = decide_name(NameInput {
                scope: &scope,
                provenance: prior.as_ref().map(|p| &p.record),
                owner: &owner,
                current_label: &label.label,
                desired_name: &desired,
                default_label_proven,
                intent: if explicit {
                    NameIntent::Explicit
                } else {
                    NameIntent::Automatic
                },
            })?;
            let revision = prior.as_ref().map(|p| p.revision.as_str());
            let outcome = match decision {
                NameDecision::Noop => NameOutcome::Unchanged,
                NameDecision::Adopt(provenance) | NameDecision::Manual(provenance) => {
                    let manual = matches!(
                        provenance,
                        super::super::native_names::NameProvenance::Manual { .. }
                    );
                    NameStore::write(&lock, &scope, &provenance, revision, &self.cancelled)?;
                    if manual {
                        NameOutcome::Manual
                    } else {
                        NameOutcome::Unchanged
                    }
                }
                NameDecision::Rename {
                    name,
                    after_success,
                } => {
                    let (native_kind, id_field, result_kind) = match kind {
                        SurfaceKind::Workspace => ("workspace", "workspace_id", "workspace_info"),
                        SurfaceKind::Tab => ("tab", "tab_id", "tab_info"),
                    };
                    let result = self.call(
                        &argv(&[native_kind, "rename", &label.id, &name]),
                        Some(result_kind),
                        true,
                    )?;
                    let ack = &result[native_kind];
                    let after = self.projection_proof()?;
                    let actual = surface(&after, kind)?;
                    if ack[id_field] != label.id
                        || ack["workspace_id"] != before.owner.workspace_id
                        || ack["number"].as_u64() != Some(label.number)
                        || ack["label"] != name
                        || actual.id != label.id
                        || actual.number != label.number
                        || actual.label != name
                        || desired_name(&after, kind) != desired
                        || after.owner != before.owner
                        || after.bound()? != scope.binding_nonce()
                        || (kind == SurfaceKind::Tab
                            && !after.snapshot.first_pane(after.owner_pane()))
                    {
                        return Err(self.refuse(
                            "native name acknowledgement changed; provenance not published",
                        ));
                    }
                    NameStore::write(&lock, &scope, &after_success, revision, &self.cancelled)?;
                    NameOutcome::Renamed
                }
            };
            match kind {
                SurfaceKind::Workspace => report.workspace = outcome,
                SurfaceKind::Tab => report.tab = outcome,
            }
        }
        Ok(report)
    }
}

fn compact(value: Option<&str>) -> Option<String> {
    value
        .filter(|s| {
            !s.is_empty()
                && s.chars().count() <= 80
                && s.trim() == *s
                && !s.chars().any(char::is_control)
        })
        .map(str::to_owned)
}
fn desired_name(v: &Verified, kind: SurfaceKind) -> String {
    match kind {
        SurfaceKind::Workspace => v.saved.card.stream().title.clone(),
        SurfaceKind::Tab => v.saved.card.session().title.clone(),
    }
}
fn surface(v: &Verified, kind: SurfaceKind) -> Result<Label> {
    match kind {
        SurfaceKind::Workspace => {
            let workspace = v
                .snapshot
                .topology()
                .workspaces
                .iter()
                .find(|w| w.id == v.owner.workspace_id)
                .expect("linked workspace");
            Ok(Label {
                id: workspace.id.clone(),
                label: workspace.label.clone(),
                number: v
                    .snapshot
                    .workspace_number(&workspace.id)
                    .expect("validated number"),
            })
        }
        SurfaceKind::Tab => {
            let tab = v
                .snapshot
                .topology()
                .tabs
                .iter()
                .find(|tab| tab.id == v.owner.tab_id)
                .expect("linked tab");
            Ok(Label {
                id: tab.id.clone(),
                label: tab.label.clone().expect("complete tab label"),
                number: v.snapshot.tab_number(&tab.id).expect("validated number"),
            })
        }
    }
}
fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| invalid("native path must be UTF-8"))
}
