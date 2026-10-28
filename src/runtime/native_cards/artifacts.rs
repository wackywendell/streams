use super::*;
use crate::artifacts::ArtifactSummary;

impl NativeCardEngine {
    pub fn open_artifact(&mut self, target: &ArtifactSummary) -> Result<()> {
        if !crate::artifacts::summaries(self.card.session(), &self.card.stream().id)
            .contains(target)
        {
            return Err(invalid("artifact reference changed; redraw before opening"));
        }
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
        if self.config.tab_naming {
            self.rename("tab", &viewer.pane.tab_id, target.key.name.clone())?;
        }
        self.focus_tab(&viewer.pane.tab_id)
    }
}
