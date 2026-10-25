//! One observation and direct effects. Subscription loss abandons its captured pass.
use super::files::Files;
use super::{Result, SharedOutput, Wake, send};
use crate::cli::{App, HerdrRead, Output, application, cards};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};
use streams_temper::{
    agent::{ContextRequest, EnrollmentRequest, RecordContext},
    cancellation::Cancellation,
    child_protocol::{Request, ServerFrame, UiFacts},
    frontend::herdr::{
        HerdrClient, HerdrNativeSnapshot,
        events::{EventFilter, EventKind, SubscriptionCancel, SubscriptionUpdate},
    },
    model::{Record, Stream},
    operations::membership::MoveIntent,
    runtime::{
        conversations::Harness,
        native_cards::{NativeCardEngine, NativeCardsConfig, OpenMode, PiOwner},
        native_markers,
    },
};

struct Pass {
    context: RecordContext,
    config: NativeCardsConfig,
    workspace_stream: Option<String>,
    transcript: Option<std::path::PathBuf>,
}
pub(super) struct Controller {
    pub(super) app: App,
    plugin_root: std::path::PathBuf,
    output: SharedOutput,
    session_id: String,
    sender: mpsc::Sender<Wake>,
    stopping: Cancellation,
    client: Option<HerdrClient>,
    owner: Option<PiOwner>,
    subscription: Option<SubscriptionCancel>,
    thread: Option<JoinHandle<()>>,
    files: Option<Files>,
    records: Option<Arc<application::OpenedRecords>>,
    enabled: bool,
    view: UiFacts,
}
impl Controller {
    pub fn new(
        app: App,
        session_id: String,
        plugin_root: std::path::PathBuf,
        sender: mpsc::Sender<Wake>,
        output: SharedOutput,
        stopping: Cancellation,
    ) -> Self {
        Self {
            app,
            session_id,
            plugin_root,
            output,
            sender,
            stopping,
            client: None,
            owner: None,
            subscription: None,
            thread: None,
            files: None,
            records: None,
            enabled: false,
            view: UiFacts {
                desired_pi_name: None,
            },
        }
    }
    fn publish_view(&mut self) -> Result<()> {
        send(
            &self.output,
            &ServerFrame::View {
                view: self.view.clone(),
            },
        )
    }
    fn config(&self) -> Result<NativeCardsConfig> {
        NativeCardsConfig::load(&self.app.environment, &self.app.cwd, &self.plugin_root)
            .map_err(Into::into)
    }
    fn ensure(&mut self, config: &NativeCardsConfig) -> Result<()> {
        if self.client.is_some() {
            return Ok(());
        }
        let client = HerdrClient::connect_cancellable(
            self.app.environment.clone(),
            config.timeout_seconds(),
            self.stopping.clone(),
        )
        .map_err(streams_temper::runtime::RuntimeError::from)?;
        if self.records.is_none() {
            let records = Arc::new(application::open_records(&self.app.config)?);
            self.files = Some(Files::new(&records, &self.session_id, self.sender.clone())?);
            self.records = Some(records);
        }
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
            .map_err(|error| crate::cli::CliError::message(error.to_string()))?;
        self.subscription = Some(subscription.cancellation_handle());
        let sender = self.sender.clone();
        self.thread = Some(
            thread::Builder::new()
                .name("streams-native-events".into())
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
    fn observe(&mut self, cancelled: &AtomicBool, reconcile: bool) -> Result<Pass> {
        let config = self.config()?;
        self.ensure(&config)?;
        if cancelled.load(Ordering::Acquire) {
            return Err(crate::cli::CliError::message("native request cancelled"));
        }
        self.files.as_mut().unwrap().prepare()?;
        let client = self
            .client
            .as_ref()
            .unwrap()
            .with_subscription(self.subscription.as_ref().unwrap());
        let mut snapshot = HerdrNativeSnapshot::parse(
            &client
                .session_snapshot()
                .map_err(streams_temper::runtime::RuntimeError::from)?,
        )
        .map_err(streams_temper::runtime::RuntimeError::from)?;
        let settings = self.app.config.load()?;
        let records = self.records.as_ref().unwrap();
        if settings.catalog_root != records.root() {
            return Err(crate::cli::CliError::message(
                "catalog root setting changed during native pass",
            ));
        }
        let mut context = self.app.context_from_records(
            records,
            ContextRequest {
                harness: Harness::Pi,
                native_id: &self.session_id,
                stream_id: None,
                environment_stream_id: None,
                workspace_stream_id: None,
            },
            settings.naming,
        )?;
        let owner = match &self.owner {
            Some(owner) => owner.clone(),
            None => {
                let locator = if let Some(session) = &context.session {
                    self.app
                        .locations(records.root(), records.host().as_str())?
                        .transcript_file(&session.stream_id, &session.id)?
                } else {
                    None
                };
                PiOwner::select_initial(
                    &snapshot,
                    self.app
                        .environment
                        .get("HERDR_PANE_ID")
                        .map(String::as_str),
                    &self.session_id,
                    locator.as_deref(),
                )?
            }
        };
        let (pane, transcript) = owner
            .observe(&snapshot)?
            .ok_or_else(|| crate::cli::CliError::message("Pi exited or changed conversation"))?;
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
        if cancelled.load(Ordering::Acquire) {
            return Err(crate::cli::CliError::message("native request cancelled"));
        }
        client
            .check_subscription()
            .map_err(streams_temper::runtime::RuntimeError::from)?;
        if reconcile && let Some(session) = &context.session {
            let destination = match &selected {
                Some(id) => id.clone(),
                None => {
                    let id = uuid::Uuid::new_v4().to_string();
                    let stream =
                        Stream::new(&id, "Untitled effort", None, records.host().as_str())?;
                    application::create_record(
                        records.records(),
                        &uuid::Uuid::new_v4().to_string(),
                        &Record::Stream(stream),
                    )?;
                    native_markers::associate_workspace(
                        &client,
                        &workspace,
                        records.source_id(),
                        records.host().as_str(),
                        &id,
                        cancelled,
                    )?;
                    snapshot
                        .apply_workspace_tokens(
                            &workspace,
                            native_markers::stream_tokens(
                                records.source_id(),
                                records.host().as_str(),
                                &id,
                            ),
                        )
                        .map_err(streams_temper::runtime::RuntimeError::from)?;
                    id
                }
            };
            if destination != session.stream_id {
                if cancelled.load(Ordering::Acquire) {
                    return Err(crate::cli::CliError::message("native request cancelled"));
                }
                client
                    .check_subscription()
                    .map_err(streams_temper::runtime::RuntimeError::from)?;
                let intent = MoveIntent::new(
                    records.host().as_str(),
                    "pi",
                    &self.session_id,
                    &destination,
                )?;
                let moved = application::move_session_observed(
                    records.records(),
                    &uuid::Uuid::new_v4().to_string(),
                    &intent,
                )?;
                let Record::Session(session) = moved.save.current.record else {
                    unreachable!("move saves Session")
                };
                let stream = moved.stream.ok_or_else(|| {
                    crate::cli::CliError::message("native move has no current Stream dependency")
                })?;
                context.session = Some(session.clone());
                context.sessions = vec![session];
                context.stream = Some(stream);
            }
        }
        self.files
            .as_mut()
            .unwrap()
            .set_stream(context.stream.as_ref().map(|stream| stream.id.as_str()))?;
        self.app.herdr = Some(HerdrRead {
            client,
            snapshot: Some(snapshot),
            owner: Some(owner),
        });
        self.view = UiFacts {
            desired_pi_name: config
                .session_naming()
                .then(|| {
                    context
                        .session
                        .as_ref()
                        .map(|session| session.title.clone())
                })
                .flatten(),
        };
        self.publish_view()?;
        Ok(Pass {
            context,
            config,
            workspace_stream: selected,
            transcript,
        })
    }
    fn engine(&self, pass: Pass, cancelled: Arc<AtomicBool>) -> Result<NativeCardEngine> {
        cards::build(
            self.app.clone(),
            cards::Options {
                session_id: Some(self.session_id.clone()),
                plugin_root: None,
                state_dir: None,
            },
            pass.config,
            cancelled,
            self.records.clone(),
            Some(pass.context),
        )
    }
    fn watch_engine(&self, engine: &NativeCardEngine) {
        if let Some(subscription) = &self.subscription {
            let (panes, tabs, workspaces) = engine.watched_ids();
            subscription.set_watched_ids(panes, tabs, workspaces);
        }
    }
    pub fn request(&mut self, request: &Request, cancelled: Arc<AtomicBool>) -> Result<Output> {
        self.enabled = true;
        if self.app.environment.get("HERDR_ENV").map(String::as_str) != Some("1") {
            if matches!(request, Request::OpenCard {} | Request::CloseCard {}) {
                return Err(crate::cli::CliError::message(
                    "native card effects require Herdr",
                ));
            }
            self.view = UiFacts {
                desired_pi_name: None,
            };
            self.publish_view()?;
            return Ok(Output::value(
                json!({"native":"inactive","reason":"outside_herdr"}),
            ));
        }
        let pass = self.observe(&cancelled, true)?;
        if pass.context.session.is_none() {
            if matches!(request, Request::OpenCard {} | Request::CloseCard {}) {
                return Err(crate::cli::CliError::message(
                    "native card effects require an enrolled Session",
                ));
            }
            return Ok(Output::value(
                json!({"native":"inactive","reason":"not_enrolled"}),
            ));
        }
        let mut engine = self.engine(pass, cancelled)?;
        let result = match request {
            Request::CloseCard {} => {
                engine.close_display()?;
                json!({"closed":true})
            }
            Request::OpenCard {} => {
                engine.project()?;
                json!({"display":cards::display_name(engine.open_display(OpenMode::Explicit)?)})
            }
            Request::SessionStarted {} => {
                engine.project()?;
                json!({"native":"active","display":cards::display_name(engine.open_display(OpenMode::Automatic)?)})
            }
            Request::Attach {} => {
                engine.project()?;
                json!({"native":"active"})
            }
            _ => unreachable!("native lifecycle request"),
        };
        self.watch_engine(&engine);
        Ok(Output::value(result))
    }
    fn project(&mut self, cancelled: Arc<AtomicBool>) -> Result<()> {
        if !self.enabled || self.app.environment.get("HERDR_ENV").map(String::as_str) != Some("1") {
            return Ok(());
        }
        let pass = self.observe(&cancelled, true)?;
        if pass.context.session.is_some() {
            let mut engine = self.engine(pass, cancelled)?;
            engine.project()?;
            self.watch_engine(&engine);
        }
        Ok(())
    }
    pub fn update(
        &mut self,
        update: std::result::Result<SubscriptionUpdate, String>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        if let Err(error) = update {
            self.stop();
            return Err(crate::cli::CliError::message(error));
        }
        self.project(cancelled)
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
            Ok(event) if !files.relevant(&event) => return Ok(()),
            Ok(_) => {}
            Err(_) => files.recover()?,
        }
        if !files.prepare()? {
            return Ok(());
        }
        self.project(cancelled)
    }
    pub fn saved(&mut self, cancelled: Arc<AtomicBool>) -> Result<()> {
        self.project(cancelled)
    }
    pub fn enroll(
        &mut self,
        fallback: &App,
        request: &Value,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Output> {
        if !self.enabled || self.app.environment.get("HERDR_ENV").map(String::as_str) != Some("1") {
            return fallback.enroll_request(&request.to_string());
        }
        let mut pass = self.observe(&cancelled, false)?;
        let records = self.records.as_ref().unwrap().clone();
        let request = EnrollmentRequest::parse(&request.to_string(), records.source_id())?;
        let target = request
            .stream_id()
            .map(str::to_owned)
            .or_else(|| {
                request.stream_proposal().map(|_| {
                    streams_temper::agent::effort_id_for(
                        records.host().as_str(),
                        request.harness(),
                        request.harness_session_id(),
                    )
                })
            })
            .or_else(|| {
                pass.context
                    .session
                    .as_ref()
                    .map(|session| session.stream_id.clone())
            });
        if pass
            .workspace_stream
            .as_ref()
            .is_some_and(|selected| target.as_ref() != Some(selected))
        {
            return Err(crate::cli::CliError::message(
                "workspace names a different Stream; captured enrollment target was not changed",
            ));
        }
        let saved = self.app.enroll_records(&records, &request, &pass.context)?;
        let mut output = saved.output;
        if saved.directory_selected
            && let (Some(session), Some(stream)) = (saved.session, saved.stream)
        {
            let completion = (|| -> Result<()> {
                if let Some(path) = &pass.transcript {
                    self.app
                        .locations(records.root(), records.host().as_str())?
                        .remember_transcript(&stream.id, &session.id, path)?;
                }
                pass.context.session = Some(session.clone());
                pass.context.sessions = vec![session];
                pass.context.stream = Some(stream);
                self.view.desired_pi_name = pass
                    .config
                    .session_naming()
                    .then(|| pass.context.session.as_ref().unwrap().title.clone());
                self.publish_view()?;
                let mut engine = self.engine(pass, cancelled)?;
                engine.project()?;
                self.watch_engine(&engine);
                Ok(())
            })();
            if let Err(error) = completion {
                output.notices.push(format!("native completion: {error}"));
            }
        }
        Ok(output)
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
