use super::*;
use crate::artifacts::ArtifactSummary;

impl NativeCardEngine {
    pub fn open_artifact(&mut self, target: &ArtifactSummary) -> Result<()> {
        if !crate::artifacts::summaries(self.card.session(), &self.card.stream().id)
            .contains(target)
        {
            return Err(invalid("artifact reference changed; redraw before opening"));
        }
        let key = json_digest(&(&self.card.scope().catalog_source, &target.key))?;
        let existing = self
            .snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| {
                self.snapshot
                    .pane_tokens(&pane.pane.id)
                    .is_some_and(|tokens| {
                        tokens
                            .get("strm_role")
                            .is_some_and(|role| role == "artifact")
                            && tokens.get("strm_artifact") == Some(&key)
                    })
            })
            .cloned();
        let (viewer, created) = if let Some(viewer) = existing {
            (viewer, false)
        } else {
            self.ensure_plugin()?;
            let launch = self.source.launch_context();
            let mut env: BTreeMap<_, _> = launch
                .environment
                .into_iter()
                .filter(|(name, _)| SOURCE_ENV_KEYS.contains(&name.as_str()))
                .collect();
            env.insert(
                "STREAMS_BIN_PATH".into(),
                actions::path_text(&std::env::current_exe()?)?.into(),
            );
            env.insert(
                "STREAMS_ARTIFACT_REFERENCE".into(),
                target.reference.clone(),
            );
            env.insert(
                "STREAMS_ARTIFACT_CWD".into(),
                actions::path_text(&launch.cwd)?.into(),
            );
            env.insert(
                "STREAMS_ARTIFACT_CATALOG_SOURCE".into(),
                self.card.scope().catalog_source.clone(),
            );
            let opened = self.call("plugin.pane.open", json!({"plugin_id":PLUGIN_ID,"entrypoint":"artifact",
                "placement":"tab","workspace_id":self.owner_pane.pane.workspace_id,"cwd":self.config.plugin_root.canonicalize()?,"focus":false,"env":env}), "plugin_pane_opened", true)?;
            let viewer = actions::plugin_pane(&opened, "artifact")?;
            if viewer.pane.workspace_id != self.owner_pane.pane.workspace_id {
                return Err(self.refuse("artifact viewer opened in an unexpected workspace"));
            }
            self.call("pane.report_metadata", json!({"pane_id":viewer.pane.id,"source":"streams","tokens":{"strm_role":"artifact","strm_artifact":key}}), "ok", true)?;
            (viewer, true)
        };
        if self.config.tab_naming && (created || self.snapshot.first_pane(&viewer)) {
            let differs = self
                .snapshot
                .topology()
                .tabs
                .iter()
                .find(|tab| tab.id == viewer.pane.tab_id)
                .and_then(|tab| tab.label.as_ref())
                != Some(&target.key.name);
            if differs {
                self.rename("tab", &viewer.pane.tab_id, target.key.name.clone())?;
            }
        }
        self.call(
            "tab.focus",
            json!({"tab_id":viewer.pane.tab_id}),
            "ok",
            true,
        )?;
        Ok(())
    }
}
