//! Native commands use one observation; card lifetime uses only an OS lock.
mod events;
mod files;
mod host;
mod source;

use super::{
    App, BTreeMap, CliError, Config, Output, PathBuf, Result, application, args::NativeCommand,
    json,
};
use std::sync::{Arc, atomic::AtomicBool};
use streams_temper::{
    agent::RecordContext,
    frontend::herdr::{HerdrClient, HerdrNativeSnapshot},
    process::{NativeProcessRunner, ProcessRequest, ProcessRunner},
    runtime::native_cards::{
        CardLock, DisplayCondition, DisplayOutcome, LaunchedCardContext, NativeCardEngine,
        NativeCardsConfig, PiOwner, SOURCE_ENV_KEYS,
    },
    tui::{CardSnapshot, Target as ViewTarget},
};

#[derive(Clone)]
pub(super) struct Options {
    pub session_id: Option<String>,
    pub plugin_root: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
}
impl App {
    pub(super) fn native_cards(&self, options: Options, command: NativeCommand) -> Result<Output> {
        let launched = LaunchedCardContext::from_environment(&self.environment)?;
        let plugin_root = options
            .plugin_root
            .clone()
            .or_else(|| launched.as_ref().map(|context| context.plugin_root.clone()))
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integrations/herdr-work-cards")
            });
        let config = NativeCardsConfig::load(&self.environment, &self.cwd, &plugin_root)?;
        if matches!(command, NativeCommand::Pane) {
            let context = launched
                .ok_or_else(|| CliError::message("native pane requires card launch inputs"))?;
            let Some(_active) = CardLock::acquire(&context.lock_directory, &context.scope)? else {
                return Ok(Output::plain(json!(null), String::new()));
            };
            let app = self.clone();
            streams_temper::tui::application::run(move |cancel, events| {
                host::Host::new(app, options, config, context, cancel, events)
            })
            .map_err(CliError::message)?;
            return Ok(Output::plain(json!(null), String::new()));
        }
        let automatic = config.startup_automatic();
        let session_naming = config.session_naming();
        let mut engine = build(
            self.clone(),
            options,
            config,
            Arc::new(AtomicBool::new(false)),
            None,
            None,
        )?;
        match command {
            NativeCommand::Observe => {
                let observed = engine.observe_owner()?;
                let scope = observed.card.scope();
                let owner = observed.card.owner();
                Ok(Output::value(
                    json!({"version":1,"scope":scope,"owner":observed.owner,
                    "native":{"pane_id":owner.pane_id,"tab_id":owner.tab_id,"workspace_id":owner.workspace_id},
                    "session":{"title":observed.card.session().title,"title_mode":observed.card.session().title_mode},
                    "config":{"automatic":automatic,"sessionNaming":session_naming}}),
                ))
            }
            NativeCommand::Open { automatic } => {
                engine.project()?;
                let display = engine.open_display(if automatic {
                    streams_temper::runtime::native_cards::OpenMode::Automatic
                } else {
                    streams_temper::runtime::native_cards::OpenMode::Explicit
                })?;
                Ok(Output::value(
                    json!({"display":display_name(display),"native":engine.status()?}),
                ))
            }
            NativeCommand::Project => {
                let names = engine.project()?;
                Ok(Output::value(
                    json!({"names":names,"native":engine.status()?}),
                ))
            }
            NativeCommand::Check => {
                let status = engine.status()?;
                let healthy = status.condition != DisplayCondition::Separated;
                Ok(Output::receipt(json!({"native":status}), healthy))
            }
            NativeCommand::Focus => {
                engine.focus_display()?;
                Ok(Output::value(json!({"focused":true})))
            }
            NativeCommand::Close => {
                engine.close_display()?;
                Ok(Output::value(json!({"closed":true})))
            }
            NativeCommand::Pane => unreachable!("pane handled above"),
        }
    }
}

pub(super) fn build(
    app: App,
    options: Options,
    config: NativeCardsConfig,
    cancel: Arc<AtomicBool>,
    records: Option<Arc<application::OpenedRecords>>,
    context: Option<RecordContext>,
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
    .map_err(streams_temper::runtime::RuntimeError::from)?;
    let snapshot = match app.herdr.as_ref().and_then(|read| read.snapshot.clone()) {
        Some(snapshot) => snapshot,
        None => HerdrNativeSnapshot::parse(
            &client
                .session_snapshot()
                .map_err(streams_temper::runtime::RuntimeError::from)?,
        )
        .map_err(streams_temper::runtime::RuntimeError::from)?,
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
                .map_err(streams_temper::runtime::RuntimeError::from)?
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
        context,
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
        ViewTarget::Link(url) => {
            engine
                .client()
                .check_subscription()
                .map_err(|error| error.to_string())?;
            if !snapshot.contains_link(url)
                || !fresh.contains_link(url)
                || !matches!(url.scheme(), "https" | "http")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err("unsafe browser URL".into());
            }
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
