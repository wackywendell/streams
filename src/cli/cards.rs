//! Explicit Rust native trial. This module never installs or selects a default CLI.
mod events;
mod files;
mod host;
mod source;

use std::sync::{Arc, atomic::AtomicBool};
use streams_temper::frontend::herdr::HerdrClient;
use streams_temper::process::{NativeProcessRunner, ProcessRequest, ProcessRunner};
use streams_temper::runtime::native_cards::{
    DisplayCondition, DisplayOutcome, LaunchedCardContext, NativeCardEngine, NativeCardsConfig,
    SOURCE_ENV_KEYS,
};
use streams_temper::runtime::native_state::NativeState;
use streams_temper::tui::{CardSnapshot, Target as ViewTarget};

use super::{App, BTreeMap, CliError, Config, Output, PathBuf, Result, args::NativeCommand, json};

#[derive(Clone)]
pub(super) struct Options {
    pub session_id: Option<String>,
    pub plugin_root: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
}
impl App {
    pub(super) fn native_cards(&self, options: Options, command: NativeCommand) -> Result<Output> {
        if let NativeCommand::MigrateEvidence { legacy_state_dir } = command {
            return self.reset_native_evidence(options, legacy_state_dir);
        }
        let launched = LaunchedCardContext::from_environment(&self.environment)?;
        if let (Some(path), Some(context)) = (&options.plugin_root, &launched)
            && std::fs::canonicalize(if path.is_absolute() {
                path.clone()
            } else {
                self.cwd.join(path)
            })? != std::fs::canonicalize(context.plugin_root())?
        {
            return Err(CliError::message(
                "explicit plugin root differs from protected card context",
            ));
        }
        let plugin_root = options
            .plugin_root
            .clone()
            .or_else(|| {
                launched
                    .as_ref()
                    .map(|context| context.plugin_root().to_owned())
            })
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integrations/herdr-work-cards")
            });
        let config = NativeCardsConfig::load(&self.environment, &self.cwd, &plugin_root)?;
        if matches!(command, NativeCommand::Pane) {
            if !self.environment.contains_key("STREAMS_CARDS_SCOPE") {
                return Err(CliError::message(
                    "native pane requires a protected launched-card context",
                ));
            }
            let app = self.clone();
            streams_temper::tui::application::run(move |cancel, events| {
                host::Host::new(app, options, config, cancel, events)
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
        )?;
        match command {
            NativeCommand::Observe => {
                let observed = engine.observe_owner()?;
                let scope = observed.card.scope();
                Ok(Output::value(json!({"version":1,
                    "scope":{"catalogSource":scope.catalog_source,"hostId":scope.host_id,"streamId":scope.stream_id,
                        "sessionId":scope.session_id,"nativeSessionId":scope.native_session_id},
                    "native":{"pane_id":observed.context.pane_id,"tab_id":observed.context.tab_id,"workspace_id":observed.context.workspace_id},
                    "context":observed.context,
                    "session":{"title":observed.card.session().title,"title_mode":observed.card.session().title_mode},
                    "config":{"automatic":automatic,"sessionNaming":session_naming}})))
            }
            NativeCommand::Capture => self.capture_native_owner(&mut engine),
            NativeCommand::Open { automatic } => {
                let outcome = engine.open_display(if automatic {
                    streams_temper::runtime::native_cards::OpenMode::Automatic
                } else {
                    streams_temper::runtime::native_cards::OpenMode::Explicit
                })?;
                let kind = match outcome {
                    DisplayOutcome::Opened(_) => "opened",
                    DisplayOutcome::Existing(_) => "existing",
                    DisplayOutcome::Suppressed => "suppressed",
                };
                Ok(Output::value(
                    json!({"display":kind,"native":engine.status()?}),
                ))
            }
            NativeCommand::Project => {
                engine.project_summaries()?;
                let names = engine.project_names()?;
                engine.project_caption()?;
                Ok(Output::value(
                    json!({"names":names,"native":engine.status()?}),
                ))
            }
            NativeCommand::Check => {
                let status = engine.status()?;
                let healthy = !matches!(
                    status.condition,
                    DisplayCondition::Pending | DisplayCondition::Separated
                );
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
            NativeCommand::Pane | NativeCommand::MigrateEvidence { .. } => {
                unreachable!("special branch handled above")
            }
        }
    }
}

impl App {
    fn reset_native_evidence(&self, options: Options, legacy_state_dir: PathBuf) -> Result<Output> {
        if options.session_id.is_some() {
            return Err(CliError::message(
                "evidence reset is not Session-scoped; omit --session-id",
            ));
        }
        let plugin_root = options.plugin_root.unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("integrations/herdr-work-cards")
        });
        let config = NativeCardsConfig::load(&self.environment, &self.cwd, &plugin_root)?;
        let client = HerdrClient::connect(self.environment.clone(), config.timeout_seconds())
            .map_err(streams_temper::runtime::RuntimeError::from)?;
        let state = NativeState::new(evidence_directory(options.state_dir, &self.environment)?)?;
        let legacy = NativeState::new(legacy_state_dir)?;
        let report = streams_temper::runtime::native_cards::reset_evidence(
            &client,
            &state,
            &legacy,
            config.timeout_seconds(),
            &AtomicBool::new(false),
        )?;
        Ok(Output::value(json!({"version":2,"evidence_reset":report})))
    }
    pub(super) fn capture_native_owner(&self, engine: &mut NativeCardEngine) -> Result<Output> {
        use streams_temper::runtime::{
            conversations::ConversationRoots, workspace::WorkspaceRuntime,
        };
        let before = engine.observe_owner()?;
        let scope = before.card.scope();
        let mut app = self.clone();
        app.environment.insert(
            "HERDR_SOCKET_PATH".into(),
            before.context.socket.to_string_lossy().into_owned(),
        );
        let opened = super::application::open(&app.config)?;
        if opened.source_id()? != scope.catalog_source || opened.host().as_str() != scope.host_id {
            return Err(CliError::NativeContextChanged(
                "capture catalog source or host changed".into(),
            ));
        }
        let catalog = opened.backend().snapshot()?.catalog;
        let stream = catalog
            .stream(&scope.stream_id)
            .ok_or_else(|| CliError::message("captured Stream disappeared"))?;
        let sessions: Vec<_> = catalog
            .sessions()
            .iter()
            .filter(|session| session.stream_id == scope.stream_id)
            .cloned()
            .collect();
        // Reuse the engine's pinned endpoint: one protocol check per process.
        let frontend =
            streams_temper::runtime::native::BuiltinFrontend::Herdr(engine.client().clone());
        let state = app.state(&opened)?;
        let locations = app.locations(&opened)?;
        let roots = ConversationRoots::from_environment()?;
        let arrangement = WorkspaceRuntime::new(
            &state,
            &locations,
            Some(&frontend),
            opened.backend(),
            &roots,
            &app.environment,
        )
        .capture_guarded(stream, &sessions, |binding| {
            let fresh = engine.observe_owner()?;
            if binding.workspace_id() != before.context.workspace_id
                || fresh.card.scope() != scope
                || fresh.context != before.context
            {
                return Err(streams_temper::runtime::RuntimeError::NativeContextChanged(
                    "captured native owner changed during arrangement capture".into(),
                ));
            }
            Ok(())
        })?;
        Ok(Output::value(
            json!({"stream_id":stream.id,"arrangement":arrangement}),
        ))
    }
}
pub(super) fn build(
    app: App,
    options: Options,
    config: NativeCardsConfig,
    cancel: Arc<AtomicBool>,
) -> Result<NativeCardEngine> {
    let mut read_environment = app.environment.clone();
    let mut read_cwd = app.cwd.clone();
    let launched = LaunchedCardContext::from_environment(&app.environment)?;
    let (scope, owner) = if let Some(context) = &launched {
        for key in SOURCE_ENV_KEYS {
            read_environment.remove(key);
        }
        read_environment.extend(context.reader().environment.clone());
        let owner = context.owner();
        read_environment.insert("HERDR_ENV".into(), "1".into());
        read_environment.insert("HERDR_PANE_ID".into(), owner.pane_id.clone());
        read_environment.insert("HERDR_TAB_ID".into(), owner.tab_id.clone());
        read_environment.insert("HERDR_WORKSPACE_ID".into(), owner.workspace_id.clone());
        read_environment.insert(
            "STREAMS_STREAM_ID".into(),
            context.scope().stream_id.clone(),
        );
        read_cwd = context.reader().cwd.clone();
        (Some(context.scope().clone()), Some(owner.clone()))
    } else {
        (None, None)
    };
    if !read_cwd.is_absolute() {
        return Err(CliError::message("reader cwd must be absolute"));
    }
    let session_id = match (&options.session_id, &scope) {
        (Some(id), Some(scope)) if id != &scope.native_session_id => {
            return Err(CliError::message(
                "explicit Pi UUID differs from captured viewing scope",
            ));
        }
        (Some(id), _) => id.clone(),
        (None, Some(scope)) => scope.native_session_id.clone(),
        _ => {
            return Err(CliError::message(
                "--session-id requires the exact native Pi UUID",
            ));
        }
    };
    uuid::Uuid::parse_str(&session_id)
        .map_err(|_| CliError::message("native Pi session ID must be a UUID"))?;
    if let Some(read) = &app.herdr {
        if let Some(reason) = &read.refusal {
            return Err(CliError::NativeContextChanged(reason.clone()));
        }
        if read.terminal.is_none() {
            return Err(CliError::NativeContextChanged(
                "Caller origin was not attested at child startup".into(),
            ));
        }
    }
    let client = match &app.herdr {
        Some(read) => read.client.with_context(app.environment.clone()),
        None => HerdrClient::connect(app.environment.clone(), config.timeout_seconds()),
    }
    .map_err(|e| CliError::message(e.to_string()))?;
    let snapshot = app.herdr.as_ref().and_then(|read| read.snapshot.clone());
    let directory = evidence_directory(options.state_dir, &app.environment)?;
    let state = Arc::new(NativeState::new(directory)?);
    let source_app = App {
        config: Config::new(&read_environment, &read_cwd)?,
        environment: read_environment,
        cwd: read_cwd,
        input: app.input.clone(),
        herdr: app.herdr.clone(),
    };
    let source = source::ApplicationSource::new(source_app, session_id, scope, owner);
    let mut engine = if let Some(snapshot) = snapshot {
        NativeCardEngine::from_snapshot(
            client,
            Box::new(source),
            state,
            config,
            app.environment,
            cancel,
            snapshot,
        )
    } else {
        NativeCardEngine::new(
            client,
            Box::new(source),
            state,
            config,
            app.environment,
            cancel,
        )
    }?;
    if let Some(terminal) = app.herdr.as_ref().and_then(|read| read.terminal.as_deref()) {
        engine.require_caller_terminal(terminal)?;
    }
    Ok(engine)
}
fn evidence_directory(
    explicit: Option<PathBuf>,
    environment: &BTreeMap<String, String>,
) -> Result<PathBuf> {
    if let Some(path) = explicit.or_else(|| {
        environment
            .get("STREAMS_CARDS_STATE_DIR")
            .map(PathBuf::from)
    }) {
        return Ok(path);
    }
    let state_home =
        if let Some(path) = environment.get("XDG_STATE_HOME").filter(|s| !s.is_empty()) {
            PathBuf::from(path)
        } else {
            PathBuf::from(environment.get("HOME").ok_or_else(|| {
                CliError::message("native evidence requires XDG_STATE_HOME or HOME")
            })?)
            .join(".local/state")
        };
    Ok(state_home.join("streams/native"))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evidence_root_is_host_local_and_only_explicit_overrides_change_it() {
        let mut environment = BTreeMap::from([("HOME".into(), "/home/user".into())]);
        assert_eq!(
            evidence_directory(None, &environment).unwrap(),
            PathBuf::from("/home/user/.local/state/streams/native")
        );
        environment.insert("XDG_STATE_HOME".into(), "/state".into());
        environment.insert("HERDR_SOCKET_PATH".into(), "/first/herdr.sock".into());
        let first = evidence_directory(None, &environment).unwrap();
        environment.insert("HERDR_SOCKET_PATH".into(), "/other/herdr.sock".into());
        assert_eq!(first, PathBuf::from("/state/streams/native"));
        assert_eq!(evidence_directory(None, &environment).unwrap(), first);
        environment.insert("STREAMS_CARDS_STATE_DIR".into(), "/configured".into());
        assert_eq!(
            evidence_directory(None, &environment).unwrap(),
            PathBuf::from("/configured")
        );
        assert_eq!(
            evidence_directory(Some("/explicit".into()), &environment).unwrap(),
            PathBuf::from("/explicit")
        );
        assert!(evidence_directory(None, &BTreeMap::new()).is_err());
    }
}
