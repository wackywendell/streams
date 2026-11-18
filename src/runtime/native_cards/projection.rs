use super::*;

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
impl Default for NameReport {
    fn default() -> Self {
        Self {
            workspace: NameOutcome::Disabled,
            tab: NameOutcome::Disabled,
        }
    }
}

enum ProjectionAction {
    Metadata {
        kind: &'static str,
        id: String,
        values: BTreeMap<String, Option<String>>,
    },
    Rename {
        kind: &'static str,
        id: String,
        title: String,
    },
    Follow(HerdrInventoryPane),
}
struct CardProjection {
    actions: Vec<ProjectionAction>,
    report: NameReport,
}

impl NativeCardEngine {
    pub fn lifecycle(
        &mut self,
        event: crate::integration::LifecycleEvent,
    ) -> Result<crate::integration::LifecycleReport> {
        use crate::integration::{
            CompanionDisposition, CompanionIntent, LifecycleEvent, LifecycleReport, NativeStatus,
            ProjectionResult,
        };
        let intent = event.companion_intent(self.config.startup_automatic());
        let desired = self.desired_projection(intent);
        let plan = self.prepare_projection(&desired)?;
        self.apply_card_projection(plan)?;
        let companion = if intent == CompanionIntent::Show {
            Some(match self.open_display(OpenMode::Explicit)? {
                DisplayOutcome::Opened => CompanionDisposition::Opened,
                DisplayOutcome::Existing => CompanionDisposition::Existing,
                DisplayOutcome::Suppressed => CompanionDisposition::Suppressed,
            })
        } else if matches!(event, LifecycleEvent::Started | LifecycleEvent::NewlyBound) {
            Some(if self.display.is_some() {
                CompanionDisposition::Existing
            } else {
                CompanionDisposition::Suppressed
            })
        } else {
            None
        };
        Ok(LifecycleReport {
            native: NativeStatus::Active,
            projection: ProjectionResult {
                companion,
                desired_pi_name: self
                    .config
                    .session_naming()
                    .then(|| self.card.session().title.clone()),
                warnings: vec![],
            },
        })
    }

    pub fn project(&mut self) -> Result<NameReport> {
        let desired = self.desired_projection(crate::integration::CompanionIntent::Keep);
        let plan = self.prepare_projection(&desired)?;
        self.apply_card_projection(plan)
    }

    fn desired_projection(
        &self,
        companion: crate::integration::CompanionIntent,
    ) -> crate::integration::DesiredProjection {
        use crate::frontend::WorkspaceFrontend;
        use crate::integration::{AssociationIntent, DesiredProjection, NativeRef};
        let space = NativeRef {
            adapter: self.client.frontend_id(),
            endpoint: self.client.endpoint_id(),
            key: self.owner_pane.pane.workspace_id.clone(),
        };
        DesiredProjection::for_session(
            self.card.stream(),
            self.card.session(),
            space.clone(),
            AssociationIntent::Bind { space },
            self.config.workspace_naming,
            self.config.tab_naming,
            companion,
        )
    }

    fn prepare_projection(
        &self,
        desired: &crate::integration::DesiredProjection,
    ) -> Result<CardProjection> {
        use crate::integration::{AssociationIntent, NameIntent};
        let [session] = desired.sessions.as_slice() else {
            return Err(invalid("card projection requires one Session"));
        };
        let owner = &self.owner_pane;
        let workspace = self
            .snapshot
            .topology()
            .workspaces
            .iter()
            .find(|workspace| workspace.id == owner.pane.workspace_id)
            .expect("located workspace");
        let tab = self
            .snapshot
            .topology()
            .tabs
            .iter()
            .find(|tab| tab.id == owner.pane.tab_id)
            .expect("located tab");
        let association = stream_tokens(
            &self.card.scope().catalog_source,
            &self.card.scope().host_id,
            &desired.stream,
        );
        let current = self
            .snapshot
            .workspace_tokens(&workspace.id)
            .expect("workspace tokens");
        // Markers present on the workspace, with any missing one taken as the Session's.
        let named: BTreeMap<_, _> = association
            .iter()
            .map(|(key, value)| (key.clone(), current.get(key).unwrap_or(value).clone()))
            .collect();
        let observed = association
            .keys()
            .any(|key| current.contains_key(key))
            .then_some(&named);
        if crate::operations::membership::placement(&association, observed)
            == crate::operations::membership::Placement::Conflict
        {
            return Err(invalid(
                "workspace markers name another Stream, catalog or host",
            ));
        }
        let binding = match &desired.association {
            AssociationIntent::Keep => BTreeMap::new(),
            AssociationIntent::Bind { .. } => association,
        };
        let workspace_values = changed(
            current,
            binding
                .into_iter()
                .map(|(key, value)| (key, Some(value)))
                .chain([(
                    "strm_goal_short".into(),
                    compact(desired.stream_display.goal_short.as_deref()),
                )]),
        );
        let session_values = changed(
            self.snapshot
                .pane_tokens(&owner.pane.id)
                .expect("Pi tokens"),
            [
                ("strm_status".into(), Some(session.view.status.to_string())),
                (
                    "strm_recap_short".into(),
                    compact(session.view.recap_short.as_deref()),
                ),
                (
                    "strm_state_short".into(),
                    compact(session.view.state_short.as_deref()),
                ),
                (
                    "strm_remaining_short".into(),
                    compact(session.view.remaining_short.as_deref()),
                ),
            ],
        );
        let report = NameReport {
            workspace: match &desired.stream_name {
                NameIntent::LeaveUnchanged => NameOutcome::Disabled,
                NameIntent::Set(title) if workspace.label == *title => NameOutcome::Unchanged,
                NameIntent::Set(_) => NameOutcome::Renamed,
            },
            tab: match &session.name {
                NameIntent::LeaveUnchanged => NameOutcome::Disabled,
                NameIntent::Set(_) if !self.snapshot.first_pane(owner) => NameOutcome::NotFirst,
                NameIntent::Set(title) if tab.label.as_deref() == Some(title.as_str()) => {
                    NameOutcome::Unchanged
                }
                NameIntent::Set(_) => NameOutcome::Renamed,
            },
        };
        let mut actions = vec![];
        for (kind, id, values) in [
            ("workspace", &owner.pane.workspace_id, workspace_values),
            ("pane", &owner.pane.id, session_values),
        ] {
            if !values.is_empty() {
                actions.push(ProjectionAction::Metadata {
                    kind,
                    id: id.clone(),
                    values,
                });
            }
        }
        if let NameIntent::Set(title) = &desired.stream_name
            && report.workspace == NameOutcome::Renamed
        {
            config::validate_title(title)?;
            actions.push(ProjectionAction::Rename {
                kind: "workspace",
                id: owner.pane.workspace_id.clone(),
                title: title.clone(),
            });
        }
        if let NameIntent::Set(title) = &session.name
            && report.tab == NameOutcome::Renamed
        {
            config::validate_title(title)?;
            actions.push(ProjectionAction::Rename {
                kind: "tab",
                id: owner.pane.tab_id.clone(),
                title: title.clone(),
            });
        }
        let caption = format!("{}: {}", desired.stream_display.title, session.view.title);
        if let Some(card) = self
            .display
            .as_ref()
            .filter(|card| self.snapshot.pane_label(&card.pane.id) != Some(caption.as_str()))
        {
            config::validate_title(&caption)?;
            actions.push(ProjectionAction::Rename {
                kind: "pane",
                id: card.pane.id.clone(),
                title: caption,
            });
        }
        if let Some(card) = self
            .display
            .as_ref()
            .filter(|card| card.pane.tab_id != owner.pane.tab_id)
        {
            actions.push(ProjectionAction::Follow(card.clone()));
        }
        Ok(CardProjection { actions, report })
    }

    fn apply_card_projection(&mut self, plan: CardProjection) -> Result<NameReport> {
        for action in plan.actions {
            match action {
                ProjectionAction::Metadata { kind, id, values } => {
                    self.call(
                        if kind == "workspace" {
                            "workspace.report_metadata"
                        } else {
                            "pane.report_metadata"
                        },
                        json!({format!("{kind}_id"):id,"source":"streams","tokens":values}),
                        "ok",
                        true,
                    )?;
                }
                ProjectionAction::Rename { kind, id, title } => self.rename(kind, &id, title)?,
                ProjectionAction::Follow(card) => self.follow_observed_display(card)?,
            }
        }
        Ok(plan.report)
    }
}

fn changed(
    current: &Tokens,
    desired: impl IntoIterator<Item = (String, Option<String>)>,
) -> BTreeMap<String, Option<String>> {
    desired
        .into_iter()
        .filter(|(key, value)| current.get(key) != value.as_ref())
        .collect()
}
fn compact(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| {
            !value.is_empty()
                && value.trim() == *value
                && value.chars().count() < 25
                && !value.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

/// The card process publishes its own identity, including after a lost launch reply.
/// The child only observes it; neither process writes reciprocal owner markers.
pub fn publish_card_marker(
    client: &HerdrClient,
    snapshot: &HerdrNativeSnapshot,
    pane_id: &str,
    scope: &Scope,
) -> Result<()> {
    let tokens = snapshot
        .pane_tokens(pane_id)
        .ok_or_else(|| invalid("card pane is not observed"))?;
    let desired = card_tokens(scope);
    if desired
        .iter()
        .any(|(key, value)| tokens.get(key).is_some_and(|current| current != value))
    {
        return Err(invalid("card pane carries another Session's marker"));
    }
    let values = changed(
        tokens,
        desired.into_iter().map(|(key, value)| (key, Some(value))),
    );
    if !values.is_empty() {
        client.card_request(
            "pane.report_metadata",
            json!({"pane_id":pane_id,"source":"streams","tokens":values}),
            Some("ok"),
            true,
        )?;
    }
    Ok(())
}
