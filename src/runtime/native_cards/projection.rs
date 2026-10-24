use super::actions::MetadataTarget;
use super::config::validate_title;
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SurfaceKind {
    Workspace,
    Tab,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum NameOutcome {
    Disabled,
    NotFirst,
    Unchanged,
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
        self.begin_observation();
        let initial = self.guard_recovering()?;
        if initial.caller != initial.owner {
            return Err(self.refuse("only owning Pi may publish Session progress"));
        }
        let state = self.state.clone();
        let _lock = state.lock(self.config.timeout_seconds, &self.cancelled)?;
        self.fresh_under_lock();
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
        self.begin_observation();
        let state = self.state.clone();
        let _lock = state.lock(self.config.timeout_seconds, &self.cancelled)?;
        self.fresh_under_lock();
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
        if before.snapshot.pane_label(&tuple.pane_id) == Some(desired.as_str()) {
            return Ok(());
        }
        verify(&before)?;
        let renamed = self.call(
            "pane.rename",
            serde_json::json!({"pane_id":tuple.pane_id,"label":desired}),
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
        verify(&after)?;
        Ok(())
    }

    pub fn project_names(&mut self) -> Result<NameReport> {
        self.begin_observation();
        self.guard_recovering()?;
        let state = self.state.clone();
        let _lock = state.lock(self.config.timeout_seconds, &self.cancelled)?;
        self.fresh_under_lock();
        let mut report = NameReport {
            workspace: NameOutcome::Disabled,
            tab: NameOutcome::Disabled,
        };
        for kind in [SurfaceKind::Workspace, SurfaceKind::Tab] {
            if !match kind {
                SurfaceKind::Workspace => self.config.workspace_naming,
                SurfaceKind::Tab => self.config.tab_naming,
            } {
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
            let outcome = if label.label == desired {
                NameOutcome::Unchanged
            } else {
                let (native_kind, id_field, result_kind) = match kind {
                    SurfaceKind::Workspace => ("workspace", "workspace_id", "workspace_info"),
                    SurfaceKind::Tab => ("tab", "tab_id", "tab_info"),
                };
                let result = self.call(
                    &format!("{native_kind}.rename"),
                    serde_json::json!({id_field:label.id,"label":desired}),
                    Some(result_kind),
                    true,
                )?;
                let ack = &result[native_kind];
                let after = self.projection_proof()?;
                let actual = surface(&after, kind)?;
                if ack[id_field] != label.id
                    || ack["workspace_id"] != before.owner.workspace_id
                    || ack["number"].as_u64() != Some(label.number)
                    || ack["label"] != desired
                    || actual.id != label.id
                    || actual.number != label.number
                    || actual.label != desired
                    || desired_name(&after, kind) != desired
                    || after.owner != before.owner
                    || after.bound()? != before.bound()?
                    || (kind == SurfaceKind::Tab && !after.snapshot.first_pane(after.owner_pane()))
                {
                    return Err(self.refuse("native name acknowledgement changed"));
                }
                NameOutcome::Renamed
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
