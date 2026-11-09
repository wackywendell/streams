use super::*;
use crate::tui::ResolvedDocument;

impl NativeCardEngine {
    pub fn open_document(&mut self, target: &DocumentTarget) -> Result<()> {
        let card = self.source.read(&self.owner_pane)?;
        if card.scope() != self.card.scope() || card.owner() != self.card.owner() {
            return Err(invalid(
                "document viewing scope changed; redraw before opening",
            ));
        }
        let document = target
            .document()
            .ok_or_else(|| invalid("document reference required"))?;
        if card
            .document_target(&document.repository_url, &document.path)
            .as_ref()
            != Some(target)
        {
            return Err(invalid(
                "document viewing context changed; redraw before opening",
            ));
        }
        let resolved = self.source.resolve_document(target)?;
        let expected = target
            .editor_identity()
            .map_err(|error| invalid(error.to_string()))?;
        let ResolvedDocument::File { selection, .. } = &target.location else {
            return Err(invalid("document is unavailable"));
        };
        if resolved.identity != expected
            || resolved.root != selection.root
            || !resolved.identity.file.starts_with(&resolved.root)
            || resolved.identity.file == resolved.root
            || resolved.root.canonicalize()? != resolved.root
            || resolved.identity.file.canonicalize()? != resolved.identity.file
            || !std::fs::metadata(&resolved.identity.file)?.is_file()
        {
            return Err(invalid("document or checkout changed during resolution"));
        }
        let key = json_digest(&(
            &resolved.identity.catalog_source,
            &resolved.identity.host_id,
            &resolved.identity.checkout_id,
            &resolved.identity.file,
        ))?;
        let existing = self
            .snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| {
                self.snapshot
                    .pane_tokens(&pane.pane.id)
                    .is_some_and(|tokens| {
                        tokens.get("strm_role").is_some_and(|role| role == "editor")
                            && tokens.get("strm_doc") == Some(&key)
                    })
            })
            .cloned();
        let (editor, created) = if let Some(editor) = existing {
            (editor, false)
        } else {
            let command = self.config.document_command(&self.environment)?;
            let root = self.config.plugin_root.canonicalize()?;
            self.ensure_plugin()?;
            let mut env = BTreeMap::from([
                ("WORK_CARDS_DOCUMENT_COMMAND", command),
                (
                    "WORK_CARDS_DOCUMENT_CWD",
                    actions::path_text(&resolved.root)?.into(),
                ),
                (
                    "WORK_CARDS_DOCUMENT_PATH",
                    actions::path_text(&resolved.identity.file)?.into(),
                ),
            ]);
            if let Some(path) = self.environment.get("PATH") {
                env.insert("PATH", path.clone());
            }
            let opened = self.call("plugin.pane.open", json!({"plugin_id":PLUGIN_ID,"entrypoint":"document",
                "placement":"tab","workspace_id":self.owner_pane.pane.workspace_id,"cwd":root,"focus":false,"env":env}), "plugin_pane_opened", true)?;
            let editor = actions::plugin_pane(&opened, "document")?;
            if editor.pane.workspace_id != self.owner_pane.pane.workspace_id {
                return Err(self.refuse("editor opened in an unexpected workspace"));
            }
            self.call(
                "pane.report_metadata",
                json!({"pane_id":editor.pane.id,"source":"streams",
                "tokens":{"strm_role":"editor","strm_doc":key}}),
                "ok",
                true,
            )?;
            (editor, true)
        };
        let first = created || self.snapshot.first_pane(&editor);
        let title = &document.path;
        let differs = self
            .snapshot
            .topology()
            .tabs
            .iter()
            .find(|tab| tab.id == editor.pane.tab_id)
            .and_then(|tab| tab.label.as_ref())
            != Some(title);
        if self.config.tab_naming && first && differs {
            self.rename("tab", &editor.pane.tab_id, title.clone())?;
        }
        self.focus_tab(&editor.pane.tab_id)
    }
}
