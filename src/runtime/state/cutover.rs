//! Offline translation of relationships; pending attempts and their fences are never rewritten.
use super::*;
use crate::model::Session;
use crate::runtime::arrangements::{ArrangedSession, ArrangementTab};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct PreparedRelationshipCutover {
    root: PathBuf,
    sources: BTreeMap<PathBuf, Vec<u8>>,
    writes: BTreeMap<PathBuf, Value>,
    _locks: Vec<file_io::FileLock>,
}
impl RuntimeState {
    pub(crate) fn prepare_cutover(
        &self,
        sessions: &BTreeMap<String, String>,
        streams: &BTreeSet<String>,
    ) -> Result<PreparedRelationshipCutover> {
        let associations = self.associations(None)?;
        let arrangements = self.arrangements()?;
        let claims = self.enrollment_claims(None)?;
        let mut workspace_locks = BTreeSet::new();
        let mut stream_locks = BTreeSet::new();
        for (scope, workspace, stream) in associations
            .iter()
            .map(|value| {
                (
                    value.scope(),
                    value.record().workspace_id(),
                    value.record().stream_id(),
                )
            })
            .chain(claims.iter().map(|value| {
                (
                    value.scope(),
                    value.record().workspace_id(),
                    value.record().stream_id(),
                )
            }))
        {
            workspace_locks.insert(
                self.path("enrollments", scope, workspace)?
                    .with_extension("lock"),
            );
            stream_locks.insert(
                self.root
                    .join("stream-locks")
                    .join(format!("{}.lock", key(&[stream]))),
            );
        }
        for value in &arrangements {
            stream_locks.insert(
                self.root
                    .join("stream-locks")
                    .join(format!("{}.lock", key(&[value.record().stream_id()]))),
            );
        }
        let mut guards = Vec::new();
        // Match the live workspace-before-Stream lock order, without attempting native admission.
        for path in workspace_locks.into_iter().chain(stream_locks) {
            self.writable(&path)?;
            guards.push(lock(&path)?);
        }
        if self.associations(None)? != associations
            || self.arrangements()? != arrangements
            || self.enrollment_claims(None)? != claims
        {
            return Err(invalid(
                "runtime relationships changed while acquiring cutover locks; stop all writers before cutover",
            ));
        }
        let mut sources = BTreeMap::new();
        let mut writes = BTreeMap::new();
        for value in associations {
            let mut record = value.record;
            let path = self.path("associations", &value.scope, &record.workspace_id)?;
            sources.insert(
                path.clone(),
                file_io::read_bytes(&path)?
                    .ok_or_else(|| invalid("association disappeared during offline cutover"))?,
            );
            if !streams.contains(&record.stream_id) {
                continue;
            }
            if let Some(launch) = &mut record.launch {
                launch.session_id=sessions.get(&launch.session_id).ok_or_else(||invalid("association has an unusable Session reference; explicitly reset it before cutover"))?.clone();
            }
            writes.insert(
                path,
                serde_json::to_value(record.wire()).map_err(|error| invalid(error.to_string()))?,
            );
        }
        for value in arrangements {
            let plan = value.record;
            let path = self.path("arrangements", &value.scope, plan.stream_id())?;
            sources.insert(
                path.clone(),
                file_io::read_bytes(&path)?
                    .ok_or_else(|| invalid("arrangement disappeared during offline cutover"))?,
            );
            if !streams.contains(plan.stream_id()) {
                continue;
            }
            let mut tabs = Vec::new();
            for tab in plan.tabs() {
                let mut entries = Vec::new();
                for entry in tab.sessions() {
                    let id=sessions.get(entry.session_id()).ok_or_else(||invalid("arrangement has an unusable Session reference; explicitly recapture or clear it before cutover"))?;
                    if *id != Session::id_for(&self.host_id, entry.harness(), entry.native_id()) {
                        return Err(invalid(
                            "arrangement native identity conflicts with catalog Session",
                        ));
                    }
                    entries.push(ArrangedSession::new(
                        id.clone(),
                        entry.harness().into(),
                        entry.native_id().into(),
                        entry.locator().map(str::to_owned),
                        entry.pane_id().map(str::to_owned),
                        entry.terminal_id().map(str::to_owned),
                    )?);
                }
                tabs.push(ArrangementTab::new(
                    tab.tab_id().into(),
                    tab.label().map(str::to_owned),
                    tab.label_mode(),
                    entries,
                )?);
            }
            let plan = Arrangement::new(plan.stream_id().into(), plan.workspace_id().into(), tabs)?;
            let mut wire =
                serde_json::to_value(plan).map_err(|error| invalid(error.to_string()))?;
            wire["version"] = Value::from(1);
            writes.insert(path, wire);
        }
        for value in claims {
            let path = self.path("enrollments", &value.scope, value.record.workspace_id())?;
            sources.insert(
                path.clone(),
                file_io::read_bytes(&path)?
                    .ok_or_else(|| invalid("claim disappeared during offline cutover"))?,
            );
        }
        Ok(PreparedRelationshipCutover {
            root: self.root.clone(),
            sources,
            writes,
            _locks: guards,
        })
    }
}
impl PreparedRelationshipCutover {
    pub(crate) fn install(&self, archive: &Path) -> Result<()> {
        for (path, bytes) in &self.sources {
            let relative = path
                .strip_prefix(&self.root)
                .expect("relationship is under runtime root");
            let target = archive.join("runtime").join(relative);
            file_io::atomic_write(
                &target,
                bytes,
                target.parent().expect("archived relationship parent"),
            )?;
        }
        for (path, value) in &self.writes {
            write_json(path, value)?;
        }
        for path in self
            .sources
            .keys()
            .filter(|path| !self.writes.contains_key(*path))
        {
            fs::remove_file(path)?;
            file_io::sync_directory(path.parent().expect("relationship parent"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
