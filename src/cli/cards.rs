//! Explicit Rust native trial. This module never installs or selects a default CLI.
mod source;

use std::sync::{Arc, atomic::AtomicBool};
use streams_temper::frontend::herdr::HerdrClient;
use streams_temper::process::{NativeProcessRunner, ProcessRequest, ProcessRunner};
use streams_temper::runtime::native_cards::{
    DisplayCondition, DisplayOutcome, LaunchedCardContext, NameProjectionIntent, NativeCardEngine,
    NativeCardsConfig, SOURCE_ENV_KEYS,
};
use streams_temper::runtime::native_state::NativeState;
use streams_temper::tui::{CardSnapshot, Target as ViewTarget, application::PaneHost};

use super::{
    App, BTreeMap, CliError, Config, Output, PathBuf, Result,
    args::{NativeCommand, NativeReclaim},
    json,
};

#[derive(Clone)]
pub(super) struct Options {
    pub session_id: Option<String>,
    pub plugin_root: Option<PathBuf>,
    pub state_dir: Option<PathBuf>,
    pub expect: Option<String>,
}
impl App {
    pub(super) fn native_cards(&self, options: Options, command: NativeCommand) -> Result<Output> {
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
            let interval = config.poll_interval();
            let app = self.clone();
            streams_temper::tui::application::run(
                move |cancel| {
                    let environment = app.environment.clone();
                    build(app, options, config, cancel)
                        .map(|engine| Host {
                            engine,
                            environment,
                        })
                        .map_err(|e| e.to_string())
                },
                interval,
            )
            .map_err(CliError::message)?;
            return Ok(Output::plain(json!(null), String::new()));
        }
        let automatic = config.startup_automatic();
        let session_naming = config.session_naming();
        let timeout = config.timeout_seconds();
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
            NativeCommand::Capture => self.capture_native_owner(&mut engine, timeout),
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
            NativeCommand::Project { reclaim } => {
                engine.project_summaries()?;
                let intent = match reclaim {
                    None => NameProjectionIntent::Automatic,
                    Some(NativeReclaim::All) => NameProjectionIntent::Explicit,
                    Some(NativeReclaim::Workspace) => NameProjectionIntent::Workspace,
                    Some(NativeReclaim::Tab) => NameProjectionIntent::Tab,
                };
                let names = engine.project_names(intent)?;
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
            NativeCommand::Pane => unreachable!("interactive branch handled above"),
        }
    }
}

impl App {
    fn capture_native_owner(&self, engine: &mut NativeCardEngine, timeout: f64) -> Result<Output> {
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
        let selection =
            app.selection(Some(streams_temper::frontend::FrontendRequest::Explicit(
                streams_temper::frontend::FrontendId("herdr".into()),
            )))?;
        let frontend = app.native_frontend(&selection, timeout)?;
        let state = app.state(&opened)?;
        let locations = app.locations(&opened)?;
        let roots = ConversationRoots::from_environment()?;
        let arrangement = WorkspaceRuntime::new(
            &state,
            &locations,
            frontend.as_ref(),
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
fn build(
    app: App,
    options: Options,
    config: NativeCardsConfig,
    cancel: Arc<AtomicBool>,
) -> Result<NativeCardEngine> {
    let mut read_environment = app.environment.clone();
    let mut read_cwd = app.cwd.clone();
    let launched = LaunchedCardContext::from_environment(&app.environment)?;
    let expected = options
        .expect
        .as_deref()
        .map(Expectation::parse)
        .transpose()?;
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
    if let Some(expected) = &expected {
        let captured = expected.scope();
        if scope.as_ref().is_some_and(|scope| scope != &captured) {
            return Err(CliError::message("conflicting captured native scopes"));
        }
    }
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
    let client = HerdrClient::connect(
        Arc::new(NativeProcessRunner),
        app.environment.clone(),
        config.timeout_seconds(),
    )
    .map_err(|e| CliError::message(e.to_string()))?;
    read_environment.insert(
        "HERDR_SOCKET_PATH".into(),
        client
            .endpoint()
            .socket
            .to_str()
            .ok_or_else(|| CliError::message("native socket must be UTF-8"))?
            .into(),
    );
    let directory = options
        .state_dir
        .or_else(|| {
            app.environment
                .get("STREAMS_CARDS_STATE_DIR")
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| {
            client
                .endpoint()
                .socket
                .parent()
                .expect("absolute socket parent")
                .join("streams-native")
        });
    let state = Arc::new(NativeState::new(directory)?);
    let source_app = App {
        config: Config::new(&read_environment, &read_cwd)?,
        environment: read_environment,
        cwd: read_cwd,
    };
    let source = source::ApplicationSource::new(
        source_app,
        session_id,
        scope,
        owner,
        config.timeout_seconds(),
    );
    let mut engine = NativeCardEngine::new(
        client,
        Box::new(source),
        state,
        config,
        app.environment,
        cancel,
    )?;
    if let Some(expected) = expected {
        engine.pin_owner(&expected.scope(), &expected.context)?;
    }
    Ok(engine)
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Expectation {
    version: u8,
    scope: ExpectedScope,
    context: streams_temper::runtime::native_cards::NativeOwnerContext,
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedScope {
    catalog_source: String,
    host_id: String,
    stream_id: String,
    session_id: String,
    native_session_id: String,
}
impl Expectation {
    fn parse(input: &str) -> Result<Self> {
        let value = streams_temper::codec::load_json(input)
            .map_err(|e| CliError::message(e.to_string()))?;
        if !value.is_object() || !value["scope"].is_object() || !value["context"].is_object() {
            return Err(CliError::message(
                "captured native expectation and its scope/context must be objects",
            ));
        }
        let expected: Self = serde_json::from_value(value)?;
        if expected.version != 1 {
            return Err(CliError::message("unsupported native expectation version"));
        }
        Ok(expected)
    }
    fn scope(&self) -> streams_temper::tui::Scope {
        streams_temper::tui::Scope {
            catalog_source: self.scope.catalog_source.clone(),
            host_id: self.scope.host_id.clone(),
            stream_id: self.scope.stream_id.clone(),
            session_id: self.scope.session_id.clone(),
            native_session_id: self.scope.native_session_id.clone(),
            harness: "pi".into(),
        }
    }
}
struct Host {
    engine: NativeCardEngine,
    environment: BTreeMap<String, String>,
}
impl PaneHost for Host {
    fn read(&mut self) -> std::result::Result<CardSnapshot, String> {
        self.engine.verify_display().map_err(|e| e.to_string())
    }
    fn activate(
        &mut self,
        snapshot: &CardSnapshot,
        target: &ViewTarget,
    ) -> std::result::Result<(), String> {
        let fresh = self.engine.verify_display().map_err(|e| e.to_string())?;
        if fresh.scope() != snapshot.scope() || fresh.owner() != snapshot.owner() {
            return Err("painted viewing scope changed".into());
        }
        match target {
            ViewTarget::Document(document) => self
                .engine
                .open_document(document)
                .map_err(|e| e.to_string()),
            ViewTarget::Link(url) => {
                if !snapshot.contains_link(url)
                    || !fresh.contains_link(url)
                    || !matches!(url.scheme(), "https" | "http")
                    || !url.username().is_empty()
                    || url.password().is_some()
                {
                    return Err("unsafe browser URL".into());
                }
                let env = self
                    .environment
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
    fn close(&mut self) -> std::result::Result<(), String> {
        self.engine.close_display().map_err(|e| e.to_string())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expectation_requires_versioned_named_complete_context() {
        let valid = json!({"version":1,"scope":{"catalogSource":"source","hostId":"host","streamId":"stream","sessionId":"session","nativeSessionId":"native"},
            "context":{"socket":"/socket","paneId":"pane","terminalId":"terminal","workspaceId":"workspace","tabId":"tab","binding":null}});
        assert!(Expectation::parse(&valid.to_string()).is_ok());
        for invalid in [
            json!([]),
            json!({}),
            {
                let mut value = valid.clone();
                value["version"] = json!(2);
                value
            },
            {
                let mut value = valid.clone();
                value["context"].as_object_mut().unwrap().remove("binding");
                value
            },
            {
                let mut value = valid.clone();
                value["context"] = json!(["/socket", "pane", "terminal", "workspace", "tab", null]);
                value
            },
            {
                let mut value = valid;
                value["scope"]["unexpected"] = json!(true);
                value
            },
        ] {
            assert!(
                Expectation::parse(&invalid.to_string()).is_err(),
                "{invalid}"
            );
        }
    }
}
