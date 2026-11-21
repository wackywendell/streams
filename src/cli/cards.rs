//! Native commands use one observation; card lifetime uses only an OS lock.
mod events;
mod files;
mod host;
mod source;
mod watch;

use super::{
    App, BTreeMap, CliError, Config, Output, PathBuf, Result, application, args::CardCommand, json,
};
use std::sync::{Arc, atomic::AtomicBool};
use streams::{
    config::CardMode,
    frontend::{
        FrontendId,
        herdr::{HerdrClient, HerdrNativeSnapshot},
    },
    model::{Session, Stream},
    process::{NativeProcessRunner, ProcessRequest, ProcessRunner},
    runtime::{
        card_settings::ConversationId,
        native_cards::{
            CardLock, DisplayOutcome, LaunchedCardContext, NativeCardEngine, NativeCardsConfig,
            OpenMode, PiOwner, SOURCE_ENV_KEYS,
        },
    },
    tui::{CardSnapshot, Target as ViewTarget},
};

#[derive(Clone)]
pub(super) struct Options {
    pub session_id: Option<String>,
    pub state_dir: Option<PathBuf>,
}

fn require_pi_conversation(session: &str) -> Result<ConversationId> {
    let conversation = ConversationId::parse_pair(session)?;
    if conversation.harness != "pi" {
        return Err(CliError::message(
            "classic Herdr cards support only pi/ID Sessions",
        ));
    }
    Ok(conversation)
}

fn save_manual_mode_via_cli(context: &LaunchedCardContext) -> std::result::Result<(), String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let target = format!(
        "{}/{}",
        context.scope.harness, context.scope.native_session_id
    );
    let mut command = std::process::Command::new(exe);
    command
        .env_clear()
        .envs(&context.reader.environment)
        .current_dir(&context.reader.cwd)
        .args(["card", "mode", &target, "manual"]);
    let output = command.output().map_err(|error| error.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(if stderr.is_empty() {
            format!("card mode exited with {}", output.status)
        } else {
            stderr
        });
    }
    Ok(())
}

impl App {
    pub(super) fn card_watch(&self, session: &str, plugin_root: PathBuf) -> Result<()> {
        let conversation = require_pi_conversation(session)?;
        watch::run(self.clone(), conversation.native_id, plugin_root)
    }

    pub(super) fn card(&self, command: CardCommand) -> Result<Output> {
        match command {
            CardCommand::Watch { .. } => {
                Err(CliError::message("nested card watch is not supported"))
            }
            CardCommand::Mode { session, mode } => {
                let conversation = ConversationId::parse_pair(&session)?;
                match mode {
                    Some(mode) => self.set_card_mode(&conversation, mode),
                    None => self.card_settings(&conversation),
                }
            }
            CardCommand::Pane => {
                let launched = LaunchedCardContext::from_environment(&self.environment)?;
                let context = launched
                    .ok_or_else(|| CliError::message("card pane requires card launch inputs"))?;
                let config =
                    NativeCardsConfig::load(&self.environment, &self.cwd, &context.plugin_root)?;
                let Some(_active) = CardLock::acquire(&context.lock_directory, &context.scope)?
                else {
                    return Ok(Output::plain(json!(null), String::new()));
                };
                let app = self.clone();
                let options = Options {
                    session_id: None,
                    state_dir: None,
                };
                let exit_context = context.clone();
                let exit = streams::tui::application::run(move |cancel, events| {
                    host::Host::new(app, options, config, context, cancel, events)
                })
                .map_err(CliError::message)?;
                if exit == streams::tui::application::ExitReason::Quit
                    && let Err(error) = save_manual_mode_via_cli(&exit_context)
                {
                    eprintln!(
                        "Streams — Card closed; manual mode may not have been saved: {error}"
                    );
                }
                Ok(Output::plain(json!(null), String::new()))
            }
            CardCommand::Open {
                session,
                plugin_root,
            } => {
                let conversation = require_pi_conversation(&session)?;
                let records = Arc::new(application::open_records(&self.config)?);
                if self
                    .selection(records.settings().frontend.clone(), None)?
                    .resolved
                    != Some(FrontendId::Herdr)
                {
                    return Err(CliError::message("native card effects require Herdr"));
                }
                self.set_card_mode(&conversation, CardMode::Automatic)?;
                let plugin_root = plugin_root.unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integrations/herdr-work-cards")
                });
                let result = (|| -> Result<Output> {
                    let config =
                        NativeCardsConfig::load(&self.environment, &self.cwd, &plugin_root)?
                            .with_auto_open(CardMode::Automatic);
                    let mut engine = build(
                        self.clone(),
                        Options {
                            session_id: Some(conversation.native_id.clone()),
                            state_dir: None,
                        },
                        config,
                        Arc::new(AtomicBool::new(false)),
                        Some(records),
                        None,
                    )?;
                    engine.project()?;
                    let display = engine.open_display(OpenMode::Explicit)?;
                    let mut output = Output::value(
                        json!({"display": display_name(display), "native": engine.status()?}),
                    );
                    output.accepted_save = true;
                    output.wrote = true;
                    Ok(output)
                })();
                result.map_err(|error| {
                    CliError::message(format!(
                        "Card preference saved as \"automatic\"; native action failed: {error}"
                    ))
                })
            }
            CardCommand::Close { session } => {
                let conversation = require_pi_conversation(&session)?;
                let records = Arc::new(application::open_records(&self.config)?);
                if self
                    .selection(records.settings().frontend.clone(), None)?
                    .resolved
                    != Some(FrontendId::Herdr)
                {
                    return Err(CliError::message("native card effects require Herdr"));
                }
                self.set_card_mode(&conversation, CardMode::Manual)?;
                let plugin_root =
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integrations/herdr-work-cards");
                let result = (|| -> Result<Output> {
                    let config =
                        NativeCardsConfig::load(&self.environment, &self.cwd, &plugin_root)?
                            .with_auto_open(CardMode::Manual);
                    let mut engine = build(
                        self.clone(),
                        Options {
                            session_id: Some(conversation.native_id.clone()),
                            state_dir: None,
                        },
                        config,
                        Arc::new(AtomicBool::new(false)),
                        Some(records),
                        None,
                    )?;
                    engine.close_display()?;
                    let mut output = Output::value(json!({"closed": true}));
                    output.accepted_save = true;
                    output.wrote = true;
                    Ok(output)
                })();
                result.map_err(|error| {
                    CliError::message(format!(
                        "Card preference saved as \"manual\"; native action failed: {error}"
                    ))
                })
            }
        }
    }
}

pub(super) fn build(
    app: App,
    options: Options,
    config: NativeCardsConfig,
    cancel: Arc<AtomicBool>,
    records: Option<Arc<application::OpenedRecords>>,
    preloaded: Option<(Stream, Session)>,
) -> Result<NativeCardEngine> {
    let launched = LaunchedCardContext::from_environment(&app.environment)?;
    let mut read_environment = app.environment.clone();
    let mut cwd = app.cwd.clone();
    if let Some(context) = &launched {
        for key in SOURCE_ENV_KEYS {
            read_environment.remove(key);
        }
        read_environment.extend(context.reader.environment.clone());
        read_environment.remove("STREAMS_STREAM_ID");
        cwd = context.reader.cwd.clone();
    }
    let session_id = options
        .session_id
        .as_deref()
        .or_else(|| {
            launched
                .as_ref()
                .map(|context| context.scope.native_session_id.as_str())
        })
        .ok_or_else(|| CliError::message("--session-id requires the exact native Pi UUID"))?;
    uuid::Uuid::parse_str(session_id)
        .map_err(|_| CliError::message("native Pi session ID must be a UUID"))?;
    if launched
        .as_ref()
        .is_some_and(|context| context.scope.native_session_id != session_id)
    {
        return Err(CliError::message(
            "explicit Pi UUID differs from the card's conversation",
        ));
    }
    let client = match &app.herdr {
        Some(read) => read.client.with_context(app.environment.clone()),
        None => HerdrClient::connect(app.environment.clone(), config.timeout_seconds()),
    }
    .and_then(|client| client.with_context(read_environment.clone()))
    .and_then(|client| client.observing())
    .map_err(streams::runtime::RuntimeError::from)?;
    let snapshot = match app.herdr.as_ref().and_then(|read| read.snapshot.clone()) {
        Some(snapshot) => snapshot,
        None => HerdrNativeSnapshot::parse(
            &client
                .session_snapshot()
                .map_err(streams::runtime::RuntimeError::from)?,
        )
        .map_err(streams::runtime::RuntimeError::from)?,
    };
    let owner = match app
        .herdr
        .as_ref()
        .and_then(|read| read.owner.clone())
        .or_else(|| launched.as_ref().map(|context| context.owner.clone()))
    {
        Some(owner) => owner,
        None => PiOwner::from_snapshot(
            &snapshot,
            &client
                .current_pane_from(snapshot.raw())
                .map_err(streams::runtime::RuntimeError::from)?
                .id,
            session_id,
        )?,
    };
    if app.herdr.is_none() && owner.observe(&snapshot)?.is_none() {
        return Err(CliError::message("Pi exited or changed conversation"));
    }
    let source_app = App {
        config: Config::new(&read_environment, &cwd)?,
        environment: read_environment,
        cwd,
        input: app.input.clone(),
        herdr: None,
    };
    let records = match records {
        Some(records) => records,
        None => Arc::new(application::open_records(&source_app.config)?),
    };
    let source = source::ApplicationSource::new(
        source_app,
        records,
        session_id.into(),
        launched.as_ref().map(|context| context.scope.clone()),
        preloaded,
    );
    let directory = options
        .state_dir
        .or_else(|| {
            launched
                .as_ref()
                .map(|context| context.lock_directory.clone())
        })
        .map(Ok)
        .unwrap_or_else(|| lock_directory(&app.environment))?;
    let mut environment = app.environment;
    environment.insert(
        "STREAMS_CARDS_LOCK_DIR".into(),
        directory.to_string_lossy().into_owned(),
    );
    NativeCardEngine::new(
        client,
        Box::new(source),
        snapshot,
        owner,
        config,
        environment,
        cancel,
    )
    .map_err(Into::into)
}
pub(super) fn lock_directory(environment: &BTreeMap<String, String>) -> Result<PathBuf> {
    let root = match environment
        .get("XDG_STATE_HOME")
        .filter(|value| !value.is_empty())
    {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from(
            environment
                .get("HOME")
                .ok_or_else(|| CliError::message("card locks require HOME or XDG_STATE_HOME"))?,
        )
        .join(".local/state"),
    };
    Ok(root.join("streams/cards"))
}
pub(super) fn display_name(display: DisplayOutcome) -> &'static str {
    match display {
        DisplayOutcome::Opened => "opened",
        DisplayOutcome::Existing => "existing",
        DisplayOutcome::Suppressed => "suppressed",
    }
}

fn activate(
    engine: &mut NativeCardEngine,
    environment: &BTreeMap<String, String>,
    snapshot: &CardSnapshot,
    target: &ViewTarget,
) -> std::result::Result<(), String> {
    let fresh = engine.verify_display().map_err(|e| e.to_string())?;
    if fresh.scope() != snapshot.scope() || fresh.owner() != snapshot.owner() {
        return Err("painted viewing scope changed".into());
    }
    match target {
        ViewTarget::Document(document) => engine.open_document(document).map_err(|e| e.to_string()),
        ViewTarget::Artifact(artifact) => engine.open_artifact(artifact).map_err(|e| e.to_string()),
        ViewTarget::Link(link) => {
            engine
                .client()
                .check_subscription()
                .map_err(|error| error.to_string())?;
            if !snapshot.contains_link(link) || !fresh.contains_link(link) {
                return Err("unsafe browser URL".into());
            }
            let url = streams::model::reference_url(&link.url).ok_or("unsafe browser URL")?;
            let env = environment
                .iter()
                .filter(|(key, _)| {
                    [
                        "HOME",
                        "PATH",
                        "DISPLAY",
                        "WAYLAND_DISPLAY",
                        "XDG_RUNTIME_DIR",
                        "DBUS_SESSION_BUS_ADDRESS",
                    ]
                    .contains(&key.as_str())
                })
                .map(|(key, value)| (key.into(), value.into()))
                .collect();
            NativeProcessRunner
                .run(&ProcessRequest {
                    program: if cfg!(target_os = "macos") {
                        "/usr/bin/open"
                    } else {
                        "xdg-open"
                    }
                    .into(),
                    args: vec![url.as_str().into()],
                    cwd: None,
                    input: None,
                    env,
                    allowed: vec![0],
                    timeout: 3.0,
                })
                .map(|_| ())
                .map_err(|e| e.message)
        }
    }
}
