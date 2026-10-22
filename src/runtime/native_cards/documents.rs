use super::super::native_names::{
    NameDecision, NameInput, NameIntent, NameOwner, NameStore, SurfaceKind, decide_name,
};
use super::super::open_protocol::{Action, Protocol};
use super::actions::MetadataTarget;
use super::config::validate_title;
use super::*;
use crate::tui::ResolvedDocument;

impl NativeCardEngine {
    fn document_proof(
        &mut self,
        target: &DocumentTarget,
    ) -> Result<(Verified, ResolvedEditor, String)> {
        let v = self.marked()?;
        if !matches!(v.pair()?, PairState::Paired { .. })
            || v.saved.card.scope() != &target.scope
            || v.saved
                .card
                .document_target(&target.document.repository, &target.document.path)
                .as_ref()
                != Some(target)
        {
            return Err(
                self.refuse("document viewing scope/reference changed; redraw before opening")
            );
        }
        let resolved = self
            .source
            .resolve_document(&self.client, &v.snapshot, target)?;
        let expected = target
            .editor_identity()
            .map_err(|e| invalid(e.to_string()))?;
        let ResolvedDocument::File { selection, .. } = &target.location else {
            return Err(self.refuse("document is unavailable"));
        };
        if resolved.identity != expected
            || resolved.root != selection.root
            || !resolved.identity.file.is_absolute()
            || !resolved.identity.file.starts_with(&resolved.root)
            || resolved.identity.file == resolved.root
            || std::fs::canonicalize(&resolved.root)? != resolved.root
            || std::fs::canonicalize(&resolved.identity.file)? != resolved.identity.file
            || !std::fs::metadata(&resolved.identity.file)?.is_file()
        {
            return Err(self.refuse("application document resolution changed"));
        }
        let key = json_digest(&(
            &resolved.identity.catalog_source,
            &resolved.identity.host_id,
            &resolved.identity.checkout_id,
            &resolved.identity.file,
        ))?;
        Ok((v, resolved, key))
    }
    pub fn open_document(&mut self, target: &DocumentTarget) -> Result<()> {
        self.begin_observation();
        let state = self.state.clone();
        let _lock = state.lock(
            &self.client.endpoint().socket,
            &self.caller.workspace_id,
            self.config.timeout_seconds,
            &self.cancelled,
        )?;
        self.fresh_under_lock();
        let (mut v, resolved, key) = self.document_proof(target)?;
        let evidence_key = json_digest(&(
            &self.client.endpoint().socket,
            &v.owner.workspace_id,
            "editor",
            &key,
        ))?;
        let old = state.read(&evidence_key)?;
        if let Some(editor) = editor_match(&v, &key)? {
            if let Some(record) = old
                .as_ref()
                .filter(|record| matches!(record.phase(), NativePhase::Reserved(_)))
            {
                self.editor_proof(target, &key, &evidence_key, &editor, record)?;
                let mut ready = record.clone();
                ready.mark_ready()?;
                _lock.write(Some(record), &ready)?;
            }
            return self.focus_editor(target, &key, &evidence_key, &editor);
        }
        if old.as_ref().is_some_and(|record| {
            matches!(
                record.phase(),
                NativePhase::Unlaunched | NativePhase::Reserved(_)
            )
        }) {
            return Err(self.refuse("uncertain editor launch retained; no duplicate launched"));
        }
        if old
            .as_ref()
            .and_then(NativeAttempt::resources)
            .is_some_and(|r| {
                v.snapshot.topology().panes.iter().any(|p| {
                    p.pane.id == r.pane_id()
                        || p.terminal_id == r.terminal_id()
                        || p.pane.tab_id == r.tab_id()
                })
            })
        {
            return Err(
                self.refuse("recorded editor resource remains live without verified ownership")
            );
        }
        self.ensure_plugin(|engine| engine.document_proof(target).map(|_| ()))?;
        let command = self.config.document_command(&self.environment)?;
        let root = std::fs::canonicalize(&self.config.plugin_root)?;
        let (fresh, current, current_key) = self.document_proof(target)?;
        if current != resolved || current_key != key || editor_match(&fresh, &key)?.is_some() {
            return Err(self.refuse("document or editor changed during preparation"));
        }
        v = fresh;
        let pending = NativeAttempt::new(&evidence_key, uuid::Uuid::new_v4().to_string())?;
        _lock.write(old.as_ref(), &pending)?;
        let protocol = Protocol::new()?.next(Action::Dispatch)?;
        let mut args = argv(&[
            "plugin",
            "pane",
            "open",
            "--plugin",
            PLUGIN_ID,
            "--entrypoint",
            "document",
            "--placement",
            "tab",
            "--workspace",
            &v.owner.workspace_id,
            "--cwd",
            path_text(&root)?,
            "--no-focus",
            "--env",
            &format!("WORK_CARDS_DOCUMENT_COMMAND={command}"),
            "--env",
            &format!("WORK_CARDS_DOCUMENT_CWD={}", path_text(&resolved.root)?),
            "--env",
            &format!(
                "WORK_CARDS_DOCUMENT_PATH={}",
                path_text(&resolved.identity.file)?
            ),
        ]);
        if let Some(path) = self.environment.get("PATH") {
            args.extend(["--env".into(), format!("PATH={path}")]);
        }
        let opened = self.call(&args, Some("plugin_pane_opened"), true)?;
        let info = &opened["plugin_pane"];
        let returned = tuple_from_reply(&info["pane"])?;
        if info["plugin_id"] != PLUGIN_ID
            || info["entrypoint"] != "document"
            || returned.workspace_id != v.owner.workspace_id
            || v.snapshot.topology().panes.iter().any(|p| {
                p.pane.id == returned.pane_id
                    || p.terminal_id == returned.terminal_id
                    || p.pane.tab_id == returned.tab_id
            })
        {
            return Err(
                self.refuse("unexpected or replayed editor tuple; pending evidence retained")
            );
        }
        let protocol = protocol.next(Action::Acknowledge)?;
        let mut reserved = pending.clone();
        reserved.reserve(returned.resources()?)?;
        _lock.write(Some(&pending), &reserved)?;
        let (after, latest, latest_key) = self.document_proof(target)?;
        let live = after.snapshot.locate_terminal(&returned.terminal_id)?;
        if latest != resolved
            || latest_key != key
            || NativeTuple::from_pane(live) != returned
            || !after.snapshot.first_pane(live)
        {
            return Err(self.refuse("launched editor tuple or document changed; evidence retained"));
        }
        let target_metadata = MetadataTarget::pane(&after, &returned.pane_id)?;
        let values = BTreeMap::from([
            ("strm_doc".into(), Some(key.clone())),
            ("strm_editor".into(), Some(reserved.nonce().into())),
            ("strm_doc_pane".into(), Some(returned.pane_id.clone())),
            (
                "strm_doc_terminal".into(),
                Some(returned.terminal_id.clone()),
            ),
            ("strm_doc_tab".into(), Some(returned.tab_id.clone())),
        ]);
        let after = self.stamp(target_metadata, values, |fresh| {
            if fresh
                .saved
                .card
                .document_target(&target.document.repository, &target.document.path)
                .as_ref()
                != Some(target)
                || state.read(&evidence_key)?.as_ref() != Some(&reserved)
            {
                return Err(invalid("editor reservation or viewing reference changed"));
            }
            Ok(())
        })?;
        if editor_match(&after, &key)?.as_ref() != Some(&returned) {
            return Err(self.refuse("editor markers did not round-trip"));
        }
        let protocol = protocol.next(Action::Bind)?;
        let mut ready = reserved.clone();
        ready.mark_ready()?;
        _lock.write(Some(&reserved), &ready)?;
        protocol.next(Action::Complete)?;
        if self.config.tab_naming {
            self.name_editor(target, &key, &evidence_key, &returned, &ready, &_lock)?;
        }
        self.focus_editor(target, &key, &evidence_key, &returned)
    }
    fn editor_proof(
        &mut self,
        target: &DocumentTarget,
        key: &str,
        evidence_key: &str,
        expected: &NativeTuple,
        record: &NativeAttempt,
    ) -> Result<Verified> {
        let (v, _, actual_key) = self.document_proof(target)?;
        let tokens = v
            .snapshot
            .pane_tokens(&expected.pane_id)
            .ok_or_else(|| self.refuse("editor disappeared"))?;
        if actual_key != key
            || record.key() != evidence_key
            || !matches!(
                record.phase(),
                NativePhase::Reserved(_) | NativePhase::Ready(_)
            )
            || editor_match(&v, key)?.as_ref() != Some(expected)
            || record
                .resources()
                .is_none_or(|r| !expected.matches_resources(r))
            || tokens.get("strm_editor").map(String::as_str) != Some(record.nonce())
            || self.state.read(evidence_key)?.as_ref() != Some(record)
        {
            return Err(self.refuse("editor has no exact native lifetime"));
        }
        Ok(v)
    }
    fn name_editor(
        &mut self,
        target: &DocumentTarget,
        key: &str,
        evidence_key: &str,
        expected: &NativeTuple,
        ready: &NativeAttempt,
        lock: &NativeStateGuard<'_>,
    ) -> Result<()> {
        if !matches!(ready.phase(), NativePhase::Ready(_)) {
            return Err(self.refuse("editor naming requires ready evidence"));
        }
        let before = self.editor_proof(target, key, evidence_key, expected, ready)?;
        let label = editor_tab(&before, expected)?;
        let desired = &target.document.path;
        validate_title(desired)?;
        let scope = NameStore::prepare_scope(
            lock,
            path_text(&self.client.endpoint().socket)?,
            &expected.workspace_id,
            before.bound()?,
            SurfaceKind::Tab,
            &expected.tab_id,
        )?;
        let prior = NameStore::read(lock, &scope)?;
        let owner = NameOwner::new(&expected.terminal_id, ready.nonce())?;
        let fresh = self.editor_proof(target, key, evidence_key, expected, ready)?;
        if editor_tab(&fresh, expected)? != label
            || fresh.owner != before.owner
            || fresh.bound()? != scope.binding_nonce()
        {
            return Err(self.refuse("editor naming source, owner or label changed"));
        }
        let decision = decide_name(NameInput {
            scope: &scope,
            provenance: prior.as_ref().map(|p| &p.record),
            owner: &owner,
            current_label: &label.0,
            desired_name: desired,
            default_label_proven: label.0 == (label.2 + 1).to_string(),
            intent: NameIntent::Automatic,
        })?;
        let revision = prior.as_ref().map(|p| p.revision.as_str());
        match decision {
            NameDecision::Noop => {}
            NameDecision::Adopt(provenance) | NameDecision::Manual(provenance) => {
                NameStore::write(lock, &scope, &provenance, revision, &self.cancelled)?;
            }
            NameDecision::Rename {
                name,
                after_success,
            } => {
                let result = self.call(
                    &argv(&["tab", "rename", &expected.tab_id, &name]),
                    Some("tab_info"),
                    true,
                )?;
                let ack = &result["tab"];
                let after = self.editor_proof(target, key, evidence_key, expected, ready)?;
                let actual = editor_tab(&after, expected)?;
                if ack["tab_id"] != expected.tab_id
                    || ack["workspace_id"] != expected.workspace_id
                    || ack["number"].as_u64() != Some(label.1)
                    || ack["label"] != name
                    || actual != (name, label.1, label.2)
                    || after.owner != before.owner
                    || after.bound()? != scope.binding_nonce()
                {
                    return Err(self
                        .refuse("editor name acknowledgement changed; provenance not published"));
                }
                NameStore::write(lock, &scope, &after_success, revision, &self.cancelled)?;
            }
        }
        Ok(())
    }
    fn focus_editor(
        &mut self,
        target: &DocumentTarget,
        key: &str,
        evidence_key: &str,
        expected: &NativeTuple,
    ) -> Result<()> {
        let record = self
            .state
            .read(evidence_key)?
            .ok_or_else(|| self.refuse("editor lacks durable launch proof"))?;
        self.editor_proof(target, key, evidence_key, expected, &record)?;
        if !matches!(record.phase(), NativePhase::Ready(_)) {
            return Err(self.refuse("editor has no exact ready native lifetime"));
        }
        self.call(&argv(&["tab", "focus", &expected.tab_id]), None, true)?;
        self.editor_proof(target, key, evidence_key, expected, &record)?;
        Ok(())
    }
}
fn editor_tab(v: &Verified, expected: &NativeTuple) -> Result<(String, u64, usize)> {
    let (position, tab) = v
        .snapshot
        .topology()
        .tabs
        .iter()
        .filter(|tab| tab.workspace_id == expected.workspace_id)
        .enumerate()
        .find(|(_, tab)| tab.id == expected.tab_id)
        .ok_or_else(|| invalid("editor tab disappeared"))?;
    Ok((
        tab.label
            .clone()
            .ok_or_else(|| invalid("editor tab lacks label"))?,
        v.snapshot
            .tab_number(&tab.id)
            .ok_or_else(|| invalid("editor tab lacks number"))?,
        position,
    ))
}
fn editor_match(v: &Verified, key: &str) -> Result<Option<NativeTuple>> {
    let mut found = None;
    for pane in &v.snapshot.topology().panes {
        let tokens = v
            .snapshot
            .pane_tokens(&pane.pane.id)
            .expect("validated pane tokens");
        if pane.pane.workspace_id != v.owner.workspace_id
            || tokens.get("strm_doc").map(String::as_str) != Some(key)
        {
            continue;
        }
        let tuple = NativeTuple::from_pane(pane);
        if tokens.get("strm_editor").is_none_or(String::is_empty)
            || tokens.get("strm_doc_pane") != Some(&tuple.pane_id)
            || tokens.get("strm_doc_terminal") != Some(&tuple.terminal_id)
            || tokens.get("strm_doc_tab") != Some(&tuple.tab_id)
            || !v.snapshot.first_pane(pane)
            || found.is_some()
        {
            return Err(invalid("duplicate or changed owned editor tuple"));
        }
        found = Some(tuple);
    }
    Ok(found)
}
fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| invalid("native path must be UTF-8"))
}
