//! Catalog-owned native workspace orchestration. Effects never resume a loaded journal.

use super::{
    Result,
    arrangements::{self, ArrangedSession, Arrangement, ArrangementTab},
    checkouts::canonical_directory,
    conversations::{ConversationRoots, Harness},
    invalid,
    launch::{SessionCommand, select_session},
    locations::DirectoryBindings,
    native::BuiltinFrontend,
    state::{Association, LaunchReference, OpenEffect, RuntimeState, StreamGuard},
};
use crate::{
    frontend::{
        CreateWorkspace, FrontendId, WorkspaceFrontend, WorkspaceRef, WorkspaceSelector,
        cmux::CmuxClient, herdr::HerdrClient,
    },
    model::{NamingMode, Session, Status, Stream},
    operations::Authority,
    sync::CatalogBackend,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
pub struct SkippedSession {
    pub session_id: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct OpenResult {
    pub stream_id: String,
    pub session_id: Option<String>,
    pub frontend: FrontendId,
    pub workspace_id: String,
    pub reused: bool,
    pub restored: Vec<String>,
    pub skipped: Vec<SkippedSession>,
    pub unavailable: Vec<SkippedSession>,
    pub needs_attention: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrangement_support: Option<&'static str>,
}

pub struct WorkspaceRuntime<'a> {
    state: &'a RuntimeState,
    locations: &'a DirectoryBindings,
    frontend: Option<&'a BuiltinFrontend>,
    backend: &'a dyn CatalogBackend,
    roots: &'a ConversationRoots,
    environment: &'a BTreeMap<String, String>,
}
impl<'a> WorkspaceRuntime<'a> {
    pub fn new(
        state: &'a RuntimeState,
        locations: &'a DirectoryBindings,
        frontend: Option<&'a BuiltinFrontend>,
        backend: &'a dyn CatalogBackend,
        roots: &'a ConversationRoots,
        environment: &'a BTreeMap<String, String>,
    ) -> Self {
        Self {
            state,
            locations,
            frontend,
            backend,
            roots,
            environment,
        }
    }
    fn frontend(&self) -> Result<&BuiltinFrontend> {
        self.frontend.ok_or_else(|| invalid("no native frontend selected; pass --frontend herdr or --frontend cmux (or use resume)"))
    }
    fn lock(&self, stream: &Stream) -> Result<(StreamGuard<'a>, Authority)> {
        let scope = self.frontend()?.scope();
        let guard = self.state.stream_lock(Some(&scope), &stream.id)?;
        let authority = authorize(self.backend, self.state.host_id(), &stream.id)?;
        Ok((guard, authority))
    }
    fn recheck(&self, stream: &Stream, expected: &Authority) -> Result<()> {
        if &authorize(self.backend, self.state.host_id(), &stream.id)? != expected {
            return Err(invalid(
                "effective owner authority changed during native operation",
            ));
        }
        Ok(())
    }
    fn directory(
        &self,
        stream: &Stream,
        session: Option<&Session>,
        explicit: Option<&Path>,
    ) -> Result<PathBuf> {
        if let Some(path) = explicit {
            return canonical_directory(path);
        }
        let saved = if let Some(session) = session {
            self.locations.read(&stream.id, Some(&session.id))?
        } else {
            None
        };
        let saved = match saved {
            Some(saved) => Some(saved),
            None => self.locations.read(&stream.id, None)?,
        };
        saved.map(|b| b.cwd).ok_or_else(|| {
            invalid(format!(
                "no local directory for stream {:?}; pass --cwd PATH",
                stream.id
            ))
        })
    }
    fn remember(&self, stream: &Stream, session: Option<&Session>, cwd: &Path) -> Result<()> {
        self.locations.remember(
            &stream.id,
            cwd,
            session.map(|s| s.id.as_str()),
            false,
            None,
            None,
        )?;
        Ok(())
    }
    fn reference(&self, id: &str) -> Result<WorkspaceRef> {
        let adapter = self.frontend()?.adapter();
        Ok(WorkspaceRef {
            frontend: adapter.frontend_id(),
            endpoint: adapter.endpoint_id(),
            native_id: id.into(),
        })
    }
    fn result(
        &self,
        stream: &Stream,
        workspace: &str,
        session: Option<&str>,
        reused: bool,
    ) -> Result<OpenResult> {
        Ok(OpenResult {
            stream_id: stream.id.clone(),
            session_id: session.map(str::to_owned),
            frontend: self.frontend()?.adapter().frontend_id(),
            workspace_id: workspace.into(),
            reused,
            restored: vec![],
            skipped: vec![],
            unavailable: vec![],
            needs_attention: vec![],
            arrangement_support: None,
        })
    }
    pub fn capture(&self, stream: &Stream, sessions: &[Session]) -> Result<Arrangement> {
        let client = self
            .frontend()?
            .herdr()
            .ok_or_else(|| invalid("multi-tab arrangement capture is unavailable for CMUX"))?;
        let (guard, authority) = self.lock(stream)?;
        let binding = guard
            .check_open()?
            .ok_or_else(|| invalid("capture requires one ready bound workspace"))?;
        let plan = arrangements::capture(
            &client.topology()?,
            binding.workspace_id(),
            stream,
            sessions,
            self.state.host_id(),
        )?;
        self.recheck(stream, &authority)?;
        if !plan.tabs().is_empty() {
            guard.write_arrangement(&plan)?;
        }
        Ok(plan)
    }
    pub fn open(
        &self,
        stream: &Stream,
        sessions: &[Session],
        session_ref: Option<&str>,
        cwd: Option<&Path>,
    ) -> Result<OpenResult> {
        let frontend = self.frontend()?;
        let outcome = match frontend {
            BuiltinFrontend::Herdr(client) => {
                self.open_herdr(client, stream, sessions, session_ref, cwd)
            }
            BuiltinFrontend::Cmux(client) => {
                self.open_cmux(client, stream, sessions, session_ref, cwd)
            }
        };
        outcome.map_err(|error| {
            if self
                .state
                .pending_attempt(&frontend.scope(), &stream.id)
                .ok()
                .flatten()
                .is_some()
            {
                super::RuntimeError::OpenIncomplete(error.to_string())
            } else {
                error
            }
        })
    }
    fn open_cmux(
        &self,
        client: &CmuxClient,
        stream: &Stream,
        sessions: &[Session],
        session_ref: Option<&str>,
        cwd: Option<&Path>,
    ) -> Result<OpenResult> {
        let environment = native_environment(&stream.id, self.environment)?;
        let (mut guard, authority) = self.lock(stream)?;
        let session = select_session(stream, sessions, self.state.host_id(), session_ref)?;
        let command = session
            .map(|s| SessionCommand::verified(s, None, self.roots))
            .transpose()?;
        let directory = self.directory(stream, session, cwd)?;
        let launch = session
            .zip(command.as_ref())
            .map(|(s, c)| LaunchReference::new(&s.id, c.command_text()))
            .transpose()?;
        if let Some(binding) = guard.check_open()? {
            let reference = self.reference(binding.workspace_id())?;
            client.resolve(WorkspaceSelector::Exact(&reference))?;
            let claim = self
                .state
                .enrollment_claim(&client_scope(client), binding.workspace_id())?;
            if !(binding.launch().is_none() && claim.is_some())
                && binding.launch() != launch.as_ref()
            {
                return Err(invalid(
                    "workspace has a different session; refusing to duplicate or replace it",
                ));
            }
            if cwd.is_some() {
                self.remember(stream, session, &directory)?;
            }
            self.recheck(stream, &authority)?;
            client.focus(&reference)?;
            let mut result = self.result(
                stream,
                binding.workspace_id(),
                binding.launch().map(LaunchReference::session_id),
                true,
            )?;
            result.arrangement_support = Some("single-session-only");
            return Ok(result);
        }
        super::text(&stream.title)?;
        self.remember(stream, session, &directory)?;
        let request = CreateWorkspace {
            title: stream.title.clone(),
            cwd: directory.clone(),
            environment,
        };
        let mut attempt = guard.begin_attempt(launch.clone(), path_text(&directory)?, None)?;
        let launched = if let Some(launch) = &launch {
            vec![json!({"session_id":launch.session_id(),"command":launch.command()})]
        } else {
            vec![]
        };
        attempt.checkpoint(None, vec![], launched)?;
        let created = attempt.effect(
            OpenEffect::WorkspaceCreate,
            || {
                self.recheck(stream, &authority)?;
                Ok(client
                    .create_session(&request, launch.as_ref().map(LaunchReference::command))?)
            },
            |w| json!({"workspace_id":w.reference.native_id}),
        )?;
        let workspace = &created.reference.native_id;
        attempt.checkpoint(
            Some(workspace),
            attempt.record().effects().to_vec(),
            attempt.record().launches().to_vec(),
        )?;
        if self
            .state
            .association(&client_scope(client), workspace)?
            .is_some()
        {
            return Err(invalid(
                "created workspace already has an association; refusing overwrite",
            ));
        }
        attempt.write_association(&Association::new(
            &stream.id,
            workspace,
            launch.clone(),
            false,
        )?)?;
        self.recheck(stream, &authority)?;
        attempt.write_association(&Association::new(&stream.id, workspace, launch, true)?)?;
        attempt.complete()?;
        let mut result = self.result(stream, workspace, session.map(|s| s.id.as_str()), false)?;
        result.restored = session.map(|s| vec![s.id.clone()]).unwrap_or_default();
        result.arrangement_support = Some("single-session-only");
        Ok(result)
    }
    fn open_herdr(
        &self,
        client: &HerdrClient,
        stream: &Stream,
        sessions: &[Session],
        session_ref: Option<&str>,
        cwd: Option<&Path>,
    ) -> Result<OpenResult> {
        let environment = native_environment(&stream.id, self.environment)?;
        let (mut guard, authority) = self.lock(stream)?;
        let mut binding = guard.check_open()?;
        let mut snapshot = None;
        if let Some(old) = &binding {
            let topology = client.topology()?;
            if !topology
                .workspaces
                .iter()
                .any(|w| w.id == old.workspace_id())
            {
                arrangements::verified_inventory(&topology, None)?;
                self.recheck(stream, &authority)?;
                guard.archive_closed(old)?;
                binding = None;
            }
            snapshot = Some(topology);
        }
        let scope = self.frontend()?.scope();
        let plan = self.state.arrangement(&scope, &stream.id)?;
        if let Some(binding) = &binding {
            let target = session_ref
                .map(|reference| {
                    select_session(stream, sessions, self.state.host_id(), Some(reference))
                })
                .transpose()?
                .flatten();
            let topology = snapshot.as_ref().expect("bound workspace was inspected");
            let live = if let Some(target) = target {
                arrangements::verified_inventory(topology, None)?
                    .into_iter()
                    .find(|a| {
                        a.harness.as_ref() == target.harness
                            && a.identity.id == target.harness_session_id
                    })
                    .map(|a| a.native)
            } else {
                None
            };
            if let Some(cwd) = cwd {
                let local: Vec<_> = sessions
                    .iter()
                    .filter(|s| s.stream_id == stream.id && s.host_id == self.state.host_id())
                    .collect();
                if target.is_none() && local.len() > 1 {
                    return Err(invalid(
                        "--cwd is ambiguous across multiple Sessions; choose --session",
                    ));
                }
                let remembered = target.or_else(|| local.first().copied());
                self.remember(stream, remembered, &canonical_directory(cwd)?)?;
            }
            if live.is_some_and(|a| a.workspace_id != binding.workspace_id()) {
                return Err(invalid(
                    "selected conversation is already running in another native workspace; refusing duplicate launch",
                ));
            }
            if target.is_none() || live.is_some() {
                let mut result = self.result(
                    stream,
                    binding.workspace_id(),
                    target
                        .map(|s| s.id.as_str())
                        .or_else(|| binding.launch().map(LaunchReference::session_id)),
                    true,
                )?;
                match arrangements::capture(
                    topology,
                    binding.workspace_id(),
                    stream,
                    sessions,
                    self.state.host_id(),
                ) {
                    Ok(plan) if !plan.tabs().is_empty() => guard.write_arrangement(&plan)?,
                    Ok(_) => {}
                    Err(error) => result.needs_attention.push(format!(
                        "arrangement capture unavailable; last good layout retained: {error}"
                    )),
                }
                self.recheck(stream, &authority)?;
                client.focus(&self.reference(binding.workspace_id())?)?;
                if let Some(live) = live {
                    let pane = topology
                        .panes
                        .iter()
                        .find(|p| p.pane.id == live.pane_id)
                        .expect("verified agent pane");
                    self.recheck(stream, &authority)?;
                    client.focus_agent(&pane.pane)?;
                }
                return Ok(result);
            }
        }
        let (prepared, skipped, mut attention) =
            self.prepare(stream, sessions, plan.as_ref(), session_ref, cwd)?;
        if let Some(binding) = &binding
            && prepared.is_empty()
        {
            self.recheck(stream, &authority)?;
            client.focus(&self.reference(binding.workspace_id())?)?;
            let mut result = self.result(stream, binding.workspace_id(), None, true)?;
            result.unavailable = skipped.clone();
            result.skipped = skipped;
            result.needs_attention = attention;
            result.needs_attention.push(
                "selected saved conversation unavailable; existing tabs left untouched".into(),
            );
            return Ok(result);
        }
        let mut observed = if let Some(binding) = &binding {
            arrangements::capture(
                snapshot.as_ref().expect("bound workspace was inspected"),
                binding.workspace_id(),
                stream,
                sessions,
                self.state.host_id(),
            )?
            .tabs()
            .to_vec()
        } else {
            vec![]
        };
        if !prepared.is_empty() {
            if snapshot.is_none() {
                snapshot = Some(client.topology()?);
            }
            let agents =
                arrangements::verified_inventory(snapshot.as_ref().expect("host inventory"), None)?;
            let mut identities = BTreeSet::new();
            for entry in prepared.iter().flat_map(|g| &g.entries) {
                if !identities.insert((&entry.session.harness, &entry.session.harness_session_id)) {
                    return Err(invalid(
                        "ambiguous saved arrangement native conversation identity",
                    ));
                }
                if agents.iter().any(|a| {
                    a.harness.as_ref() == entry.session.harness
                        && a.identity.id == entry.session.harness_session_id
                }) {
                    return Err(invalid(format!(
                        "conversation {:?} is already running; refusing duplicate launch",
                        entry.session.id
                    )));
                }
            }
        }
        let first = prepared.first().and_then(|g| g.entries.first());
        let directory = if let Some(first) = first {
            first.cwd.clone()
        } else {
            self.directory(stream, None, cwd)?
        };
        let launch = first
            .map(|e| LaunchReference::new(&e.session.id, e.command.command_text()))
            .transpose()?;
        super::text(&stream.title)?;
        let reused = binding.is_some();
        if !reused {
            self.remember(stream, None, &directory)?;
        }
        let mut attempt = guard.begin_attempt(
            launch.clone(),
            path_text(&directory)?,
            binding.as_ref().map(Association::workspace_id),
        )?;
        let request = CreateWorkspace {
            title: stream.title.clone(),
            cwd: directory,
            environment,
        };
        let (workspace, mut root_pane) = if let Some(binding) = &binding {
            (binding.workspace_id().to_owned(), None)
        } else {
            let created = attempt.effect(OpenEffect::WorkspaceCreate, || {self.recheck(stream, &authority)?; Ok(client.create_native(&request)?)}, |c| json!({"workspace_id":c.workspace.reference.native_id,"tab_id":c.tab.id,"pane_id":c.root_pane.id}))?;
            let workspace = created.workspace.reference.native_id;
            attempt.checkpoint(
                Some(&workspace),
                attempt.record().effects().to_vec(),
                attempt.record().launches().to_vec(),
            )?;
            if self.state.association(&scope, &workspace)?.is_some() {
                return Err(invalid(
                    "created workspace already has an association; refusing overwrite",
                ));
            }
            attempt.write_association(&Association::new(
                &stream.id,
                &workspace,
                launch.clone(),
                false,
            )?)?;
            (workspace, Some((created.tab, created.root_pane)))
        };
        let mut restored = Vec::new();
        for group in &prepared {
            let (tab, mut pane) = if let Some((tab, pane)) = root_pane.take() {
                if let Some(label) = group.manual_label() {
                    attempt.effect(
                        OpenEffect::TabRename {
                            tab_id: tab.id.clone(),
                            label: label.to_owned(),
                        },
                        || {
                            self.recheck(stream, &authority)?;
                            Ok(client.rename_tab(&tab, label)?)
                        },
                        |_| Value::Null,
                    )?;
                }
                (tab, pane)
            } else {
                attempt.effect(
                    OpenEffect::TabCreate,
                    || {
                        self.recheck(stream, &authority)?;
                        Ok(client.create_tab(
                            &self.reference(&workspace)?,
                            &group.entries[0].cwd,
                            &request.environment,
                            group.manual_label(),
                        )?)
                    },
                    |(t, p)| json!({"tab_id":t.id,"pane_id":p.id,"workspace_id":p.workspace_id}),
                )?
            };
            let mut entries = Vec::new();
            for (index, entry) in group.entries.iter().enumerate() {
                if index > 0 {
                    pane = attempt.effect(
                        OpenEffect::PaneSplit {
                            pane_id: pane.id.clone(),
                        },
                        || {
                            self.recheck(stream, &authority)?;
                            Ok(client.split_pane(&pane, &entry.cwd, &request.environment)?)
                        },
                        |p| json!({"pane_id":p.id,"tab_id":p.tab_id,"workspace_id":p.workspace_id}),
                    )?;
                }
                let name = format!(
                    "streams-{}",
                    &uuid::Uuid::new_v4().simple().to_string()[..24]
                );
                let mut launches = attempt.record().launches().to_vec();
                launches.push(json!({"session_id":entry.session.id,"pane_id":pane.id,"tab_id":pane.tab_id,"name":name,"ready":false}));
                attempt.checkpoint(
                    Some(&workspace),
                    attempt.record().effects().to_vec(),
                    launches,
                )?;
                let started = attempt.effect(
                    OpenEffect::AgentStart {session_id:entry.session.id.clone(),pane_id:pane.id.clone(),name:name.clone()},
                    || {self.recheck(stream, &authority)?; Ok(client.start_agent(&pane, &name, entry.command.harness().as_ref(), &entry.command.argv()[1..])?)},
                    |a| json!({"pane_id":a.pane.id,"tab_id":a.pane.tab_id,"workspace_id":a.pane.workspace_id,"name":a.name,"agent":a.agent,"terminal_id":a.terminal_id,"interactive_ready":a.interactive_ready}),
                )?;
                let saved = ArrangedSession::new(
                    entry.session.id.clone(),
                    entry.session.harness.clone(),
                    entry.session.harness_session_id.clone(),
                    Some(path_text(entry.command.locator())?.into()),
                    Some(pane.id.clone()),
                    started.terminal_id,
                )?;
                let topology = client.topology()?;
                let live = arrangements::verified_inventory(&topology, None)?;
                require_live(&live, &workspace, &tab.id, &saved)?;
                let mut launches = attempt.record().launches().to_vec();
                let launch = launches.last_mut().expect("launch was persisted");
                launch["ready"] = Value::Bool(true);
                launch["returned"] =
                    attempt.record().effects().last().expect("launch effect")["returned"].clone();
                attempt.checkpoint(
                    Some(&workspace),
                    attempt.record().effects().to_vec(),
                    launches,
                )?;
                restored.push(entry.session.id.clone());
                entries.push(saved);
            }
            observed.push(ArrangementTab::new(
                tab.id,
                group.manual_label().map(str::to_owned),
                if group.manual_label().is_some() {
                    NamingMode::Manual
                } else {
                    NamingMode::Automatic
                },
                entries,
            )?);
        }
        if !restored.is_empty() {
            let topology = client.topology()?;
            let live = arrangements::verified_inventory(&topology, None)?;
            for tab in &observed {
                for saved in tab
                    .sessions()
                    .iter()
                    .filter(|s| restored.iter().any(|id| id == s.session_id()))
                {
                    require_live(&live, &workspace, tab.tab_id(), saved)?;
                }
            }
        }
        if !observed.is_empty() {
            attempt.write_arrangement(&Arrangement::new(
                stream.id.clone(),
                workspace.clone(),
                observed,
            )?)?;
        }
        self.recheck(stream, &authority)?;
        let association_launch = binding
            .as_ref()
            .map(|b| b.launch().cloned())
            .unwrap_or_else(|| launch.clone());
        attempt.write_association(&Association::new(
            &stream.id,
            &workspace,
            association_launch,
            true,
        )?)?;
        attempt.complete()?;
        self.recheck(stream, &authority)?;
        client.focus(&self.reference(&workspace)?)?;
        if !skipped.is_empty() {
            attention.push(
                "some saved Sessions were unavailable or excluded; inspect skipped diagnostics"
                    .into(),
            );
        }
        let mut result = self.result(
            stream,
            &workspace,
            launch.as_ref().map(LaunchReference::session_id),
            reused,
        )?;
        result.restored = restored;
        result.unavailable = skipped.clone();
        result.skipped = skipped;
        result.needs_attention = attention;
        Ok(result)
    }
    fn prepare<'s>(
        &self,
        stream: &Stream,
        sessions: &'s [Session],
        plan: Option<&Arrangement>,
        reference: Option<&str>,
        cwd: Option<&Path>,
    ) -> Result<(Vec<PreparedTab<'s>>, Vec<SkippedSession>, Vec<String>)> {
        let known: BTreeMap<_, _> = sessions
            .iter()
            .filter(|s| s.stream_id == stream.id && s.host_id == self.state.host_id())
            .map(|s| (s.id.as_str(), s))
            .collect();
        let mut attention = Vec::new();
        let groups = if let Some(reference) = reference {
            let selected = select_session(stream, sessions, self.state.host_id(), Some(reference))?
                .expect("explicit session selection");
            let saved = plan.and_then(|p| {
                p.tabs()
                    .iter()
                    .flat_map(|t| t.sessions())
                    .find(|e| e.session_id() == selected.id)
            });
            vec![(None, vec![(selected.id.as_str(), saved)])]
        } else if let Some(plan) = plan.filter(|p| !p.tabs().is_empty()) {
            plan.tabs()
                .iter()
                .map(|t| {
                    (
                        t.label().map(str::to_owned),
                        t.sessions()
                            .iter()
                            .map(|e| (e.session_id(), Some(e)))
                            .collect(),
                    )
                })
                .collect()
        } else {
            let local: Vec<_> = known.values().copied().filter(|s| !inactive(s)).collect();
            if local.len() > 1 {
                attention.push(format!(
                    "multiple local Sessions; empty workspace opened; choose --session: {}",
                    local
                        .iter()
                        .map(|s| s.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            if local.len() == 1 {
                vec![(None, vec![(local[0].id.as_str(), None)])]
            } else {
                vec![]
            }
        };
        if cwd.is_some()
            && groups
                .iter()
                .flat_map(|(_, e)| e)
                .filter(|(id, _)| {
                    known
                        .get(id)
                        .is_some_and(|s| reference.is_some() || !inactive(s))
                })
                .count()
                > 1
        {
            return Err(invalid(
                "--cwd is ambiguous across a multi-Session arrangement; choose --session",
            ));
        }
        let mut prepared = Vec::new();
        let mut skipped = Vec::new();
        for (label, entries) in groups {
            let mut ready = Vec::new();
            for (id, saved) in entries {
                let reason = match known.get(id) {
                    None => Some("Session is not available on this host for this Stream"),
                    Some(s) if reference.is_none() && inactive(s) => {
                        Some("Session is done or parked; choose --session to restore explicitly")
                    }
                    Some(s)
                        if saved.is_some_and(|e| {
                            e.harness() != s.harness || e.native_id() != s.harness_session_id
                        }) =>
                    {
                        Some("saved arrangement native identity differs from catalog Session")
                    }
                    _ => None,
                };
                if let Some(reason) = reason {
                    skipped.push(SkippedSession {
                        session_id: id.into(),
                        reason: reason.into(),
                    });
                    continue;
                }
                let session = known[id];
                session.harness.parse::<Harness>().map_err(|_| {
                    invalid(format!("unsupported session harness {:?}", session.harness))
                })?;
                let entry: Result<_> = (|| {
                    let cwd = self.directory(stream, Some(session), cwd)?;
                    let command = SessionCommand::verified(
                        session,
                        saved.and_then(ArrangedSession::locator).map(Path::new),
                        self.roots,
                    )?;
                    Ok(PreparedSession {
                        session,
                        command,
                        cwd,
                    })
                })();
                match entry {
                    Ok(entry) => {
                        self.remember(stream, Some(session), &entry.cwd)?;
                        ready.push(entry);
                    }
                    Err(error) => skipped.push(SkippedSession {
                        session_id: id.into(),
                        reason: error.to_string(),
                    }),
                }
            }
            if !ready.is_empty() {
                prepared.push(PreparedTab {
                    label,
                    entries: ready,
                });
            }
        }
        Ok((prepared, skipped, attention))
    }
    pub fn resume(
        &self,
        stream: &Stream,
        sessions: &[Session],
        reference: Option<&str>,
        cwd: Option<&Path>,
    ) -> Result<ResumePermit<'a>> {
        let scope = self.frontend.map(BuiltinFrontend::scope);
        let guard = self.state.stream_lock(scope.as_deref(), &stream.id)?;
        let authority = authorize(self.backend, self.state.host_id(), &stream.id)?;
        let session = select_session(stream, sessions, self.state.host_id(), reference)?
            .ok_or_else(|| {
                invalid(format!(
                    "stream {:?} has no eligible local session to resume",
                    stream.id
                ))
            })?;
        if let Some(BuiltinFrontend::Herdr(client)) = self.frontend {
            guard.check_open()?;
            let topology = client.topology()?;
            if arrangements::verified_inventory(&topology, None)?
                .iter()
                .any(|a| {
                    a.harness.as_ref() == session.harness
                        && a.identity.id == session.harness_session_id
                })
            {
                return Err(invalid(
                    "conversation is already running in a native pane; refusing duplicate resume",
                ));
            }
        }
        let scoped = scope
            .as_deref()
            .map(|scope| self.state.arrangement(scope, &stream.id))
            .transpose()?
            .flatten();
        let plan = if scoped.is_some() {
            scoped
        } else {
            let plans: Vec<_> = self
                .state
                .arrangements()?
                .into_iter()
                .filter(|p| p.record().stream_id() == stream.id)
                .collect();
            if plans.len() > 1 {
                return Err(invalid(
                    "multiple saved arrangements across scopes; inspect runtime state before resume",
                ));
            }
            plans.into_iter().next().map(|p| p.record().clone())
        };
        let locator = plan
            .as_ref()
            .and_then(|p| {
                p.tabs().iter().flat_map(|t| t.sessions()).find(|e| {
                    e.session_id() == session.id
                        && e.harness() == session.harness
                        && e.native_id() == session.harness_session_id
                })
            })
            .and_then(ArrangedSession::locator)
            .map(Path::new);
        let command = SessionCommand::verified(session, locator, self.roots)?;
        let directory = self.directory(stream, Some(session), cwd)?;
        if cwd.is_some() {
            self.remember(stream, Some(session), &directory)?;
        }
        Ok(ResumePermit {
            _guard: guard,
            backend: self.backend,
            authority,
            stream_id: stream.id.clone(),
            command,
            cwd: directory,
        })
    }
}

struct PreparedTab<'a> {
    label: Option<String>,
    entries: Vec<PreparedSession<'a>>,
}
impl PreparedTab<'_> {
    fn manual_label(&self) -> Option<&str> {
        self.label
            .as_deref()
            .filter(|label| !label.trim().is_empty())
    }
}
struct PreparedSession<'a> {
    session: &'a Session,
    command: SessionCommand,
    cwd: PathBuf,
}
fn inactive(session: &Session) -> bool {
    matches!(session.status, Status::Done | Status::Parked)
}
fn require_live(
    live: &[arrangements::VerifiedAgent<'_>],
    workspace: &str,
    tab: &str,
    saved: &ArrangedSession,
) -> Result<()> {
    if live.iter().any(|agent| {
        agent.harness.as_ref() == saved.harness()
            && agent.identity.id == saved.native_id()
            && agent.native.workspace_id == workspace
            && agent.native.tab_id == tab
            && Some(agent.native.pane_id.as_str()) == saved.pane_id()
            && saved
                .terminal_id()
                .is_none_or(|id| id == agent.native.terminal_id)
    }) {
        Ok(())
    } else {
        Err(invalid(
            "started conversation is not live in its acknowledged native pane; refusing ready binding",
        ))
    }
}

/// Pin caller selectors rather than inherit a long-lived GUI server's environment.
/// Credentials and caller pane identities are intentionally excluded.
pub fn native_environment(
    stream: &str,
    caller: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let home = Path::new(
        caller
            .get("HOME")
            .ok_or_else(|| invalid("HOME must be supplied explicitly"))?,
    );
    if !home.is_absolute() {
        return Err(invalid("HOME must be absolute"));
    }
    let default = |path: &str| home.join(path).to_string_lossy().into_owned();
    let nonempty = |key: &str, fallback: String| {
        caller
            .get(key)
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or(fallback)
    };
    let provided = |key: &str, fallback: String| caller.get(key).cloned().unwrap_or(fallback);
    let mut values: BTreeMap<_, _> = [
        ("STREAMS_STREAM_ID", stream.to_owned()),
        ("HOME", home.to_string_lossy().into_owned()),
        (
            "XDG_CONFIG_HOME",
            nonempty("XDG_CONFIG_HOME", default(".config")),
        ),
        (
            "XDG_DATA_HOME",
            nonempty("XDG_DATA_HOME", default(".local/share")),
        ),
        (
            "XDG_STATE_HOME",
            nonempty("XDG_STATE_HOME", default(".local/state")),
        ),
        (
            "PI_CODING_AGENT_DIR",
            nonempty("PI_CODING_AGENT_DIR", default(".pi/agent")),
        ),
        ("CODEX_HOME", provided("CODEX_HOME", default(".codex"))),
        (
            "CLAUDE_CONFIG_DIR",
            provided("CLAUDE_CONFIG_DIR", default(".claude")),
        ),
        ("PATH", provided("PATH", "/bin:/usr/bin".into())),
        ("HERDR_BIN_PATH", nonempty("HERDR_BIN_PATH", "herdr".into())),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect();
    for key in ["HERDR_SOCKET_PATH", "PI_OFFLINE"] {
        if let Some(value) = caller.get(key) {
            values.insert(key.into(), value.clone());
        }
    }
    if values.values().any(|value| value.contains('\0')) {
        return Err(invalid("invalid launch environment"));
    }
    Ok(values)
}
fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| invalid("native cwd/locator must be UTF-8"))
}
fn client_scope(client: &CmuxClient) -> String {
    super::state::cmux_scope(&client.endpoint().socket)
}
fn authorize(backend: &dyn CatalogBackend, host: &str, stream: &str) -> Result<Authority> {
    let authority=backend.authority(stream).map_err(|e|invalid(e.to_string()))?.filter(|a|a.host_id==host).ok_or_else(||invalid("effective owner authority for this host is required; pending owner fields do not grant runtime authority"))?;
    super::text(&authority.host_id)?;
    super::text(&authority.token)?;
    Ok(authority)
}

/// Keep global stream exclusion through the exec boundary, not merely through planning.
pub struct ResumePermit<'a> {
    _guard: StreamGuard<'a>,
    backend: &'a dyn CatalogBackend,
    authority: Authority,
    stream_id: String,
    command: SessionCommand,
    cwd: PathBuf,
}
impl ResumePermit<'_> {
    pub fn command(&self) -> &SessionCommand {
        &self.command
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    pub fn exec(self) -> Result<std::convert::Infallible> {
        use std::os::unix::process::CommandExt;
        if authorize(self.backend, &self.authority.host_id, &self.stream_id)? != self.authority {
            return Err(invalid("effective owner authority changed before resume"));
        }
        let error = std::process::Command::new(&self.command.argv()[0])
            .args(&self.command.argv()[1..])
            .current_dir(&self.cwd)
            .env("STREAMS_STREAM_ID", &self.stream_id)
            .exec();
        Err(error.into())
    }
}
