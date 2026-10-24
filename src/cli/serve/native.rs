use std::sync::{Arc, atomic::AtomicBool, mpsc};
use std::thread;

use streams_temper::{
    child_protocol::{Request, ServerFrame, UiFacts},
    frontend::herdr::{
        HerdrClient, HerdrNativeSnapshot,
        events::{HerdrEvent, SubscriptionCancel, SubscriptionUpdate},
    },
    runtime::{
        native::PiCallerObservation,
        native_cards::{DisplayOutcome, NativeCardsConfig, OpenMode},
    },
};

use super::super::{
    App, CliError, HerdrRead, NativeObservation, Output, PathBuf, Result, cards, json,
};
use super::{
    Wake, files,
    scope::{Owner, Sample},
};

pub(super) enum Trigger<'a> {
    Refresh,
    Native(&'a HerdrEvent),
    Resubscribed,
}

pub(super) struct Controller {
    pub enabled: bool,
    session_id: String,
    plugin_root: PathBuf,
    owner: Option<Owner>,
    pub app: Option<App>,
    files: Option<files::Files>,
    sender: mpsc::Sender<Wake>,
    output: super::SharedOutput,
    subscription: Option<(SubscriptionCancel, thread::JoinHandle<()>)>,
}

impl Controller {
    pub fn new(
        session_id: String,
        plugin_root: PathBuf,
        sender: mpsc::Sender<Wake>,
        output: super::SharedOutput,
    ) -> Self {
        Self {
            enabled: false,
            session_id,
            plugin_root,
            owner: None,
            app: None,
            files: None,
            sender,
            output,
            subscription: None,
        }
    }

    fn ensure(&mut self, app: App) -> Result<()> {
        if self.app.is_none() {
            self.owner = Some(Owner::inherited(&app.environment));
            self.app = Some(app);
        }
        if self.subscription.is_some() {
            return Ok(());
        }
        let app = self.app.as_mut().expect("native application");
        let config = NativeCardsConfig::load(&app.environment, &app.cwd, &self.plugin_root)?;
        let client = HerdrClient::connect(app.environment.clone(), config.timeout_seconds())
            .map_err(|error| CliError::message(error.to_string()))?;
        let owner = self.owner.as_ref().expect("native owner");
        let mut subscription = client
            .socket()
            .subscribe(owner.filter())
            .map_err(|error| CliError::message(error.to_string()))?;
        let cancellation = subscription.cancellation_handle();
        let sender = self.sender.clone();
        let handle = thread::spawn(move || {
            loop {
                let update = subscription.receive().map_err(|error| error.to_string());
                let failed = update.is_err();
                let (reply, receiver) = mpsc::sync_channel(0);
                if sender.send(Wake::Native(update, reply)).is_err() || failed {
                    break;
                }
                let Ok(owner) = receiver.recv() else {
                    break;
                };
                let filter = owner.filter();
                subscription.set_watched_ids(filter.pane_ids, filter.tab_ids, filter.workspace_ids);
            }
        });
        app.herdr = Some(HerdrRead {
            client,
            snapshot: None,
            terminal: None,
            refusal: None,
        });
        self.subscription = Some((cancellation, handle));
        Ok(())
    }

    fn observe(&mut self, trigger: Trigger<'_>) -> Result<bool> {
        let app = self.app.as_mut().expect("enabled native application");
        let previous = self.owner.as_ref().expect("native owner");
        if let Some(reason) = &previous.refusal {
            return Err(CliError::NativeContextChanged(reason.clone()));
        }
        let mut target = match trigger {
            Trigger::Native(event) => match previous.event_target(event) {
                Some(target) => target,
                None => return Ok(true),
            },
            _ => previous.clone(),
        };
        let read = app.herdr.as_ref().expect("native connection");
        let snapshot = HerdrNativeSnapshot::parse(
            &read
                .client
                .session_snapshot()
                .map_err(|error| CliError::message(error.to_string()))?,
        )
        .map_err(|error| CliError::message(error.to_string()))?;
        let pane = if matches!(trigger, Trigger::Resubscribed)
            && let Some(terminal) = &previous.terminal
        {
            snapshot
                .topology()
                .panes
                .iter()
                .find(|pane| &pane.terminal_id == terminal)
        } else {
            snapshot
                .topology()
                .panes
                .iter()
                .find(|pane| pane.pane.id == target.pane)
        };
        let mut sample = pane.map(|pane| Sample {
            pane: pane.pane.id.clone(),
            tab: pane.pane.tab_id.clone(),
            workspace: pane.pane.workspace_id.clone(),
            terminal: pane.terminal_id.clone(),
            session: None,
        });
        if matches!(trigger, Trigger::Resubscribed)
            && previous.terminal.is_some()
            && let Some(sample) = &sample
        {
            target.pane = sample.pane.clone();
            target.tab = sample.tab.clone();
            target.workspace = sample.workspace.clone();
        }
        if let Err(reason) = previous.confirm_terminal(&target, sample.as_ref()) {
            self.owner.as_mut().expect("native owner").refusal = Some(reason.clone());
            return Err(CliError::NativeContextChanged(reason));
        }
        let mut environment = app.environment.clone();
        target.apply(&mut environment);
        let client = read
            .client
            .with_context(environment)
            .map_err(|error| CliError::message(error.to_string()))?;
        let proof = match PiCallerObservation::from_snapshot(&client, snapshot, &self.session_id) {
            Ok(proof) => proof,
            Err(error) => {
                let reason = error.to_string();
                self.owner.as_mut().expect("native owner").refusal = Some(reason.clone());
                return Err(CliError::NativeContextChanged(reason));
            }
        };
        // The constructor has now resolved and verified exactly this Pi identity.
        sample.as_mut().expect("confirmed owner").session = Some(self.session_id.clone());
        let owner = previous
            .confirm(&target, sample.as_ref(), &self.session_id)
            .map_err(CliError::NativeContextChanged)?;
        owner.apply(&mut app.environment);
        app.herdr = Some(HerdrRead {
            client: client
                .with_context(app.environment.clone())
                .map_err(|error| CliError::message(error.to_string()))?,
            snapshot: None,
            terminal: owner.terminal.clone(),
            refusal: None,
        });
        self.owner = Some(owner);
        if self.files.is_none() {
            let mut files = files::Files::new(app, &self.session_id, self.sender.clone())?;
            files.discover(app, &self.session_id)?;
            self.files = Some(files);
        }
        self.files
            .as_mut()
            .expect("native record watches")
            .prepare(Some(proof.caller().workspace_id()))?;
        let observed = match app.agent_observation_from_native(
            streams_temper::runtime::conversations::Harness::Pi,
            &self.session_id,
            None,
            true,
            false,
            Some(NativeObservation::Pi(&proof)),
        ) {
            Ok(observed) => observed,
            Err(CliError::NotEnrolled) => {
                super::send(
                    &self.output,
                    &ServerFrame::View {
                        view: UiFacts {
                            desired_pi_name: None,
                        },
                    },
                )?;
                return Ok(false);
            }
            Err(error) => return Err(error),
        };
        let config = NativeCardsConfig::load(&app.environment, &app.cwd, &self.plugin_root)?;
        let desired_pi_name = config.session_naming().then(|| {
            observed
                .context
                .session
                .as_ref()
                .expect("attested Session")
                .title
                .clone()
        });
        super::send(
            &self.output,
            &ServerFrame::View {
                view: UiFacts { desired_pi_name },
            },
        )?;
        Ok(true)
    }

    fn engine(
        &self,
        cancelled: Arc<AtomicBool>,
    ) -> Result<streams_temper::runtime::native_cards::NativeCardEngine> {
        let app = self.app.as_ref().expect("native application");
        let config = NativeCardsConfig::load(&app.environment, &app.cwd, &self.plugin_root)?;
        cards::build(
            app.clone(),
            cards::Options {
                session_id: Some(self.session_id.clone()),
                plugin_root: Some(self.plugin_root.clone()),
                state_dir: None,
            },
            config,
            cancelled,
        )
    }

    fn project(&self, cancelled: Arc<AtomicBool>, capture: bool) -> Result<()> {
        let mut engine = self.engine(cancelled)?;
        engine.project_summaries()?;
        engine.project_names()?;
        engine.project_caption()?;
        if capture {
            self.app
                .as_ref()
                .expect("native application")
                .capture_native_owner(&mut engine)?;
        }
        Ok(())
    }

    pub fn request(
        &mut self,
        app: App,
        request: Request,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Output> {
        let lifecycle = matches!(request, Request::SessionStarted {} | Request::Attach {});
        if matches!(request, Request::SessionStarted {}) && self.enabled {
            return Err(CliError::message(
                "native lifecycle already started; use attach",
            ));
        }
        if lifecycle {
            self.enabled = true;
        }
        if !self.enabled {
            return Err(CliError::message("native lifecycle is not enabled"));
        }
        if app.environment.get("HERDR_ENV").map(String::as_str) != Some("1") {
            return if lifecycle {
                Ok(Output::value(
                    json!({"native":"inactive","reason":"outside_herdr"}),
                ))
            } else {
                Err(CliError::message("native action is inactive outside Herdr"))
            };
        }
        self.ensure(app)?;
        if !self.observe(Trigger::Refresh)? {
            return if lifecycle {
                Ok(Output::value(
                    json!({"native":"inactive","reason":"not_enrolled"}),
                ))
            } else {
                Err(CliError::NotEnrolled)
            };
        }
        match request {
            Request::Attach {} => {
                self.project(cancelled, false)?;
                Ok(Output::value(json!({"native":"active"})))
            }
            Request::SessionStarted {} => {
                self.project(cancelled.clone(), true)?;
                let display = self.engine(cancelled)?.open_display(OpenMode::Automatic)?;
                Ok(Output::value(
                    json!({"native":"active","display":display_name(display)}),
                ))
            }
            Request::OpenCard {} => {
                let display = self.engine(cancelled)?.open_display(OpenMode::Explicit)?;
                Ok(Output::value(json!({"display":display_name(display)})))
            }
            Request::CloseCard {} => {
                self.engine(cancelled)?.close_display()?;
                Ok(Output::value(json!({"closed":true})))
            }
            _ => unreachable!("only lifecycle requests enter the native handler"),
        }
    }

    pub fn saved(&mut self, app: App, cancelled: Arc<AtomicBool>) -> Result<()> {
        if !self.enabled || app.environment.get("HERDR_ENV").map(String::as_str) != Some("1") {
            return Ok(());
        }
        self.ensure(app)?;
        if self.observe(Trigger::Refresh)? {
            self.project(cancelled, false)?;
        }
        Ok(())
    }

    pub fn update(
        &mut self,
        update: std::result::Result<SubscriptionUpdate, String>,
    ) -> Result<()> {
        match update.map_err(CliError::message)? {
            SubscriptionUpdate::Event(event) => {
                self.observe(Trigger::Native(&event))?;
            }
            SubscriptionUpdate::Resubscribed => {
                self.observe(Trigger::Resubscribed)?;
            }
        }
        Ok(())
    }

    pub fn owner(&self) -> Owner {
        self.owner.as_ref().expect("subscribed owner").clone()
    }

    pub fn file_event(
        &mut self,
        generation: u8,
        event: notify::Result<notify::Event>,
    ) -> Result<()> {
        let Some(files) = &mut self.files else {
            return Ok(());
        };
        if !files.current(generation) {
            return Ok(());
        }
        let recovered = event.as_ref().map_or(true, |event| event.need_rescan());
        if !recovered && !event.as_ref().is_ok_and(|event| files.relevant(event)) {
            return Ok(());
        }
        if recovered {
            files.recover()?;
        }
        let workspace = self.owner.as_ref().expect("native owner").workspace.clone();
        let changed = files.prepare(Some(&workspace))?;
        if (recovered || changed) && self.observe(Trigger::Refresh)? {
            self.project(Arc::new(AtomicBool::new(false)), false)?;
        }
        Ok(())
    }

    pub fn shutdown(&mut self) {
        if let Some((cancellation, handle)) = self.subscription.take() {
            let _ = cancellation.cancel();
            let _ = handle.join();
        }
    }
}

fn display_name(outcome: DisplayOutcome) -> &'static str {
    match outcome {
        DisplayOutcome::Opened(_) => "opened",
        DisplayOutcome::Existing(_) => "existing",
        DisplayOutcome::Suppressed => "suppressed",
    }
}
