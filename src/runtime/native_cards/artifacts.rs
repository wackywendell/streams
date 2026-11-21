use super::*;
use crate::{
    artifacts::{ArtifactKey, ArtifactSummary},
    model::Session,
};

/// Validate a producing key against one freshly supplied Session, even after a move.
pub(crate) fn resolve_artifact_summary(
    key: &ArtifactKey,
    session: &Session,
) -> Result<ArtifactSummary> {
    key.validate().map_err(|error| invalid(error.to_string()))?;
    if session.id != key.session || session.harness != key.harness || session.host_id != key.host {
        return Err(invalid("saved artifact reference is unavailable"));
    }
    crate::artifacts::summaries(session)
        .into_iter()
        .find(|summary| &summary.key == key)
        .ok_or_else(|| invalid("saved artifact reference is unavailable"))
}

impl NativeCardEngine {
    pub fn open_artifact(&mut self, target: &ArtifactSummary) -> Result<()> {
        let current = self.source.read_records(&self.owner_pane)?;
        if current.scope() != self.card.scope() || current.owner() != self.card.owner() {
            return Err(invalid(
                "artifact viewing scope changed; redraw before opening",
            ));
        }
        if resolve_artifact_summary(&target.key, current.session())? != *target {
            return Err(invalid("artifact reference changed; redraw before opening"));
        }
        self.open_resolved_artifact(&target.key)
    }

    /// Native effects for an already-validated producing key, including moved producers.
    pub fn open_resolved_artifact(&mut self, key: &crate::artifacts::ArtifactKey) -> Result<()> {
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
        env.insert("STREAMS_ARTIFACT_REFERENCE".into(), key.reference());
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
            self.rename("tab", &viewer.pane.tab_id, key.name.clone())?;
        }
        self.focus_tab(&viewer.pane.tab_id)
    }
}
