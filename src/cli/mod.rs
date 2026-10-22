//! CLI orchestration uses the same backend for reads, captured intent and receipts.
//! Native activation is deliberately not installed by this experimental binary.

mod args;
mod cards;
mod catalog;
mod checkouts;
mod debug;
mod documents;
mod enrollment;
mod fields;
#[cfg(test)]
mod native_tests;
mod workspaces;

use args::{
    AgentCommand, CheckoutCommand, Command, ConfigCommand, FrontendCommand, NativeCommand,
    SessionCommand, Target, WorkspaceCommand,
};
use clap::Parser;
use icu_casemap::CaseMapper;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};
use streams_temper::{
    agent::{self, ContextRequest},
    application::{self, OpenedCatalog},
    codec::record_to_data,
    config::{Config, ConfigKey},
    frontend::{self, FrontendMode, FrontendRequest, FrontendSelection},
    model::{Catalog, Checkout, NamingMode, Record, Session, Stream, ValidationError},
    operations::{self, Actor, Operation},
    runtime::{
        self, CheckoutRegistry, DirectoryBindings,
        native::{self, BuiltinFrontend},
        state::RuntimeState,
    },
    sync::{CatalogBackend, ReadResult, envelope::Origin, outcomes, policy::SyncEvent},
};
use uuid::Uuid;

type Result<T> = std::result::Result<T, CliError>;

#[derive(Debug, thiserror::Error)]
pub(super) enum CliError {
    #[error("{0}")]
    Message(String),
    #[error("{0}")]
    InvalidRef(String),
    #[error("{0}")]
    NativeContextChanged(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{message}")]
    Ambiguous {
        message: String,
        candidates: Vec<String>,
    },
}

impl CliError {
    pub(super) fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
    fn data(&self) -> Value {
        let kind = match self {
            Self::Message(_) => "error",
            Self::InvalidRef(_) => "invalid-ref",
            Self::NativeContextChanged(_) => "native-context-changed",
            Self::NotFound(_) => "not-found",
            Self::Ambiguous { .. } => "ambiguous",
        };
        let mut error = json!({"kind":kind,"message":self.to_string()});
        if let Self::Ambiguous { candidates, .. } = self {
            error["candidates"] = json!(candidates);
        }
        json!({"error":error})
    }
}

macro_rules! message_error {
    ($($kind:ty),+ $(,)?) => { $(impl From<$kind> for CliError {
        fn from(error: $kind) -> Self { Self::message(error.to_string()) }
    })+ };
}
message_error!(
    io::Error,
    serde_json::Error,
    ValidationError,
    application::ApplicationError,
    streams_temper::config::ConfigError,
    streams_temper::sync::state::RepairRequired
);

impl From<frontend::FrontendSelectionError> for CliError {
    fn from(error: frontend::FrontendSelectionError) -> Self {
        let message = error.to_string();
        match error {
            frontend::FrontendSelectionError::IncompleteHerdr => Self::Message(message),
            frontend::FrontendSelectionError::Ambiguous(candidates) => Self::Ambiguous {
                message,
                candidates: candidates.into_iter().map(|id| id.0).collect(),
            },
        }
    }
}

impl From<runtime::RuntimeError> for CliError {
    fn from(error: runtime::RuntimeError) -> Self {
        match error {
            runtime::RuntimeError::NativeContextChanged(message) => {
                Self::NativeContextChanged(message)
            }
            runtime::RuntimeError::NotFound(message) => Self::NotFound(message),
            runtime::RuntimeError::Ambiguous {
                message,
                candidates,
            } => Self::Ambiguous {
                message,
                candidates,
            },
            error => Self::message(error.to_string()),
        }
    }
}

struct Output {
    value: Value,
    plain: Option<String>,
    notices: Vec<String>,
    code: u8,
}
impl Output {
    fn value(value: Value) -> Self {
        Self {
            value,
            plain: None,
            notices: Vec::new(),
            code: 0,
        }
    }
    fn plain(value: Value, plain: String) -> Self {
        Self {
            value,
            plain: Some(plain),
            notices: Vec::new(),
            code: 0,
        }
    }
    fn receipt(value: Value, ok: bool) -> Self {
        Self {
            value,
            plain: None,
            notices: Vec::new(),
            code: if ok { 0 } else { 2 },
        }
    }
}

fn preview() -> Result<Output> {
    let source = io::read_to_string(io::stdin())?;
    Ok(Output::value(agent::preview(&source)?))
}

pub(super) fn run() -> ExitCode {
    let argv: Vec<_> = std::env::args_os().collect();
    let requested_json = argv
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--json");
    let parsed = match args::Cli::try_parse_from(argv) {
        Ok(parsed) => parsed,
        Err(error) => {
            let code = error.exit_code() as u8;
            if code == 0 || !requested_json {
                let _ = error.print();
            } else {
                let _ = writeln!(
                    io::stdout(),
                    "{}",
                    CliError::message(error.to_string()).data()
                );
            }
            return ExitCode::from(code);
        }
    };
    let outcome = match parsed.command {
        Command::Debug { arguments } => debug::forward(arguments, parsed.json),
        Command::Native {
            command: NativeCommand::Pane,
            ..
        } if parsed.json => Err(CliError::message(
            "native pane is interactive and does not support --json",
        )),
        Command::Agent {
            command: AgentCommand::Preview,
        } => preview(),
        command => App::new().and_then(|app| app.execute(command)),
    };
    let (text, code) = match outcome {
        Ok(output) => {
            if !parsed.json {
                for notice in &output.notices {
                    eprintln!("{notice}");
                }
            }
            let text = if parsed.json {
                serde_json::to_string_pretty(&output.value)
            } else if let Some(plain) = output.plain {
                Ok(plain)
            } else {
                serde_json::to_string_pretty(&output.value)
            };
            match text {
                Ok(text) => (text, output.code),
                Err(error) => {
                    eprintln!(
                        "streams: could not encode output; inspect catalog before retrying: {error}"
                    );
                    return ExitCode::from(2);
                }
            }
        }
        Err(error) if parsed.json => (error.data().to_string(), 2),
        Err(error) => {
            eprintln!("streams: {error}");
            return ExitCode::from(2);
        }
    };
    if let Err(error) = writeln!(io::stdout(), "{text}") {
        eprintln!("streams: output failed; inspect catalog before retrying: {error}");
        return ExitCode::from(2);
    }
    ExitCode::from(code)
}

#[derive(Clone)]
struct App {
    config: Config,
    environment: BTreeMap<String, String>,
    cwd: PathBuf,
}
impl App {
    fn new() -> Result<Self> {
        let cwd = std::env::current_dir()?;
        let mut environment = BTreeMap::new();
        for key in [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "CMUX_WORKSPACE_ID",
            "CMUX_SOCKET_PATH",
            "HERDR_ENV",
            "HERDR_PANE_ID",
            "STREAMS_STREAM_ID",
            "HERDR_BIN_PATH",
            "HERDR_SOCKET_PATH",
            "HERDR_SESSION",
            "PI_CODING_AGENT_DIR",
            "PI_OFFLINE",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "PATH",
            "HERDR_TAB_ID",
            "HERDR_WORKSPACE_ID",
            "WORK_CARDS_CONFIG_PATH",
            "EDITOR",
            "STREAMS_CARDS_SCOPE",
            "STREAMS_CARDS_OWNER",
            "STREAMS_CARDS_NONCE",
            "STREAMS_CARDS_OWNER_TERMINAL",
            "STREAMS_CARDS_READ_ENV",
            "STREAMS_CARDS_READ_CWD",
            "STREAMS_CARDS_STATE_DIR",
            "STREAMS_CARDS_PLUGIN_ROOT",
            "STREAMS_RUST_BIN",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "XDG_RUNTIME_DIR",
            "DBUS_SESSION_BUS_ADDRESS",
        ] {
            match std::env::var(key) {
                Ok(value) => {
                    environment.insert(key.into(), value);
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(CliError::message(format!("{key} must be UTF-8")));
                }
            }
        }
        Ok(Self {
            config: Config::new(&environment, &cwd)?,
            environment,
            cwd,
        })
    }
    fn selection(&self, requested: Option<FrontendRequest>) -> Result<FrontendSelection> {
        Ok(frontend::resolve_frontend(
            self.config.load()?.frontend,
            requested,
            &self.environment,
        )?)
    }
    fn registry(&self, opened: &OpenedCatalog) -> Result<CheckoutRegistry> {
        Ok(CheckoutRegistry::new(
            self.config.checkout_state_dir(),
            opened.host().as_str(),
        )?)
    }
    fn state(&self, opened: &OpenedCatalog) -> Result<RuntimeState> {
        Ok(RuntimeState::new(
            self.config.runtime_state_dir_for(opened.root())?,
            opened.host().as_str(),
        )?)
    }
    fn locations(&self, opened: &OpenedCatalog) -> Result<DirectoryBindings> {
        Ok(DirectoryBindings::new(
            self.state(opened)?.root(),
            self.registry(opened)?,
        ))
    }
    fn native_frontend(
        &self,
        selection: &FrontendSelection,
        timeout: f64,
    ) -> Result<Option<BuiltinFrontend>> {
        selection
            .resolved
            .as_ref()
            .map(|id| {
                BuiltinFrontend::connect(
                    id,
                    std::sync::Arc::new(streams_temper::process::NativeProcessRunner),
                    self.environment.clone(),
                    timeout,
                )
                .map_err(Into::into)
            })
            .transpose()
    }
    fn current_stream_id(
        &self,
        opened: &OpenedCatalog,
        requested: Option<FrontendRequest>,
    ) -> Result<String> {
        let explicit = self
            .environment
            .get("STREAMS_STREAM_ID")
            .map(String::as_str);
        if let Some(id) = explicit {
            streams_temper::model::check_text(id, "STREAMS_STREAM_ID", false)?;
        }
        let configured = self.config.load()?.frontend;
        let selection = match frontend::resolve_frontend(configured, requested, &self.environment) {
            Ok(selection) => selection,
            // Only policy ambiguity/incompleteness permits this historical --current fallback.
            Err(error) => return explicit.map(str::to_owned).ok_or_else(|| error.into()),
        };
        match self.native_frontend(&selection, 30.0)? {
            Some(frontend) => Ok(frontend.caller()?.current_stream_id(&self.state(opened)?, explicit)?),
            None => explicit.map(str::to_owned).ok_or_else(|| CliError::message("no native frontend selected; pass --frontend herdr or --frontend cmux (or use resume)")),
        }
    }
    fn path(&self, path: &Path) -> Result<PathBuf> {
        if path == Path::new("~") || path.starts_with("~/") {
            let home = self
                .environment
                .get("HOME")
                .ok_or_else(|| CliError::message("HOME is unavailable"))?;
            let suffix = path
                .strip_prefix("~")
                .map_err(|error| CliError::message(error.to_string()))?;
            return Ok(Path::new(home).join(suffix));
        }
        Ok(path.to_path_buf())
    }
    fn read_text(&self, path: &Path) -> Result<String> {
        if path == Path::new("-") {
            Ok(io::read_to_string(io::stdin())?)
        } else {
            Ok(fs::read_to_string(self.path(path)?)?)
        }
    }
    fn target<'a>(
        &self,
        opened: &OpenedCatalog,
        catalog: &'a Catalog,
        target: &Target,
    ) -> Result<&'a Stream> {
        if target.current {
            if target.reference.is_some() {
                return Err(CliError::message(
                    "--current cannot be combined with a reference",
                ));
            }
            let id = self.current_stream_id(opened, target.frontend.clone())?;
            return catalog.stream(&id).ok_or_else(|| {
                CliError::NotFound(format!("current stream {id:?} is not in the local catalog"))
            });
        }
        let reference = target
            .reference
            .as_deref()
            .ok_or_else(|| CliError::InvalidRef("supply a reference or --current".into()))?;
        resolve(catalog.streams(), reference, |s| (&s.id, &s.title, None))
    }
    fn execute(&self, command: Command) -> Result<Output> {
        match command {
            Command::Debug { arguments } => debug::forward(arguments, false),
            Command::Init {
                sync,
                remote,
                branch,
                adopt,
            } => {
                let opened =
                    application::initialize(&self.config, sync, remote.as_deref(), &branch, adopt)?;
                let mut value =
                    json!({"catalog":opened.root(),"host_id":opened.host().as_str(),"sync":sync});
                if adopt {
                    let git = streams_temper::sync::git_io::Git::new(opened.root(), 30.0);
                    let tip = git
                        .head()
                        .map_err(|error| CliError::message(error.to_string()))?;
                    let history = streams_temper::sync::history::read_history(&git, Some(&tip))?;
                    value["baseline_revision"] = json!(history.root());
                }
                Ok(Output::value(value))
            }
            Command::Config { action } => self.configure(action),
            Command::Native {
                session_id,
                plugin_root,
                state_dir,
                expect,
                command,
            } => self.native_cards(
                cards::Options {
                    session_id,
                    plugin_root,
                    state_dir,
                    expect,
                },
                command,
            ),
            Command::Frontend {
                command: FrontendCommand::Get { frontend },
            } => {
                let selection = self.selection(frontend)?;
                Ok(Output::value(
                    json!({"configured":mode(&selection.configured),
                    "requested":selection.requested.map(|request| match request { FrontendRequest::Auto=>"auto".into(), FrontendRequest::Explicit(id)=>id.0 }),
                    "resolved":selection.resolved.map(|id|id.0)}),
                ))
            }
            Command::List {
                query,
                status,
                archived,
                all,
                refresh,
            } => {
                let opened = application::open(&self.config)?;
                catalog::list(
                    opened.backend(),
                    query.as_deref(),
                    &status,
                    archived,
                    all,
                    refresh,
                )
            }
            Command::Show { target, refresh } => {
                let opened = application::open(&self.config)?;
                catalog::show(opened.backend(), refresh, |catalog| {
                    self.target(&opened, catalog, &target)
                })
            }
            Command::Create { title, fields, cwd } => {
                self.create_stream(title, fields, cwd, workspaces::Creation::Record)
            }
            Command::Start {
                title,
                fields,
                cwd,
                frontend,
            } => self.create_stream(
                title,
                fields,
                cwd,
                workspaces::Creation::Workspace(frontend),
            ),
            Command::Open {
                target,
                session,
                cwd,
            } => self.open_workspace(target, session, cwd),
            Command::Resume {
                target,
                session,
                cwd,
            } => self.resume_session(target, session, cwd),
            Command::Workspace {
                command: WorkspaceCommand::Capture { target },
            } => self.capture_workspace(target),
            Command::Update {
                target,
                title,
                fields,
            } => {
                let opened = application::open(&self.config)?;
                catalog::update(
                    opened.backend(),
                    |catalog| self.target(&opened, catalog, &target),
                    title,
                    &fields,
                    |path| self.read_text(path),
                    || Uuid::new_v4().to_string(),
                )
            }
            Command::Archive { target } => self.archive(&target, true),
            Command::Restore { target } => self.archive(&target, false),
            Command::Check { reference } => {
                let opened = application::open(&self.config)?;
                catalog::check(opened.backend(), reference.as_deref())
            }
            Command::Sync { event } => {
                let opened = application::open(&self.config)?;
                catalog::sync(opened.backend(), event)
            }
            Command::Session { command } => self.session(command),
            Command::Checkout { command } => self.checkout(command),
            Command::Agent { command } => self.agent(command),
        }
    }
    fn archive(&self, target: &Target, archived: bool) -> Result<Output> {
        let opened = application::open(&self.config)?;
        catalog::archive(
            opened.backend(),
            |catalog| self.target(&opened, catalog, target),
            archived,
            || Uuid::new_v4().to_string(),
        )
    }
    fn configure(&self, action: Option<ConfigCommand>) -> Result<Output> {
        if let Some(action) = action {
            let key = match action {
                ConfigCommand::Get { key } => key,
                ConfigCommand::Set { key, value } => {
                    self.config.set(key, &value)?;
                    key
                }
            };
            let value = self.config.get(key)?;
            let plain = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            return Ok(Output::plain(value, plain));
        }
        let values = self.config.values()?;
        let overrides = self.config.read_overrides()?;
        let mut plain = format!("Config: {}", self.config.config_path().display());
        for key in [
            ConfigKey::CatalogRoot,
            ConfigKey::FrontendMode,
            ConfigKey::StreamSeconds,
            ConfigKey::SessionSeconds,
        ] {
            let text = key.to_string();
            let (section, name) = text.split_once('.').expect("closed configuration key");
            let source = if overrides.get(section).and_then(|o| o.get(name)).is_some() {
                "configured"
            } else {
                "default"
            };
            plain.push_str(&format!("\n{text}: {} ({source})", values[section][name]));
        }
        Ok(Output::plain(values, plain))
    }
    fn session(&self, command: SessionCommand) -> Result<Output> {
        let opened = application::open(&self.config)?;
        let catalog = opened.backend().snapshot()?.catalog;
        match command {
            SessionCommand::List { stream } => {
                let stream = stream
                    .as_deref()
                    .map(|reference| {
                        resolve(catalog.streams(), reference, |s| (&s.id, &s.title, None))
                    })
                    .transpose()?;
                let sessions: Vec<_> = catalog
                    .sessions()
                    .iter()
                    .filter(|session| stream.is_none_or(|stream| session.stream_id == stream.id))
                    .collect();
                Ok(Output::value(json!({"sessions":sessions})))
            }
            SessionCommand::Show { reference } => Ok(Output::value(
                json!({"session":resolve(catalog.sessions(),&reference,|s|(&s.id,&s.title,Some(&s.host_id)))?}),
            )),
            SessionCommand::Attach {
                target,
                harness,
                session_id,
                title,
                cwd,
                repository,
                vcs_ref,
                directory_within_checkout,
            } => {
                let stream = self.target(&opened, &catalog, &target)?;
                let repository_url = repository
                    .as_deref()
                    .map(|alias| {
                        stream
                            .repositories
                            .get(alias)
                            .map(|r| r.url.as_str())
                            .ok_or_else(|| {
                                CliError::message(format!("unknown repository alias: {alias:?}"))
                            })
                    })
                    .transpose()?;
                let checkout = match (repository.as_deref(), vcs_ref) {
                    (Some(alias), Some(reference)) => {
                        Some(Checkout::new(alias, reference, directory_within_checkout)?)
                    }
                    (None, None) if directory_within_checkout == "." => None,
                    (None, _) => {
                        return Err(CliError::message(
                            "checkout --ref/--directory-within-checkout requires --repository",
                        ));
                    }
                    (Some(_), None) => {
                        return Err(CliError::message("checkout --repository requires --ref"));
                    }
                };
                let mut session = Session::new(
                    agent::session_id_for(opened.host().as_str(), harness, &session_id),
                    opened.host().as_str(),
                    &stream.id,
                    harness.to_string(),
                    session_id,
                    title,
                )?;
                session.title_mode = NamingMode::Manual;
                session.checkout = checkout;
                let authority = opened.backend().authority(&stream.id)?;
                let operation = Operation::new(
                    Uuid::new_v4().to_string(),
                    None,
                    Record::Session(session.clone()),
                    Actor::User,
                    authority.map(|a| a.token),
                    vec![],
                )?;
                let saved = save(opened.backend(), &operation)?;
                let extra = if saved.accepted() {
                    let directory = cwd.unwrap_or_else(|| self.cwd.clone());
                    self.locations(&opened).and_then(|locations|Ok(locations.remember(&stream.id,&self.path(&directory)?,Some(&session.id),false,repository_url,None)?)).err()
                        .map(|error|json!({"runtime":{"error":error.to_string(),"detail":"session saved; pass --cwd when resuming"}}))
                } else {
                    None
                };
                finish(
                    opened.backend(),
                    &operation,
                    saved,
                    SyncEvent::Attach,
                    extra,
                )
            }
            SessionCommand::Update {
                reference,
                title,
                fields,
            } => {
                let before = resolve(catalog.sessions(), &reference, |s| {
                    (&s.id, &s.title, Some(&s.host_id))
                })?;
                let mut changes =
                    fields::session(before, &fields, &mut |path| self.read_text(path))?;
                if let Some(title) = title {
                    changes.insert("title".into(), json!(title));
                }
                if changes.is_empty() {
                    return Err(CliError::message(
                        "session update requires at least one field change",
                    ));
                }
                let authority = opened.backend().authority(&before.stream_id)?;
                let operation = operations::update(
                    Uuid::new_v4().to_string(),
                    &Record::Session(before.clone()),
                    &changes,
                    Actor::User,
                    authority.map(|a| a.token),
                    vec![],
                )?;
                let saved = save(opened.backend(), &operation)?;
                finish(
                    opened.backend(),
                    &operation,
                    saved,
                    SyncEvent::Session,
                    None,
                )
            }
            SessionCommand::Directory {
                reference,
                cwd,
                checkout_id,
                within,
                repository,
            } => {
                let session = resolve(catalog.sessions(), &reference, |s| {
                    (&s.id, &s.title, Some(&s.host_id))
                })?;
                if session.host_id != opened.host().as_str() {
                    return Err(CliError::message(
                        "directory selection requires a Session belonging to this host",
                    ));
                }
                if within.is_some() && checkout_id.is_none() {
                    return Err(CliError::message("--within requires --checkout"));
                }
                if repository.is_some() && cwd.is_none() && checkout_id.is_none() {
                    return Err(CliError::message(
                        "--repository requires --cwd or --checkout",
                    ));
                }
                let locations = self.locations(&opened)?;
                if cwd.is_some() || checkout_id.is_some() {
                    let stream = catalog
                        .stream(&session.stream_id)
                        .ok_or_else(|| CliError::message("Session stream is missing"))?;
                    let portable_url = session
                        .checkout
                        .as_ref()
                        .and_then(|checkout| checkout.repository_url(stream));
                    let url = match repository.as_deref() {
                        Some(alias) => Some(
                            stream
                                .repositories
                                .get(alias)
                                .map(|repo| repo.url.as_str())
                                .ok_or_else(|| {
                                    CliError::message(format!(
                                        "unknown repository alias: {alias:?}"
                                    ))
                                })?,
                        ),
                        None => portable_url,
                    };
                    if repository.is_some() && portable_url.is_some() && url != portable_url {
                        return Err(CliError::message(
                            "selected repository differs from the Session's portable checkout",
                        ));
                    }
                    let directory = if let Some(id) = checkout_id.as_deref() {
                        self.registry(&opened)?
                            .directory(id, within.as_deref().unwrap_or("."))?
                    } else {
                        self.path(cwd.as_deref().expect("directory selection"))?
                    };
                    locations.remember(
                        &stream.id,
                        &directory,
                        Some(&session.id),
                        false,
                        url,
                        checkout_id.as_deref(),
                    )?;
                }
                let directory = locations
                    .read(&session.stream_id, Some(&session.id))?
                    .map(|binding| binding_data(&binding));
                Ok(Output::value(
                    json!({"session_id":session.id,"host_id":session.host_id,"directory":directory}),
                ))
            }
        }
    }
    fn capture_inputs(&self, stream: Option<&str>, workspace: Option<&str>) -> Result<Output> {
        use agent::CaptureTarget;
        use streams_temper::runtime::{
            conversations::ConversationRoots,
            state::{RuntimeState, herdr_scope},
        };
        let inspected = application::inspect(&self.config)?;
        let state = RuntimeState::new(
            self.config.runtime_state_dir_for(&inspected.root)?,
            inspected.host.as_str(),
        )?;
        let native = (|| -> Result<_> {
            let connection = self.native_frontend(
                &self.selection(None)?,
                self.config.load()?.sync.timeout_seconds(),
            )?;
            let client = connection
                .as_ref()
                .and_then(BuiltinFrontend::herdr)
                .ok_or_else(|| {
                    CliError::message("Herdr topology unavailable; open state unknown")
                })?;
            let endpoint = client.endpoint();
            let scope = herdr_scope(&endpoint.socket, endpoint.session.as_deref());
            Ok((
                client
                    .topology()
                    .map_err(|error| CliError::message(error.to_string()))?,
                scope,
            ))
        })()
        .map_err(|error| error.to_string());
        let native = native
            .as_ref()
            .map(|(topology, scope)| (topology, scope.as_str()))
            .map_err(Clone::clone);
        let target = match (stream, workspace) {
            (Some(id), None) => CaptureTarget::Stream(id),
            (None, Some(id)) => CaptureTarget::Workspace(id),
            (None, None) => CaptureTarget::Inventory,
            _ => return Err(CliError::message("select a stream or workspace, not both")),
        };
        Ok(Output::value(agent::capture_inputs(
            &inspected,
            &state,
            native,
            target,
            &ConversationRoots::from_environment()?,
        )?))
    }

    fn agent_observation(
        &self,
        harness: streams_temper::runtime::conversations::Harness,
        session_id: &str,
        stream_id: Option<&str>,
        attest: bool,
        include_inventory: bool,
        native_timeout: f64,
    ) -> Result<AgentObservation> {
        let connection = self.native_frontend(&self.selection(None)?, native_timeout)?;
        let snapshot = connection
            .as_ref()
            .and_then(BuiltinFrontend::herdr)
            .map(|client| client.session_snapshot())
            .transpose()
            .map_err(|e| CliError::message(e.to_string()))?;
        self.agent_observation_from_native(
            harness,
            session_id,
            stream_id,
            attest,
            include_inventory,
            connection
                .as_ref()
                .map(|connection| (connection, snapshot.as_ref())),
        )
    }

    fn agent_observation_from_native(
        &self,
        harness: streams_temper::runtime::conversations::Harness,
        session_id: &str,
        stream_id: Option<&str>,
        attest: bool,
        include_inventory: bool,
        observed: Option<(&BuiltinFrontend, Option<&Value>)>,
    ) -> Result<AgentObservation> {
        let opened = application::open(&self.config)?;
        let state = self.state(&opened)?;
        let connection = observed.map(|(connection, _)| connection);
        let workspace_stream = connection
            .as_ref()
            .map(|frontend| {
                match observed.and_then(|(_, snapshot)| snapshot) {
                    Some(snapshot) => frontend.caller_from_snapshot(snapshot)?,
                    None => frontend.caller()?,
                }
                .context_stream_id(
                    &state,
                    self.environment
                        .get("STREAMS_STREAM_ID")
                        .map(String::as_str),
                )
            })
            .transpose()?
            .flatten();
        let source = opened.source_id()?;
        let context = agent::context(
            opened.backend(),
            opened.host().as_str(),
            &source,
            ContextRequest {
                harness,
                native_id: session_id,
                stream_id,
                environment_stream_id: self
                    .environment
                    .get("STREAMS_STREAM_ID")
                    .map(String::as_str),
                workspace_stream_id: workspace_stream.as_deref(),
            },
        )
        .map_err(|error| CliError::message(error.to_string()))?;
        let documents = if let (Some(stream), Some(session)) = (&context.stream, &context.session) {
            documents::context(
                &self.registry(&opened)?,
                &self.locations(&opened)?,
                stream,
                session,
            )
        } else {
            json!({"checkouts":[],"document_locations":[]})
        };
        let native = if attest {
            let (stream, session) = context
                .stream
                .as_ref()
                .zip(context.session.as_ref())
                .ok_or_else(|| {
                    CliError::message("native projection requires an enrolled Stream and Session")
                })?;
            let client = connection.and_then(BuiltinFrontend::herdr).ok_or_else(|| {
                CliError::message("native projection attestation requires an exact Herdr caller")
            })?;
            Some(native::attest_context_from_snapshot(
                client,
                observed
                    .and_then(|(_, snapshot)| snapshot)
                    .expect("attested snapshot"),
                &state,
                stream,
                session,
                context.authority.as_ref(),
            )?)
        } else {
            None
        };
        let native_inventory = if include_inventory {
            if !attest {
                return Err(CliError::message("native inventory requires --attest"));
            }
            Some(native::native_inventory_from_snapshot(
                connection
                    .and_then(BuiltinFrontend::herdr)
                    .expect("attestation requires Herdr"),
                observed
                    .and_then(|(_, snapshot)| snapshot)
                    .expect("attested snapshot"),
            )?)
        } else {
            None
        };
        Ok(AgentObservation {
            context,
            documents,
            native,
            native_inventory,
        })
    }

    fn agent(&self, command: AgentCommand) -> Result<Output> {
        match command {
            AgentCommand::Enroll => self.enroll(),
            AgentCommand::Preview => preview(),
            AgentCommand::CaptureInputs {
                stream_id,
                workspace_id,
            } => self.capture_inputs(stream_id.as_deref(), workspace_id.as_deref()),
            AgentCommand::Apply => {
                let opened = application::open(&self.config)?;
                let source = io::read_to_string(io::stdin())?;
                catalog::apply(opened.backend(), &source, &opened.source_id()?)
            }
            AgentCommand::Context {
                harness,
                session_id,
                stream_id,
                attest,
                native_inventory,
            } => {
                let observed = self.agent_observation(
                    harness,
                    &session_id,
                    stream_id.as_deref(),
                    attest,
                    native_inventory,
                    30.0,
                )?;
                let mut value = serde_json::to_value(&observed.context)?;
                value.as_object_mut().expect("context object").extend(
                    observed
                        .documents
                        .as_object()
                        .expect("document context object")
                        .clone(),
                );
                if let Some(native) = observed.native {
                    value["native"] = serde_json::to_value(native)?;
                }
                if let Some(inventory) = observed.native_inventory {
                    value["native_inventory"] = serde_json::to_value(inventory)?;
                }
                Ok(Output::value(value))
            }
        }
    }
}

struct AgentObservation {
    context: agent::AgentContext,
    documents: Value,
    native: Option<native::NativeCaller>,
    native_inventory: Option<native::NativeInventory>,
}

fn mode(mode: &FrontendMode) -> &str {
    match mode {
        FrontendMode::Auto => "auto",
        FrontendMode::None => "none",
        FrontendMode::Explicit(id) => &id.0,
    }
}
fn fold(value: &str) -> String {
    CaseMapper::new().fold_string(value).into_owned()
}
fn printable(value: &str) -> String {
    use unicode_general_category::{GeneralCategory::*, get_general_category};
    value
        .chars()
        .map(|c| match get_general_category(c) {
            Control | Format | Surrogate | PrivateUse | Unassigned | LineSeparator
            | ParagraphSeparator | SpaceSeparator
                if c != ' ' =>
            {
                ' '
            }
            _ => c,
        })
        .collect()
}
fn resolve<'a, T>(
    records: &'a [T],
    reference: &str,
    identity: impl Fn(&T) -> (&str, &str, Option<&str>),
) -> Result<&'a T> {
    if reference.trim().is_empty() {
        return Err(CliError::InvalidRef("reference must not be blank".into()));
    }
    let folded = fold(reference);
    let candidates = records
        .iter()
        .filter(|item| {
            let (id, _, host) = identity(item);
            id == reference || host.is_some_and(|host| reference == format!("{host}/{id}"))
        })
        .collect::<Vec<_>>();
    let candidates = if candidates.is_empty() {
        records
            .iter()
            .filter(|item| fold(identity(item).1) == folded)
            .collect()
    } else {
        candidates
    };
    let candidates = if candidates.is_empty() {
        records
            .iter()
            .filter(|item| fold(identity(item).1).contains(&folded))
            .collect()
    } else {
        candidates
    };
    match candidates.as_slice() {
        [record] => Ok(*record),
        [] => Err(CliError::NotFound(format!(
            "no record matches {reference:?}"
        ))),
        _ => Err(CliError::Ambiguous {
            message: format!("ambiguous reference {reference:?}; choose a stable ID"),
            candidates: candidates
                .iter()
                .map(|item| {
                    let (id, _, host) = identity(item);
                    host.map_or_else(|| id.to_owned(), |host| format!("{host}/{id}"))
                })
                .collect(),
        }),
    }
}
fn catalog_data(catalog: &Catalog) -> Value {
    json!({"streams":catalog.streams(),"sessions":catalog.sessions()})
}
fn read_catalog(backend: &dyn CatalogBackend, refresh: bool) -> Result<(Catalog, Value)> {
    let ReadResult {
        catalog,
        local,
        confirmed,
        freshness,
        fetched_at,
        conflicts,
        detail,
    } = backend.read(refresh)?;
    let mut sharing = json!({"freshness":freshness,"fetched_at":fetched_at,"view":if catalog==local.catalog{"local"}else{"projected"},"conflicts":conflicts,"detail":detail});
    if !conflicts.is_empty()
        && let Some(confirmed) = confirmed
    {
        sharing["confirmed"] = catalog_data(&confirmed.catalog);
    }
    Ok((catalog, sharing))
}
fn result_data(result: &outcomes::Result) -> Result<Value> {
    let mut value = serde_json::to_value(result)?;
    if let Some(domain) = value.get_mut("domain").and_then(Value::as_object_mut) {
        domain.remove("record");
    }
    Ok(value)
}
fn save(backend: &dyn CatalogBackend, operation: &Operation) -> Result<outcomes::Result> {
    Ok(backend.save(
        operation,
        Some(&Origin::from([("command".into(), "streams".into())])),
    )?)
}
fn finish(
    backend: &dyn CatalogBackend,
    operation: &Operation,
    saved: outcomes::Result,
    event: SyncEvent,
    extra: Option<Value>,
) -> Result<Output> {
    let record = saved
        .domain
        .as_ref()
        .and_then(|domain| match domain {
            operations::Reconciliation::Changes { record, .. } => Some(record.as_ref()),
            _ => None,
        })
        .unwrap_or_else(|| operation.after());
    let key = record.kind().to_string();
    let mut payload = json!({"operation_id":operation.id(),"save":result_data(&saved)?,key.clone():record_to_data(record)});
    let mut ok = saved.accepted();
    if ok {
        let completion = (|| -> Result<()> {
            let publication = backend.sync(event);
            ok = !publication.requires_repair();
            payload["publication"] = result_data(&publication)?;
            if ok {
                let current = backend
                    .snapshot()?
                    .catalog
                    .get(&operation.target())
                    .ok_or_else(|| {
                        CliError::message(
                            "saved record is missing; inspect catalog before retrying",
                        )
                    })?;
                payload[&key] = record_to_data(&current);
            }
            Ok(())
        })();
        if let Err(error) = completion {
            payload["completion_error"] = json!({"message":error.to_string(),"detail":"local save succeeded; inspect before retrying"});
            ok = false;
        }
    }
    let completed = extra
        .as_ref()
        .is_none_or(|extra| extra["runtime"].get("error").is_none());
    if let Some(Value::Object(extra)) = extra {
        payload
            .as_object_mut()
            .expect("receipt object")
            .extend(extra);
    }
    Ok(Output::receipt(payload, ok && completed))
}
fn binding_data(binding: &runtime::DirectoryBinding) -> Value {
    json!({"cwd":binding.cwd,"checkout_id":binding.checkout.as_ref().map(|c|&c.id),"relative_cwd":binding.checkout.as_ref().map(|c|&c.relative_cwd),"repository_url":binding.checkout.as_ref().and_then(|c|c.repository_url.as_ref())})
}
