//! CLI orchestration uses the same backend for reads, captured intent and receipts.
//! Native activation is deliberately not installed by this experimental binary.

mod args;
mod artifacts;
mod cards;
mod catalog;
mod checkouts;
mod debug;
mod documents;
mod enrollment;
mod fields;
mod integration;
#[cfg(test)]
mod native_tests;
mod pager;
mod serve;
mod sessions;
mod settings;
mod workspaces;

use args::{
    AgentCommand, CheckoutCommand, Command, ConfigCommand, FrontendCommand, NativeCommand, Target,
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
use streams::{
    agent::{self, ContextRequest},
    application,
    config::{Config, ConfigKey},
    frontend::{self, FrontendMode, FrontendRequest, FrontendSelection},
    model::{Record, Session, Stream, TitleMode, ValidationError},
    operations::{self, Actor},
    runtime::{
        self, CheckoutRegistry, DirectoryBindings, native::BuiltinFrontend, state::RuntimeState,
    },
    sync::outcomes,
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
    NotFound(String),
    #[error("{0}")]
    UnsupportedCommand(String),
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
            Self::NotFound(_) => "not-found",
            Self::UnsupportedCommand(_) => "unsupported-command",
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
    streams::config::ConfigError,
    streams::sync::state::RepairRequired
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
    accepted_save: bool,
}
impl Output {
    fn value(value: Value) -> Self {
        Self {
            value,
            plain: None,
            notices: Vec::new(),
            code: 0,
            accepted_save: false,
        }
    }
    fn plain(value: Value, plain: String) -> Self {
        Self {
            value,
            plain: Some(plain),
            notices: Vec::new(),
            code: 0,
            accepted_save: false,
        }
    }
    fn receipt(value: Value, ok: bool) -> Self {
        Self {
            value,
            plain: None,
            notices: Vec::new(),
            code: if ok { 0 } else { 2 },
            accepted_save: false,
        }
    }
}

#[derive(Clone, Default)]
enum Input {
    #[default]
    Stdin,
}
impl Input {
    fn read(&self) -> Result<String> {
        match self {
            Self::Stdin => Ok(io::read_to_string(io::stdin())?),
        }
    }
}

fn preview(input: &Input) -> Result<Output> {
    Ok(Output::value(agent::preview(&input.read()?)?))
}

fn execute(command: Command, input: Input, json: bool) -> Result<Output> {
    match command {
        Command::Debug { arguments } => debug::forward(arguments, json),
        Command::Native {
            command: NativeCommand::Pane,
            ..
        } if json => Err(CliError::message(
            "native pane is interactive and does not support --json",
        )),
        Command::Integration {
            command: args::IntegrationCommand::ViewDocument { .. },
        } if json => Err(CliError::message(
            "integration view-document is interactive and does not support --json",
        )),
        Command::Artifact {
            command: args::ArtifactCommand::View { .. },
        } if json => Err(CliError::message(
            "artifact view is interactive and does not support --json",
        )),
        Command::Agent {
            command: AgentCommand::Preview,
        } => preview(&input),
        command @ Command::Integration {
            command:
                args::IntegrationCommand::Request { .. } | args::IntegrationCommand::Describe { .. },
        } => {
            let mut app = App::new()
                .map_err(|_| CliError::message("integration configuration is unavailable"))?;
            app.input = input;
            app.execute(command)
        }
        command => {
            let mut app = App::new()?;
            app.input = input;
            app.execute(command)
        }
    }
}

pub(super) fn run() -> ExitCode {
    let argv: Vec<_> = std::env::args_os().collect();
    let requested_json = argv
        .iter()
        .take_while(|arg| *arg != "--")
        .any(|arg| arg == "--json");
    let mut parsed = match args::Cli::try_parse_from(argv) {
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
    if matches!(
        &parsed.command,
        Command::Integration {
            command: args::IntegrationCommand::Request { .. }
                | args::IntegrationCommand::Describe { .. }
        }
    ) {
        parsed.json = false; // This protocol owns stdout, including its failure framing.
    }
    if let Command::Serve {
        session_id,
        sdk_pid,
        plugin_root,
    } = parsed.command
    {
        return match serve::run(session_id, sdk_pid, plugin_root) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("streams serve: {error}");
                ExitCode::from(2)
            }
        };
    }
    let raw_content = !parsed.json
        && matches!(
            &parsed.command,
            Command::Artifact {
                command: args::ArtifactCommand::Read { .. }
            }
        );
    let outcome = execute(parsed.command, Input::Stdin, parsed.json);
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
    let printed = if raw_content {
        write!(io::stdout(), "{text}")
    } else {
        writeln!(io::stdout(), "{text}")
    };
    if let Err(error) = printed {
        eprintln!("streams: output failed; inspect catalog before retrying: {error}");
        return ExitCode::from(2);
    }
    ExitCode::from(code)
}

#[derive(Clone)]
struct HerdrRead {
    client: frontend::herdr::HerdrClient,
    snapshot: Option<frontend::herdr::HerdrNativeSnapshot>,
    owner: Option<runtime::native_cards::PiOwner>,
    workspace_stream: Option<String>,
}

#[derive(Clone)]
struct App {
    config: Config,
    environment: BTreeMap<String, String>,
    cwd: PathBuf,
    input: Input,
    herdr: Option<HerdrRead>,
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
            "HC_SOCKET",
            "HC_SESSION",
            "HC_TERMINAL_ID",
            "PI_CODING_AGENT_DIR",
            "PI_OFFLINE",
            "CODEX_HOME",
            "CLAUDE_CONFIG_DIR",
            "PATH",
            "HERDR_TAB_ID",
            "HERDR_WORKSPACE_ID",
            "WORK_CARDS_CONFIG_PATH",
            "EDITOR",
            "STREAMS_CARDS_CONTEXT",
            "STREAMS_ARTIFACT_CATALOG_SOURCE",
            "STREAMS_BIN_PATH",
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
            input: Input::Stdin,
            herdr: None,
        })
    }
    fn selection(&self, requested: Option<FrontendRequest>) -> Result<FrontendSelection> {
        Ok(frontend::resolve_frontend(
            self.config.load()?.frontend,
            requested,
            &self.environment,
        )?)
    }
    fn registry(&self, host: &str) -> Result<CheckoutRegistry> {
        Ok(CheckoutRegistry::new(
            self.config.checkout_state_dir(),
            host,
        )?)
    }
    fn state(&self, root: &Path, host: &str) -> Result<RuntimeState> {
        Ok(RuntimeState::new(
            self.config.runtime_state_dir_for(root)?,
            host,
        )?)
    }
    fn locations(&self, root: &Path, host: &str) -> Result<DirectoryBindings> {
        Ok(DirectoryBindings::new(
            self.state(root, host)?.root(),
            self.registry(host)?,
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
                if id.0 == "herdr"
                    && let Some(read) = &self.herdr
                {
                    return Ok(BuiltinFrontend::Herdr(
                        read.client
                            .with_context(self.environment.clone())
                            .map_err(runtime::RuntimeError::from)?,
                    ));
                }
                BuiltinFrontend::connect(
                    id,
                    std::sync::Arc::new(streams::process::NativeProcessRunner),
                    self.environment.clone(),
                    timeout,
                )
                .map_err(Into::into)
            })
            .transpose()
    }
    fn current_stream_id(
        &self,
        host: &str,
        catalog_source: &str,
        requested: Option<FrontendRequest>,
    ) -> Result<String> {
        let configured = self.config.load()?.frontend;
        let explicit = self
            .environment
            .get("STREAMS_STREAM_ID")
            .map(String::as_str);
        if let Some(id) = explicit {
            streams::model::check_text(id, "STREAMS_STREAM_ID", false)?;
            return Ok(id.to_owned());
        }
        let selection = frontend::resolve_frontend(configured, requested, &self.environment)?;
        let connection = self.native_frontend(&selection, 30.0)?;
        let client = connection
            .as_ref()
            .and_then(BuiltinFrontend::herdr)
            .ok_or_else(|| {
                CliError::message(
                    "--current requires an explicit Stream key or Herdr workspace metadata",
                )
            })?;
        let snapshot = client
            .session_snapshot()
            .and_then(|snapshot| frontend::herdr::HerdrNativeSnapshot::parse(&snapshot))
            .map_err(runtime::RuntimeError::from)?;
        runtime::native_markers::selected_stream(
            &snapshot,
            &client
                .current_pane_from(snapshot.raw())
                .map_err(runtime::RuntimeError::from)?
                .workspace_id,
            catalog_source,
            host,
        )?
        .ok_or_else(|| {
            CliError::message(
                "calling workspace has no Stream selector; pass an explicit reference",
            )
        })
    }
    fn record_target(
        &self,
        opened: &application::OpenedRecords,
        target: &Target,
    ) -> Result<Stream> {
        use streams::{
            model::RecordKind,
            storage::records::{RecordAddress, RecordStore},
        };
        if target.current {
            if target.reference.is_some() {
                return Err(CliError::message(
                    "--current cannot be combined with a reference",
                ));
            }
            let id = self.current_stream_id(
                opened.host().as_str(),
                opened.source_id(),
                target.frontend.clone(),
            )?;
            let address = RecordAddress::new(opened.host().as_str(), RecordKind::Stream, &id)
                .map_err(application::ApplicationError::from)?;
            return Ok(opened
                .records()
                .with_reader(|reader| -> application::Result<Stream> {
                    let read = reader
                        .read(&address)?
                        .ok_or_else(|| ValidationError::new("current Stream is missing"))?;
                    let Record::Stream(stream) = read.record else {
                        unreachable!("addressed Stream codec")
                    };
                    Ok(stream)
                })?);
        }
        let reference = target
            .reference
            .as_deref()
            .ok_or_else(|| CliError::InvalidRef("supply a reference or --current".into()))?;
        Ok(catalog::select_stream(&opened.inventory()?, reference)?.clone())
    }
    fn sync_records(&self) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        catalog::sync(&opened)
    }
    fn refresh_records(&self, opened: &application::OpenedRecords) -> Result<Value> {
        let transport =
            streams::sync::records::RecordSync::open(opened.root(), opened.host().as_str())
                .map_err(application::ApplicationError::from)?;
        result_data(&transport.sync())
    }
    fn show_records(&self, target: &Target, refresh: bool) -> Result<Output> {
        if target.current && target.reference.is_some() {
            return Err(CliError::message(
                "--current cannot be combined with a reference",
            ));
        }
        if !target.current && target.reference.is_none() {
            return Err(CliError::InvalidRef(
                "supply a reference or --current".into(),
            ));
        }
        let opened = application::open_records(&self.config)?;
        let publication = if refresh {
            Some(self.refresh_records(&opened)?)
        } else {
            None
        };
        let mut value = if target.current {
            json!({"stream":self.record_target(&opened,target)?})
        } else {
            let reference = target
                .reference
                .as_deref()
                .ok_or_else(|| CliError::InvalidRef("supply a reference or --current".into()))?;
            let records = opened.inventory()?;
            let stream = catalog::select_stream(&records, reference)?;
            let sessions: Vec<_> = records
                .iter()
                .filter_map(|read| match &read.record {
                    Record::Session(session)
                        if session.stream_id == stream.id
                            && session.host_id == stream.owner_host_id =>
                    {
                        Some(session)
                    }
                    _ => None,
                })
                .collect();
            json!({"stream":stream,"sessions":sessions})
        };
        if let Some(publication) = publication {
            value["sharing"] = publication;
        }
        Ok(Output::value(value))
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
            self.input.read()
        } else {
            Ok(fs::read_to_string(self.path(path)?)?)
        }
    }
    fn execute(&self, command: Command) -> Result<Output> {
        match command {
            Command::Integration {
                command:
                    args::IntegrationCommand::BindSpace {
                        space_ref,
                        stream,
                        scope_json,
                    },
            } => self.integration_bind_space(&space_ref, &stream, &scope_json),
            Command::Integration {
                command:
                    args::IntegrationCommand::ViewDocument {
                        scope_json,
                        reference_json,
                    },
            } => self.integration_view_document(&scope_json, &reference_json),
            Command::Integration {
                command: args::IntegrationCommand::Describe { frontend },
            } => self.integration_describe(frontend),
            Command::Integration {
                command: args::IntegrationCommand::Request { request_json },
            } => self.integration_request(&request_json),
            Command::Serve { .. } => Err(CliError::UnsupportedCommand(
                "nested serve is not supported".into(),
            )),
            Command::Debug { arguments } => debug::forward(arguments, false),
            Command::Init {
                sync,
                remote,
                branch,
            } => {
                let opened = application::initialize_records(
                    &self.config,
                    sync,
                    remote.as_deref(),
                    &branch,
                )?;
                Ok(Output::value(
                    json!({"catalog":opened.root(),"host_id":opened.host().as_str(),"sync":sync}),
                ))
            }
            Command::Config { action } => self.configure(action),
            Command::Native {
                session_id,
                plugin_root,
                state_dir,
                command,
            } => self.native_cards(
                cards::Options {
                    session_id,
                    plugin_root,
                    state_dir,
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
                let opened = application::open_records(&self.config)?;
                let publication = if refresh {
                    Some(self.refresh_records(&opened)?)
                } else {
                    None
                };
                catalog::list_records(
                    &opened,
                    query.as_deref(),
                    &status,
                    archived,
                    all,
                    publication,
                )
            }
            Command::Show { target, refresh } => self.show_records(&target, refresh),
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
            Command::Update {
                target,
                title,
                fields,
            } => {
                let opened = application::open_records(&self.config)?;
                let settings = self.config.load()?;
                if settings.catalog_root != opened.root() {
                    return Err(CliError::message(
                        "catalog root setting changed during update",
                    ));
                }
                let stream = self.record_target(&opened, &target)?;
                catalog::update_record(
                    &opened,
                    &stream,
                    title,
                    &fields,
                    settings.naming,
                    |path| self.read_text(path),
                    || Uuid::new_v4().to_string(),
                )
            }
            Command::Archive { target } => self.archive(&target, true),
            Command::Restore { target } => self.archive(&target, false),
            Command::Check { reference } => {
                let opened = application::open_records(&self.config)?;
                catalog::check(&opened, reference.as_deref())
            }
            Command::Sync => self.sync_records(),
            Command::Artifact { command } => self.artifact_cli(command),
            Command::Session { command } => self.session(command),
            Command::Checkout { command } => self.checkout(command),
            Command::Agent { command } => self.agent(command),
        }
    }
    fn archive(&self, target: &Target, archived: bool) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let settings = self.config.load()?;
        if settings.catalog_root != opened.root() {
            return Err(CliError::message(
                "catalog root setting changed during archive",
            ));
        }
        let stream = self.record_target(&opened, target)?;
        catalog::save_stream_edit(
            &opened,
            &stream,
            json!({"archived":archived})
                .as_object()
                .expect("archive fields")
                .clone(),
            settings.naming,
            &Uuid::new_v4().to_string(),
        )
    }
    fn configure(&self, action: Option<ConfigCommand>) -> Result<Output> {
        if let Some(action) = action {
            let key = match action {
                ConfigCommand::Get { key } => key,
                ConfigCommand::Set { key, value } => {
                    match key {
                        ConfigKey::StreamNaming | ConfigKey::SessionNaming => {
                            let target = if key == ConfigKey::StreamNaming {
                                streams::child_protocol::TitleTarget::Stream
                            } else {
                                streams::child_protocol::TitleTarget::Session
                            };
                            let mode = value.parse().map_err(|_| {
                                CliError::message(format!("{key} must be automatic or manual"))
                            })?;
                            self.set_title_default(target, mode)?;
                        }
                        _ => self.config.set(key, &value)?,
                    }
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
            ConfigKey::StreamNaming,
            ConfigKey::SessionNaming,
            ConfigKey::CardAutoOpen,
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
    fn capture_inputs(&self, stream: Option<&str>, workspace: Option<&str>) -> Result<Output> {
        use agent::CaptureTarget;
        use streams::runtime::{conversations::ConversationRoots, state::RuntimeState};
        let opened = application::open_records(&self.config)?;
        let settings = self.config.load()?;
        if settings.catalog_root != opened.root() {
            return Err(CliError::message(
                "catalog root setting changed during capture inspection",
            ));
        }
        let state = RuntimeState::new(
            self.config.runtime_state_dir_for(opened.root())?,
            opened.host().as_str(),
        )?;
        let native = (|| -> Result<_> {
            let connection = self.native_frontend(
                &self.selection(None)?,
                streams::sync::COMMAND_TIMEOUT_SECONDS,
            )?;
            let client = connection
                .as_ref()
                .and_then(BuiltinFrontend::herdr)
                .ok_or_else(|| {
                    CliError::message("Herdr topology unavailable; open state unknown")
                })?;
            client
                .session_snapshot()
                .and_then(|snapshot| frontend::herdr::HerdrNativeSnapshot::parse(&snapshot))
                .map_err(runtime::RuntimeError::from)
                .map_err(CliError::from)
        })()
        .map_err(|error| error.to_string());
        let native = native.as_ref().map_err(Clone::clone);
        let target = match (stream, workspace) {
            (Some(id), None) => CaptureTarget::Stream(id),
            (None, Some(id)) => CaptureTarget::Workspace(id),
            (None, None) => CaptureTarget::Inventory,
            _ => return Err(CliError::message("select a stream or workspace, not both")),
        };
        Ok(Output::value(agent::capture_inputs(
            &opened,
            native,
            target,
            settings.naming,
            &DirectoryBindings::new(state.root(), self.registry(opened.host().as_str())?),
            &ConversationRoots::from_environment()?,
        )?))
    }

    fn context_from_records(
        &self,
        opened: &application::OpenedRecords,
        request: ContextRequest<'_>,
        title_defaults: streams::model::TitleDefaults,
    ) -> Result<agent::RecordContext> {
        streams::storage::records::RecordStore::with_reader(opened.records(), |reader| {
            agent::record_context(
                reader,
                opened.host().as_str(),
                opened.source_id(),
                request,
                title_defaults,
            )
        })
        .map_err(|error| CliError::message(error.to_string()))
    }

    fn agent_observation(
        &self,
        harness: streams::runtime::conversations::Harness,
        session_id: &str,
        stream_id: Option<&str>,
    ) -> Result<AgentObservation> {
        let opened = application::open_records(&self.config)?;
        let settings = self.config.load()?;
        if settings.catalog_root != opened.root() {
            return Err(CliError::message(
                "catalog root setting changed during observation",
            ));
        }
        let context = self.context_from_records(
            &opened,
            ContextRequest {
                harness,
                native_id: session_id,
                stream_id,
                environment_stream_id: self
                    .environment
                    .get("STREAMS_STREAM_ID")
                    .map(String::as_str),
                workspace_stream_id: self
                    .herdr
                    .as_ref()
                    .and_then(|read| read.workspace_stream.as_deref()),
            },
            settings.naming,
        )?;
        let documents = if let (Some(stream), Some(session)) = (&context.stream, &context.session) {
            documents::context(
                &self.registry(opened.host().as_str())?,
                &self.locations(opened.root(), opened.host().as_str())?,
                stream,
                session,
            )
        } else {
            json!({"checkouts":[],"document_locations":[]})
        };
        Ok(AgentObservation { context, documents })
    }

    fn agent(&self, command: AgentCommand) -> Result<Output> {
        match command {
            AgentCommand::Enroll => self.enroll(),
            AgentCommand::Preview => preview(&self.input),
            AgentCommand::CaptureInputs {
                stream_id,
                workspace_id,
            } => self.capture_inputs(stream_id.as_deref(), workspace_id.as_deref()),
            AgentCommand::Apply => self.apply_request(&self.input.read()?),
            AgentCommand::Context {
                harness,
                session_id,
                stream_id,
            } => self
                .agent_observation(harness, &session_id, stream_id.as_deref())?
                .output(),
        }
    }

    fn apply_request(&self, source: &str) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let settings = self.config.load()?;
        if settings.catalog_root != opened.root() {
            return Err(CliError::message(
                "catalog root setting changed during apply",
            ));
        }
        catalog::apply(
            opened.records(),
            source,
            opened.source_id(),
            settings.naming,
        )
    }
}

struct AgentObservation {
    context: agent::RecordContext,
    documents: Value,
}
impl AgentObservation {
    fn output(self) -> Result<Output> {
        let mut value = serde_json::to_value(&self.context)?;
        value.as_object_mut().expect("context object").extend(
            self.documents
                .as_object()
                .expect("document context object")
                .clone(),
        );
        Ok(Output::value(value))
    }
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
fn result_data(result: &outcomes::Result) -> Result<Value> {
    let mut value = serde_json::to_value(result)?;
    if let Some(domain) = value.get_mut("domain").and_then(Value::as_object_mut) {
        domain.remove("record");
    }
    Ok(value)
}
fn binding_data(binding: &runtime::DirectoryBinding) -> Value {
    json!({"cwd":binding.cwd,"checkout_id":binding.checkout.as_ref().map(|c|&c.id),"relative_cwd":binding.checkout.as_ref().map(|c|&c.relative_cwd),"repository_url":binding.checkout.as_ref().and_then(|c|c.repository_url.as_ref())})
}
