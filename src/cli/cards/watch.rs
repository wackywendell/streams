//! Classic Herdr card controller: watches Herdr events, record files and stdin EOF.
mod files;
#[cfg(test)]
mod tests;

use files::Files;
use serde_json::{Map, Value};
use std::{
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use streams::{
    cancellation::Cancellation,
    frontend::{
        FrontendId,
        herdr::{
            HerdrClient, HerdrNativeSnapshot,
            events::{EventFilter, EventKind, SubscriptionCancel, SubscriptionUpdate},
        },
    },
    model::{Session, Stream, Title},
    operations::edits::EditRequest,
    runtime::{
        card_settings::ConversationId,
        native_cards::{LifecycleEvent, NativeCardEngine, NativeCardsConfig, PiOwner},
        native_markers,
    },
    storage::records::{RecordAddress, RecordStore},
};
use uuid::Uuid;

use crate::cli::{App, CliError, HerdrRead, Result, application, cards};

const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

pub(super) enum Wake {
    InputFinished,
    Files(u8, notify::Result<notify::Event>),
    Native(std::result::Result<SubscriptionUpdate, String>),
}

#[derive(Default, PartialEq)]
enum Binding {
    #[default]
    Unobserved,
    Unbound,
    Session(String),
}

enum FileBaseline {
    Required,
    Prepared,
}

struct Pass {
    newly_bound: bool,
    records: Arc<application::OpenedRecords>,
    pair: Option<(Stream, Session)>,
    config: NativeCardsConfig,
}

pub(super) struct Controller {
    pub(super) app: App,
    plugin_root: PathBuf,
    session_id: String,
    sender: mpsc::Sender<Wake>,
    stopping: Cancellation,
    client: Option<HerdrClient>,
    owner: Option<PiOwner>,
    subscription: Option<SubscriptionCancel>,
    thread: Option<JoinHandle<()>>,
    files: Option<Files>,
    catalog_source: Option<String>,
    binding: Binding,
    last_workspace: Option<String>,
}

impl Controller {
    pub fn new(
        app: App,
        session_id: String,
        plugin_root: PathBuf,
        sender: mpsc::Sender<Wake>,
        stopping: Cancellation,
    ) -> Self {
        Self {
            app,
            plugin_root,
            session_id,
            sender,
            stopping,
            client: None,
            owner: None,
            subscription: None,
            thread: None,
            files: None,
            catalog_source: None,
            binding: Binding::Unobserved,
            last_workspace: None,
        }
    }

    fn ensure_client(&mut self, config: &NativeCardsConfig) -> Result<()> {
        if self.client.is_some() {
            return Ok(());
        }
        let client = HerdrClient::connect_cancellable(
            self.app.environment.clone(),
            config.timeout_seconds(),
            self.stopping.clone(),
        )
        .map_err(streams::runtime::RuntimeError::from)?;
        let filter = EventFilter {
            kinds: [
                EventKind::PaneMoved,
                EventKind::PaneAgentDetected,
                EventKind::PaneUpdated,
                EventKind::TabMoved,
                EventKind::PaneExited,
                EventKind::PaneClosed,
                EventKind::TabClosed,
                EventKind::WorkspaceClosed,
                EventKind::WorkspaceMetadataUpdated,
            ]
            .into(),
            pane_ids: self
                .app
                .environment
                .get("HERDR_PANE_ID")
                .cloned()
                .into_iter()
                .collect(),
            terminal_ids: self
                .owner
                .as_ref()
                .map(|owner| owner.terminal_id.clone())
                .into_iter()
                .collect(),
            ..EventFilter::default()
        };
        let mut subscription = client
            .socket()
            .subscribe(filter)
            .map_err(|error| CliError::message(error.to_string()))?;
        self.subscription = Some(subscription.cancellation_handle());
        let sender = self.sender.clone();
        self.thread = Some(
            thread::Builder::new()
                .name("streams-card-watch-events".into())
                .spawn(move || {
                    loop {
                        let update = subscription.receive().map_err(|error| error.to_string());
                        let failed = update.is_err();
                        if sender.send(Wake::Native(update)).is_err() || failed {
                            break;
                        }
                    }
                })?,
        );
        self.client = Some(client);
        Ok(())
    }

    /// Keep file watches bound to the currently opened catalog source.
    fn ensure_catalog(&mut self, records: &Arc<application::OpenedRecords>) -> Result<bool> {
        if self.catalog_source.as_deref() == Some(records.source_id()) && self.files.is_some() {
            return Ok(false);
        }
        self.files = None;
        self.catalog_source = None;
        self.binding = Binding::Unobserved;
        let files = Files::new(records, &self.session_id, self.sender.clone())?;
        self.files = Some(files);
        self.catalog_source = Some(records.source_id().to_owned());
        Ok(true)
    }

    fn read_pair(&self, records: &application::OpenedRecords) -> Result<Option<(Stream, Session)>> {
        let host = records.host().as_str();
        let found = records
            .records()
            .with_reader(|reader| -> application::Result<_> {
                let Some(session) = reader.read_session(host, "pi", &self.session_id)? else {
                    return Ok(None);
                };
                let Some(stream) = reader.read_stream(host, &session.stream_id)? else {
                    return Err(application::ApplicationError::not_found(format!(
                        "no Stream {host}/{}",
                        session.stream_id
                    )));
                };
                Ok(Some((stream, session)))
            })?;
        Ok(found)
    }

    /// Move an enrolled Session when its pane moves into another Herdr workspace,
    /// calling the same `edit_session` with `before` holding the old `stream_id`.
    fn sync_workspace_move(
        &mut self,
        records: &application::OpenedRecords,
        workspace: &str,
        selected: Option<&str>,
        pair: Option<(Stream, Session)>,
    ) -> Result<Option<(Stream, Session)>> {
        let workspace_changed = self
            .last_workspace
            .as_deref()
            .is_some_and(|prev| prev != workspace);
        self.last_workspace = Some(workspace.to_owned());
        if !workspace_changed {
            return Ok(pair);
        }
        let Some((_stream, session)) = pair else {
            return Ok(None);
        };
        let host = records.host().as_str();
        let dest_stream_id = match selected {
            Some(id) if id == session.stream_id => return self.read_pair(records),
            Some(id) => id.to_owned(),
            None => {
                let new_id = Uuid::new_v4().to_string();
                let new_stream =
                    Stream::new(host, &new_id, Title::new(&session.title.text, false), None)?;
                application::create_stream(records.records(), &new_stream)?;
                new_id
            }
        };
        let mut changes = Map::new();
        changes.insert("stream_id".into(), Value::String(dest_stream_id));
        let mut before = Map::new();
        before.insert("stream_id".into(), Value::String(session.stream_id.clone()));
        let target = RecordAddress::for_session(&session);
        let _ = application::edit_session(
            records.records(),
            &target,
            &EditRequest {
                changes,
                before: Some(before),
            },
        )?;
        self.read_pair(records)
    }

    fn observe_opened(
        &mut self,
        records: Arc<application::OpenedRecords>,
        cancelled: &AtomicBool,
        baseline: FileBaseline,
    ) -> Result<Pass> {
        let settings = records.settings();
        let mut config = NativeCardsConfig::from_settings(
            settings,
            self.app.config.config_path(),
            &self.app.cwd,
            &self.plugin_root,
        )?;
        self.ensure_client(&config)?;
        let rebuilt_catalog = self.ensure_catalog(&records)?;
        if cancelled.load(Ordering::Acquire) {
            return Err(CliError::message("card watch cancelled"));
        }
        if !rebuilt_catalog && matches!(baseline, FileBaseline::Required) {
            self.files.as_mut().unwrap().prepare()?;
        }
        let client = self
            .client
            .as_ref()
            .unwrap()
            .with_subscription(self.subscription.as_ref().unwrap());
        let snapshot = HerdrNativeSnapshot::parse(
            &client
                .session_snapshot()
                .map_err(streams::runtime::RuntimeError::from)?,
        )
        .map_err(streams::runtime::RuntimeError::from)?;
        config = config.with_auto_open(
            self.app
                .conversation_card_settings(
                    &records,
                    &ConversationId {
                        harness: "pi".into(),
                        native_id: self.session_id.clone(),
                    },
                )?
                .mode()?
                .resolve(settings.herdr.cards_auto_open),
        );
        let pair = self.read_pair(&records)?;
        if self.binding == Binding::Unobserved {
            self.observe_binding(pair.as_ref().map(|(_, s)| s));
        }
        let owner = match &self.owner {
            Some(owner) => owner.clone(),
            None => PiOwner::select_initial(
                &snapshot,
                self.app
                    .environment
                    .get("HERDR_PANE_ID")
                    .map(String::as_str),
                &self.session_id,
                None,
            )?,
        };
        let (pane, _transcript) = owner
            .observe(&snapshot)?
            .ok_or_else(|| CliError::message("Pi exited or changed conversation"))?;
        let workspace = pane.pane.workspace_id.clone();
        self.owner = Some(owner.clone());
        self.subscription.as_ref().unwrap().set_watched_ids(
            [pane.pane.id.clone()].into(),
            [pane.pane.tab_id.clone()].into(),
            [workspace.clone()].into(),
        );
        let selected = native_markers::selected_stream(
            &snapshot,
            &workspace,
            records.source_id(),
            records.host().as_str(),
        )?;
        let pair = self.sync_workspace_move(&records, &workspace, selected.as_deref(), pair)?;
        if cancelled.load(Ordering::Acquire) {
            return Err(CliError::message("card watch cancelled"));
        }
        client
            .check_subscription()
            .map_err(streams::runtime::RuntimeError::from)?;
        self.files
            .as_mut()
            .unwrap()
            .set_stream(pair.as_ref().map(|(stream, _)| stream.id.as_str()))?;
        self.app.herdr = Some(HerdrRead {
            client,
            snapshot: Some(snapshot),
            owner: Some(owner),
        });
        let newly_bound = self.observe_binding(pair.as_ref().map(|(_, s)| s));
        Ok(Pass {
            newly_bound,
            records,
            pair,
            config,
        })
    }

    fn observe_binding(&mut self, session: Option<&Session>) -> bool {
        let binding = session.map_or(Binding::Unbound, |s| Binding::Session(s.id.clone()));
        let newly_bound = self.binding != Binding::Unobserved
            && self.binding != binding
            && matches!(binding, Binding::Session(_));
        self.binding = binding;
        newly_bound
    }

    fn engine(&self, pass: Pass, cancelled: Arc<AtomicBool>) -> Result<NativeCardEngine> {
        cards::build(
            self.app.clone(),
            cards::Options {
                session_id: Some(self.session_id.clone()),
                state_dir: None,
            },
            pass.config,
            cancelled,
            Some(pass.records),
            pass.pair,
        )
    }

    fn watch_engine(&self, engine: &NativeCardEngine) {
        if let Some(subscription) = &self.subscription {
            let (panes, tabs, workspaces) = engine.watched_ids();
            subscription.set_watched_ids(panes, tabs, workspaces);
        }
    }

    fn start(
        &mut self,
        records: Arc<application::OpenedRecords>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let pass = self.observe_opened(records, &cancelled, FileBaseline::Required)?;
        if pass.pair.is_some() {
            let mut engine = self.engine(pass, cancelled)?;
            engine.lifecycle(LifecycleEvent::Started)?;
            self.watch_engine(&engine);
        }
        Ok(())
    }

    fn project(&mut self, cancelled: Arc<AtomicBool>, baseline: FileBaseline) -> Result<()> {
        let records = Arc::new(application::open_records(&self.app.config)?);
        if !self
            .app
            .selection(records.settings().frontend.clone(), None)
            .is_ok_and(|selection| selection.resolved == Some(FrontendId::Herdr))
        {
            return Ok(());
        }
        let pass = self.observe_opened(records, &cancelled, baseline)?;
        if pass.pair.is_some() {
            let newly_bound = pass.newly_bound;
            let mut engine = self.engine(pass, cancelled)?;
            if newly_bound {
                engine.lifecycle(LifecycleEvent::NewlyBound)?;
            } else {
                engine.project()?;
            }
            self.watch_engine(&engine);
        }
        Ok(())
    }

    pub fn update(
        &mut self,
        update: std::result::Result<SubscriptionUpdate, String>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        match update {
            Err(error) => {
                self.stop();
                Err(CliError::message(error))
            }
            Ok(SubscriptionUpdate::Event(_) | SubscriptionUpdate::Resubscribed) => {
                self.project(cancelled, FileBaseline::Required)
            }
        }
    }

    pub fn file_event(
        &mut self,
        generation: u8,
        event: notify::Result<notify::Event>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let Some(files) = self.files.as_mut() else {
            return Ok(());
        };
        if !files.current(generation) {
            return Ok(());
        }
        match event {
            Ok(event) if event.need_rescan() => files.recover()?,
            Ok(event) if !files.relevant(&event) => return Ok(()),
            Ok(_) => {}
            Err(_) => files.recover()?,
        }
        if !files.prepare()? {
            return Ok(());
        }
        self.project(cancelled, FileBaseline::Prepared)
    }

    /// Close the companion card when Pi exits (stdin EOF), without changing card mode.
    fn close_on_exit(&mut self, deadline: Instant) {
        if self.client.is_none() {
            return;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let Ok(records) = application::open_records(&self.app.config) else {
            return;
        };
        let records = Arc::new(records);
        let Ok(pass) = self.observe_opened(records, &cancelled, FileBaseline::Required) else {
            return;
        };
        if pass.pair.is_none() {
            return;
        }
        if let Ok(mut engine) = self.engine(pass, cancelled) {
            let _ = engine.close_display();
        }
    }

    pub fn stop(&mut self) {
        if let Some(subscription) = &self.subscription {
            let _ = subscription.cancel();
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.subscription = None;
        self.client = None;
        self.app.herdr = None;
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn run(app: App, session_id: String, plugin_root: PathBuf) -> Result<()> {
    streams::model::check_token(&session_id, "Session ID")
        .map_err(|e| CliError::InvalidRef(e.to_string()))?;
    let records = Arc::new(application::open_records(&app.config)?);
    if app
        .selection(records.settings().frontend.clone(), None)?
        .resolved
        != Some(FrontendId::Herdr)
    {
        return Ok(());
    }
    let (sender, receiver) = mpsc::channel();
    let stopping = Cancellation::new()?;
    let cancelled = stopping.flag();
    let input_sender = sender.clone();
    thread::Builder::new()
        .name("streams-card-watch-stdin".into())
        .spawn(move || {
            let mut stdin = std::io::stdin().lock();
            let mut buffer = [0u8; 256];
            while stdin.read(&mut buffer).is_ok_and(|read| read > 0) {}
            let _ = input_sender.send(Wake::InputFinished);
        })?;
    let mut controller = Controller::new(app, session_id, plugin_root, sender, stopping.clone());
    if let Err(error) = controller.start(records, cancelled.clone()) {
        if controller.client.is_none() {
            return Err(error);
        }
        eprintln!("streams card watch: {error}");
    }
    while let Ok(wake) = receiver.recv() {
        match wake {
            Wake::InputFinished => {
                controller.close_on_exit(Instant::now() + SHUTDOWN_GRACE);
                break;
            }
            Wake::Files(generation, event) => {
                if let Err(error) = controller.file_event(generation, event, cancelled.clone()) {
                    eprintln!("streams card watch: {error}");
                }
            }
            Wake::Native(update) => {
                let failed = update.is_err();
                if let Err(error) = controller.update(update, cancelled.clone()) {
                    eprintln!("streams card watch: {error}");
                }
                if failed {
                    break;
                }
            }
        }
    }
    let _ = stopping.cancel();
    controller.stop();
    Ok(())
}
