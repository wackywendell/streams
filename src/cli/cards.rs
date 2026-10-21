//! Explicit Rust native trial. This module never installs or selects a default CLI.
mod source;

use serde::Deserialize;
use std::sync::{Arc, atomic::AtomicBool};
use streams_temper::frontend::herdr::HerdrClient;
use streams_temper::process::{NativeProcessRunner, ProcessRequest, ProcessRunner};
use streams_temper::runtime::native_cards::{
    DisplayCondition, DisplayOutcome, NameProjectionIntent, NativeCardEngine, NativeCardsConfig,
    SOURCE_ENV_KEYS,
};
use streams_temper::runtime::native_state::NativeState;
use streams_temper::tui::{
    CardSnapshot, NativeOwnerObservation, Scope, Target as ViewTarget, application::PaneHost,
};

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
}
impl App {
    pub(super) fn native_cards(&self, options: Options, command: NativeCommand) -> Result<Output> {
        let plugin_root = options.plugin_root.clone().unwrap_or_else(|| {
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
        let mut engine = build(
            self.clone(),
            options,
            config,
            Arc::new(AtomicBool::new(false)),
        )?;
        match command {
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

fn build(
    app: App,
    options: Options,
    config: NativeCardsConfig,
    cancel: Arc<AtomicBool>,
) -> Result<NativeCardEngine> {
    let mut read_environment = app.environment.clone();
    let mut read_cwd = app.cwd.clone();
    let (scope, owner) = if app.environment.contains_key("STREAMS_CARDS_SCOPE") {
        let wire: ScopeInput = decode_env(&app.environment, "STREAMS_CARDS_SCOPE")?;
        let scope = Scope {
            catalog_source: wire.catalog_source,
            host_id: wire.host_id,
            stream_id: wire.stream_id,
            session_id: wire.session_id,
            harness: "pi".into(),
            native_session_id: wire.native_session_id,
        };
        let owner: OwnerInput = decode_env(&app.environment, "STREAMS_CARDS_OWNER")?;
        let captured: BTreeMap<String, String> =
            decode_env(&app.environment, "STREAMS_CARDS_READ_ENV")?;
        if captured
            .keys()
            .any(|key| !SOURCE_ENV_KEYS.contains(&key.as_str()))
        {
            return Err(CliError::message(
                "captured reader environment contains an unsupported key",
            ));
        }
        for key in SOURCE_ENV_KEYS {
            read_environment.remove(key);
        }
        read_environment.extend(captured);
        read_environment.insert("HERDR_ENV".into(), "1".into());
        read_environment.insert("HERDR_PANE_ID".into(), owner.pane_id.clone());
        read_environment.insert("HERDR_TAB_ID".into(), owner.tab_id.clone());
        read_environment.insert("HERDR_WORKSPACE_ID".into(), owner.workspace_id.clone());
        read_environment.insert("STREAMS_STREAM_ID".into(), scope.stream_id.clone());
        read_cwd = PathBuf::from(
            app.environment
                .get("STREAMS_CARDS_READ_CWD")
                .ok_or_else(|| CliError::message("captured reader cwd missing"))?,
        );
        (
            Some(scope),
            Some(NativeOwnerObservation {
                pane_id: owner.pane_id,
                tab_id: owner.tab_id,
                workspace_id: owner.workspace_id,
            }),
        )
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
    NativeCardEngine::new(
        client,
        Box::new(source),
        state,
        config,
        app.environment,
        cancel,
    )
    .map_err(Into::into)
}
fn decode_env<T: serde::de::DeserializeOwned>(
    env: &BTreeMap<String, String>,
    key: &str,
) -> Result<T> {
    let value: serde_json::Value = streams_temper::codec::load_json(
        env.get(key)
            .ok_or_else(|| CliError::message(format!("missing {key}")))?,
    )
    .map_err(|e| CliError::message(e.to_string()))?;
    if !value.is_object() {
        return Err(CliError::message(format!("{key} must be an object")));
    }
    serde_json::from_value(value).map_err(Into::into)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScopeInput {
    catalog_source: String,
    host_id: String,
    stream_id: String,
    session_id: String,
    native_session_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerInput {
    pane_id: String,
    tab_id: String,
    workspace_id: String,
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
                if !matches!(url.scheme(), "https" | "http")
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
