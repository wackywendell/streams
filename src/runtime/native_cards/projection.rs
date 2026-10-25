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

impl NativeCardEngine {
    pub fn project(&mut self) -> Result<NameReport> {
        let owner = self.owner_pane.clone();
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
        let workspace_title = self.card.stream().title.clone();
        let session_title = self.card.session().title.clone();
        let association = stream_tokens(
            &self.card.scope().catalog_source,
            &self.card.scope().host_id,
            &self.card.scope().stream_id,
        );
        let current = self
            .snapshot
            .workspace_tokens(&workspace.id)
            .expect("workspace tokens");
        if association
            .iter()
            .any(|(key, value)| current.get(key).is_some_and(|current| current != value))
        {
            return Err(invalid(
                "workspace markers name another Stream, catalog or host",
            ));
        }
        let workspace_values = changed(
            current,
            association
                .into_iter()
                .map(|(key, value)| (key, Some(value)))
                .chain([(
                    "strm_goal_short".into(),
                    compact(self.card.stream().goal_short.as_deref()),
                )]),
        );
        let session_values = changed(
            self.snapshot
                .pane_tokens(&owner.pane.id)
                .expect("Pi tokens"),
            [
                (
                    "strm_status".into(),
                    Some(self.card.session().status.to_string()),
                ),
                (
                    "strm_state_short".into(),
                    compact(self.card.session().state_short.as_deref()),
                ),
            ],
        );
        let report = NameReport {
            workspace: if !self.config.workspace_naming {
                NameOutcome::Disabled
            } else if workspace.label == workspace_title {
                NameOutcome::Unchanged
            } else {
                NameOutcome::Renamed
            },
            tab: if !self.config.tab_naming {
                NameOutcome::Disabled
            } else if !self.snapshot.first_pane(&owner) {
                NameOutcome::NotFirst
            } else if tab.label.as_deref() == Some(&session_title) {
                NameOutcome::Unchanged
            } else {
                NameOutcome::Renamed
            },
        };
        let caption = self
            .display
            .as_ref()
            .filter(|pane| self.snapshot.pane_label(&pane.pane.id) != Some(&workspace_title))
            .cloned();
        if report.workspace == NameOutcome::Renamed || caption.is_some() {
            config::validate_title(&workspace_title)?;
        }
        if report.tab == NameOutcome::Renamed {
            config::validate_title(&session_title)?;
        }
        // The complete comparison above is pure. Checked replies complete these
        // actions; there are no confirmation reads or remembered projections.
        for (kind, id, values) in [
            ("workspace", &owner.pane.workspace_id, workspace_values),
            ("pane", &owner.pane.id, session_values),
        ] {
            if !values.is_empty() {
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
        }
        if report.workspace == NameOutcome::Renamed {
            self.rename(
                "workspace",
                &owner.pane.workspace_id,
                workspace_title.clone(),
            )?;
        }
        if report.tab == NameOutcome::Renamed {
            self.rename("tab", &owner.pane.tab_id, session_title)?;
        }
        if let Some(card) = caption {
            self.rename("pane", &card.pane.id, workspace_title)?;
        }
        self.follow_display()?;
        Ok(report)
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
                && value.chars().count() <= 80
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
