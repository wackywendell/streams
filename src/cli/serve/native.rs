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
use streams::{
    agent::{ContextRequest, EnrollmentRequest, RecordContext},
    cancellation::Cancellation,
    child_protocol::{Request, ServerFrame, UiFacts},
    frontend::{
        FrontendId,
        herdr::{
            HerdrClient, HerdrNativeSnapshot,
            events::{EventFilter, EventKind, SubscriptionCancel, SubscriptionUpdate},
        },
    },
    runtime::{
        conversations::Harness,
        native_cards::{LifecycleEvent, NativeCardEngine, NativeCardsConfig, OpenMode, PiOwner},
        native_markers,
    },
};

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
    context: RecordContext,
    config: NativeCardsConfig,
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
    control: Option<super::control::Control>,
    catalog_source: Option<String>,
    enabled: bool,
    binding: Binding,
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
            control: None,
            catalog_source: None,
            enabled: false,
            binding: Binding::Unobserved,
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
    /// Keep file watches and card-control socket bound to the currently opened catalog source.
    fn ensure_catalog(&mut self, records: &Arc<application::OpenedRecords>) -> Result<bool> {
        if self.catalog_source.as_deref() == Some(records.source_id())
            && self.files.is_some()
            && self.control.is_some()
        {
            return Ok(false);
        }
        self.files = None;
        self.control = None;
        self.catalog_source = None;
        self.binding = Binding::Unobserved;
        self.app.environment.remove("STREAMS_CARD_CONTROL_SOCKET");
        let files = Files::new(records, &self.session_id, self.sender.clone())?;
        let preferences = self.app.conversation_card_settings(
            records,
            &streams::integration::ConversationId {
                harness: "pi".into(),
                native_id: self.session_id.clone(),
            },
        )?;
        let control = super::control::Control::start(preferences.path(), self.sender.clone())?;
        self.app.environment.insert(
            "STREAMS_CARD_CONTROL_SOCKET".into(),
            control.path().to_string_lossy().into_owned(),
        );
        self.files = Some(files);
        self.control = Some(control);
        self.catalog_source = Some(records.source_id().to_owned());
        Ok(true)
    }
    fn observe(&mut self, cancelled: &AtomicBool, baseline: FileBaseline) -> Result<Pass> {
        let records = Arc::new(application::open_records(&self.app.config)?);
        self.observe_opened(records, cancelled, baseline)
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
            return Err(crate::cli::CliError::message("native request cancelled"));
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
                    &streams::integration::ConversationId {
                        harness: "pi".into(),
                        native_id: self.session_id.clone(),
                    },
                )?
                .mode()?
                .resolve(settings.herdr.cards_auto_open),
        );
        let session_id = self.session_id.clone();
        let environment_stream_id = self.app.environment.get("STREAMS_STREAM_ID").cloned();
        let request = ContextRequest {
            harness: Harness::Pi,
            native_id: &session_id,
            stream_id: None,
            environment_stream_id: environment_stream_id.as_deref(),
            workspace_stream_id: None,
        };
        let context = self
            .app
            .context_from_records(&records, request, settings.naming)?;
        // Establish the catalog baseline even if Herdr has not detected Pi yet.
        // Later binding changes are observed only after native observation succeeds.
        if self.binding == Binding::Unobserved {
            self.observe_binding(&context);
        }
        let owner = match &self.owner {
            Some(owner) => owner.clone(),
            None => {
                let locator = if let Some(session) = &context.session {
                    records.locations()?.transcript_file(&session.id)?
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
        let context = streams::application::with_workspace_stream(
            context,
            request,
            || selected.clone(),
            |request| {
                self.app
                    .context_from_records(&records, request, settings.naming)
            },
        )?;
        if cancelled.load(Ordering::Acquire) {
            return Err(crate::cli::CliError::message("native request cancelled"));
        }
        client
            .check_subscription()
            .map_err(streams::runtime::RuntimeError::from)?;
        self.files
            .as_mut()
            .unwrap()
            .set_stream(context.stream.as_ref().map(|stream| stream.id.as_str()))?;
        self.app.herdr = Some(HerdrRead {
            client,
            snapshot: Some(snapshot),
            owner: Some(owner),
            workspace_stream: selected,
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
        let newly_bound = self.observe_binding(&context);
        Ok(Pass {
            newly_bound,
            records,
            context,
            config,
            transcript,
        })
    }
    fn observe_binding(&mut self, context: &RecordContext) -> bool {
        let binding = context
            .session
            .as_ref()
            .map_or(Binding::Unbound, |session| {
                Binding::Session(session.id.clone())
            });
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
                plugin_root: None,
                state_dir: None,
            },
            pass.config,
            cancelled,
            Some(pass.records),
            Some(pass.context),
        )
    }
    fn watch_engine(&self, engine: &NativeCardEngine) {
        if let Some(subscription) = &self.subscription {
            let (panes, tabs, workspaces) = engine.watched_ids();
            subscription.set_watched_ids(panes, tabs, workspaces);
        }
    }
    pub fn request(
        &mut self,
        request: &Request,
        frontend: Option<FrontendId>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Output> {
        use streams::{child_protocol::CardTarget, config::CardMode};
        let is_herdr = frontend == Some(FrontendId::Herdr);
        let mode = match request {
            Request::OpenCard {} => Some(CardMode::Automatic),
            Request::CloseCard {} => Some(CardMode::Manual),
            _ => None,
        }
        .filter(|_| is_herdr);
        if let Some(mode) = mode {
            if cancelled.load(Ordering::Acquire) {
                return Err(crate::cli::CliError::message("native request cancelled"));
            }
            self.app
                .set_card_mode(&self.session_id, CardTarget::Session, mode)?;
        }
        self.request_native(request, is_herdr, cancelled)
            .map_err(|error| {
                if let Some(mode) = mode {
                    crate::cli::CliError::message(format!(
                        "Card preference saved as {}; native action failed: {error}",
                        serde_json::to_string(&mode).unwrap()
                    ))
                } else {
                    error
                }
            })
    }
    fn request_native(
        &mut self,
        request: &Request,
        is_herdr: bool,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Output> {
        self.enabled = true;
        if !is_herdr {
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
        let pass = self.observe(&cancelled, FileBaseline::Required)?;
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
        let newly_bound = pass.newly_bound;
        let mut engine = self.engine(pass, cancelled.clone())?;
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
                let report = engine.lifecycle(LifecycleEvent::Started)?;
                json!({"native":"active","display":report.projection.companion})
            }
            Request::Attach {} => {
                engine.lifecycle(if newly_bound {
                    LifecycleEvent::NewlyBound
                } else {
                    LifecycleEvent::Reattached
                })?;
                json!({"native":"active"})
            }
            _ => unreachable!("native lifecycle request"),
        };
        self.watch_engine(&engine);
        Ok(Output::value(result))
    }
    fn project(&mut self, cancelled: Arc<AtomicBool>, baseline: FileBaseline) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let records = Arc::new(application::open_records(&self.app.config)?);
        if !self
            .app
            .selection(records.settings().frontend.clone(), None)
            .is_ok_and(|selection| selection.resolved == Some(FrontendId::Herdr))
        {
            return Ok(());
        }
        let pass = self.observe_opened(records, &cancelled, baseline)?;
        if pass.context.session.is_some() {
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
                Err(crate::cli::CliError::message(error))
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
    pub fn saved(&mut self, cancelled: Arc<AtomicBool>) -> Result<()> {
        self.project(cancelled, FileBaseline::Required)
    }
    pub fn enroll(
        &mut self,
        fallback: &App,
        request: Value,
        frontend: Option<FrontendId>,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Output> {
        if !self.enabled || frontend != Some(FrontendId::Herdr) {
            return fallback.enroll_request(request);
        }
        let mut pass = self.observe(&cancelled, FileBaseline::Required)?;
        let records = pass.records.clone();
        let request = EnrollmentRequest::parse(request, records.source_id())?;
        let target = request
            .stream_id()
            .map(str::to_owned)
            .or_else(|| {
                request.stream_proposal().map(|_| {
                    streams::agent::effort_id_for(
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
        // An enrolled Session keeps its Stream wherever its pane is.
        if pass.context.session.is_none()
            && self
                .app
                .herdr
                .as_ref()
                .and_then(|read| read.workspace_stream.as_ref())
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
                    records
                        .locations()?
                        .remember_transcript(&session.id, path)?;
                }
                pass.context.session = Some(session);
                let stream_id = stream.id.clone();
                pass.context.stream = Some(stream);
                self.view.desired_pi_name = pass
                    .config
                    .session_naming()
                    .then(|| pass.context.session.as_ref().unwrap().title.clone());
                self.publish_view()?;
                let newly_bound = self.observe_binding(&pass.context);
                let mut engine = self.engine(pass, cancelled)?;
                engine.project()?;
                if newly_bound {
                    engine.open_display(OpenMode::Automatic)?;
                }
                self.app.herdr.as_mut().unwrap().workspace_stream = Some(stream_id);
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

#[cfg(test)]
mod tests;
