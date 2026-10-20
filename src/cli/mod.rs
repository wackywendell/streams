//! CLI orchestration uses the same backend for reads, captured intent and receipts.
//! Native activation is deliberately not installed by this experimental binary.

mod args;
mod documents;
mod fields;

use args::{
    AgentCommand, CheckoutCommand, Command, ConfigCommand, FrontendCommand, SessionCommand, Target,
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
    runtime::{self, CheckoutRegistry, DirectoryBindings},
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
    let outcome = App::new().and_then(|app| app.execute(parsed.command));
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
            "HERDR_ENV",
            "HERDR_PANE_ID",
            "STREAMS_STREAM_ID",
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
    fn locations(&self, opened: &OpenedCatalog) -> Result<DirectoryBindings> {
        use sha2::{Digest, Sha256};
        // Python Runtime scopes each catalog's local state by its host identity.
        let host = serde_json::to_vec(&[opened.host().as_str()])?;
        let state = self
            .config
            .runtime_state_dir_for(opened.root())?
            .join(format!("{:x}", Sha256::digest(host)));
        Ok(DirectoryBindings::new(state, self.registry(opened)?))
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
    fn target<'a>(&self, catalog: &'a Catalog, target: &Target) -> Result<&'a Stream> {
        if target.current {
            if target.reference.is_some() {
                return Err(CliError::message(
                    "--current cannot be combined with a reference",
                ));
            }
            if self.selection(target.frontend.clone())?.resolved.is_some() {
                return Err(CliError::message(
                    "native current-workspace targeting is not enabled in this Rust slice",
                ));
            }
            let id = self.environment.get("STREAMS_STREAM_ID").ok_or_else(|| {
                CliError::message(
                    "native current-workspace targeting is not enabled in this Rust slice",
                )
            })?;
            streams_temper::model::check_text(id, "STREAMS_STREAM_ID", false)?;
            return catalog.stream(id).ok_or_else(|| {
                CliError::NotFound(format!("current stream {id:?} is not in the local catalog"))
            });
        }
        let reference = target
            .reference
            .as_deref()
            .ok_or_else(|| CliError::InvalidRef("supply a reference or --current".into()))?;
        resolve(&catalog.streams, reference, |s| (&s.id, &s.title, None))
    }
    fn execute(&self, command: Command) -> Result<Output> {
        match command {
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
                    value["baseline_revision"] = json!(history.root);
                }
                Ok(Output::value(value))
            }
            Command::Config { action } => self.configure(action),
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
                let (catalog, sharing) = read_catalog(opened.backend(), refresh)?;
                let query = query.as_deref().map(fold);
                let mut streams: Vec<_> = catalog
                    .streams
                    .iter()
                    .filter(|stream| {
                        (all || stream.archived == archived)
                            && (status.is_empty() || status.contains(&stream.status))
                            && query.as_ref().is_none_or(|q| {
                                fold(&stream.title).contains(q)
                                    || fold(stream.goal.as_deref().unwrap_or("")).contains(q)
                            })
                    })
                    .collect();
                streams.sort_by_cached_key(|s| (fold(&s.title), s.id.clone()));
                let mut plain = String::from("ID  STATUS  TITLE");
                for stream in &streams {
                    plain.push_str(&format!(
                        "\n{}  {}  {}",
                        stream.id,
                        stream.status,
                        printable(&stream.title)
                    ));
                }
                let data: Vec<_> = streams
                    .iter()
                    .map(|s| record_to_data(&Record::Stream((*s).clone())))
                    .collect();
                let mut output = Output::plain(json!({"streams":data,"sharing":sharing}), plain);
                let sharing = &output.value["sharing"];
                if let Some(detail) = sharing["detail"]
                    .as_str()
                    .filter(|detail| !detail.is_empty())
                {
                    output.notices.push(detail.to_owned());
                }
                if sharing["conflicts"]
                    .as_array()
                    .is_some_and(|conflicts| !conflicts.is_empty())
                {
                    output.notices.push("catalog conflicts: showing local records; use list --json or show for confirmed alternatives".into());
                }
                Ok(output)
            }
            Command::Show { target, refresh } => {
                let opened = application::open(&self.config)?;
                let (catalog, sharing) = read_catalog(opened.backend(), refresh)?;
                let stream = self.target(&catalog, &target)?;
                let sessions: Vec<_> = catalog
                    .sessions
                    .iter()
                    .filter(|s| s.stream_id == stream.id)
                    .collect();
                Ok(Output::value(
                    json!({"stream":stream,"sessions":sessions,"sharing":sharing}),
                ))
            }
            Command::Create { title, fields, cwd } => {
                let opened = application::open(&self.config)?;
                let mut stream = Stream::new(
                    Uuid::new_v4().to_string(),
                    title,
                    None,
                    opened.host().as_str(),
                )?;
                stream.title_mode = NamingMode::Manual;
                let changes = fields::stream(&stream, &fields, &mut |path| self.read_text(path))?;
                let base = Record::Stream(stream);
                let desired = operations::update(
                    Uuid::new_v4().to_string(),
                    &base,
                    &changes,
                    Actor::User,
                    None,
                    vec![],
                )?
                .after()
                .clone();
                let operation = Operation::new(
                    Uuid::new_v4().to_string(),
                    None,
                    desired,
                    Actor::User,
                    None,
                    vec![],
                )?;
                let saved = save(opened.backend(), &operation)?;
                let extra = if saved.accepted() {
                    self.locations(&opened).and_then(|locations| Ok(locations.remember(operation.target().id(), &self.path(cwd.as_deref().unwrap_or(&self.cwd))?, None, false, None, None)?))
                        .err().map(|error|json!({"runtime":{"error":error.data()["error"],"detail":"stream saved; retry open with its ID"}}))
                } else {
                    None
                };
                let incomplete = extra.is_some();
                let mut output = finish(
                    opened.backend(),
                    &operation,
                    saved,
                    SyncEvent::Stream,
                    extra,
                )?;
                if incomplete {
                    output.code = 2;
                }
                Ok(output)
            }
            Command::Update {
                target,
                title,
                fields,
            } => {
                let opened = application::open(&self.config)?;
                let snapshot = opened.backend().snapshot()?;
                let stream = self.target(&snapshot.catalog, &target)?;
                let mut changes =
                    fields::stream(stream, &fields, &mut |path| self.read_text(path))?;
                if let Some(title) = title {
                    changes.insert("title".into(), json!(title));
                }
                if changes.is_empty() {
                    return Err(CliError::message(
                        "update requires at least one field change",
                    ));
                }
                let operation = operations::update(
                    Uuid::new_v4().to_string(),
                    &Record::Stream(stream.clone()),
                    &changes,
                    Actor::User,
                    None,
                    vec![],
                )?;
                let saved = save(opened.backend(), &operation)?;
                finish(opened.backend(), &operation, saved, SyncEvent::Stream, None)
            }
            Command::Archive { target } => self.archive(&target, true),
            Command::Restore { target } => self.archive(&target, false),
            Command::Check { reference } => {
                let opened = application::open(&self.config)?;
                let catalog = opened.backend().snapshot()?.catalog;
                let stream = reference
                    .as_deref()
                    .map(|reference| {
                        resolve(&catalog.streams, reference, |s| (&s.id, &s.title, None))
                    })
                    .transpose()?;
                Ok(Output::value(
                    json!({"valid":true,"streams":if stream.is_some(){1}else{catalog.streams.len()},"sessions":catalog.sessions.iter().filter(|session|stream.is_none_or(|stream|session.stream_id==stream.id)).count()}),
                ))
            }
            Command::Sync { event } => {
                let opened = application::open(&self.config)?;
                let result = opened.backend().sync(event);
                let ok = !result.needs_attention();
                Ok(Output::receipt(result_data(&result)?, ok))
            }
            Command::Session { command } => self.session(command),
            Command::Checkout { command } => {
                let opened = application::open(&self.config)?;
                let registry = self.registry(&opened)?;
                let value = match command {
                    CheckoutCommand::List => json!({"checkouts":registry.list()?}),
                    CheckoutCommand::Register {
                        directory,
                        repository,
                    } => {
                        json!({"checkout":registry.register(&self.path(&directory)?,repository.as_deref())?})
                    }
                    CheckoutCommand::Show { checkout_id } => {
                        json!({"checkout":registry.get(&checkout_id)?})
                    }
                    CheckoutCommand::Move {
                        checkout_id,
                        directory,
                    } => {
                        json!({"checkout":registry.move_checkout(&checkout_id,&self.path(&directory)?)?})
                    }
                    CheckoutCommand::Default {
                        repository,
                        checkout_id,
                    } => {
                        registry.set_default(&repository, &checkout_id)?;
                        json!({"repository":repository,"checkout_id":checkout_id})
                    }
                    CheckoutCommand::Resolve { repository } => {
                        json!({"checkout":registry.resolve(&repository,None,None)?})
                    }
                };
                Ok(Output::value(value))
            }
            Command::Agent { command } => self.agent(command),
        }
    }
    fn archive(&self, target: &Target, archived: bool) -> Result<Output> {
        let opened = application::open(&self.config)?;
        let catalog = opened.backend().snapshot()?.catalog;
        let stream = self.target(&catalog, target)?;
        let operation = operations::update(
            Uuid::new_v4().to_string(),
            &Record::Stream(stream.clone()),
            json!({"archived":archived})
                .as_object()
                .expect("archive fields"),
            Actor::User,
            None,
            vec![],
        )?;
        let saved = save(opened.backend(), &operation)?;
        finish(opened.backend(), &operation, saved, SyncEvent::Stream, None)
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
                        resolve(&catalog.streams, reference, |s| (&s.id, &s.title, None))
                    })
                    .transpose()?;
                let sessions: Vec<_> = catalog
                    .sessions
                    .iter()
                    .filter(|session| stream.is_none_or(|stream| session.stream_id == stream.id))
                    .collect();
                Ok(Output::value(json!({"sessions":sessions})))
            }
            SessionCommand::Show { reference } => Ok(Output::value(
                json!({"session":resolve(&catalog.sessions,&reference,|s|(&s.id,&s.title,Some(&s.host_id)))?}),
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
                let stream = self.target(&catalog, &target)?;
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
                    Uuid::new_v4().to_string(),
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
                let before = resolve(&catalog.sessions, &reference, |s| {
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
                let session = resolve(&catalog.sessions, &reference, |s| {
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
                    let alias = repository.as_deref().or_else(|| {
                        session
                            .checkout
                            .as_ref()
                            .map(|checkout| checkout.repository.as_str())
                    });
                    let url = alias
                        .map(|alias| {
                            stream
                                .repositories
                                .get(alias)
                                .map(|r| r.url.as_str())
                                .ok_or_else(|| {
                                    CliError::message(format!(
                                        "unknown repository alias: {alias:?}"
                                    ))
                                })
                        })
                        .transpose()?;
                    if let (Some(selected), Some(portable)) =
                        (repository.as_deref(), session.checkout.as_ref())
                        && selected != portable.repository
                    {
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
    fn agent(&self, command: AgentCommand) -> Result<Output> {
        let opened = application::open(&self.config)?;
        match command {
            AgentCommand::Apply => {
                let source = io::read_to_string(io::stdin())?;
                let operation = agent::parse_update(&source, &opened.source_id()?)?;
                let saved = save(opened.backend(), &operation)?;
                let event = if matches!(operation.after(), Record::Session(_)) {
                    SyncEvent::Session
                } else {
                    SyncEvent::Stream
                };
                finish(opened.backend(), &operation, saved, event, None)
            }
            AgentCommand::Context {
                harness,
                session_id,
                stream_id,
                attest,
            } => {
                if self.selection(None)?.resolved.is_some() {
                    return Err(CliError::message(
                        "native caller association inspection is not enabled in this Rust slice",
                    ));
                }
                let source = opened.source_id()?;
                let context = agent::context(
                    opened.backend(),
                    opened.host().as_str(),
                    &source,
                    ContextRequest {
                        harness,
                        native_id: &session_id,
                        stream_id: stream_id.as_deref(),
                        environment_stream_id: self
                            .environment
                            .get("STREAMS_STREAM_ID")
                            .map(String::as_str),
                        workspace_stream_id: None,
                    },
                )
                .map_err(|error| CliError::message(error.to_string()))?;
                if attest {
                    return Err(CliError::message(
                        if context.stream.is_none() || context.session.is_none() {
                            "native projection requires an enrolled Stream and Session"
                        } else {
                            "native projection attestation is not enabled in this Rust slice"
                        },
                    ));
                }
                let mut value = serde_json::to_value(&context)?;
                let document_context =
                    if let (Some(stream), Some(session)) = (&context.stream, &context.session) {
                        documents::context(
                            &self.registry(&opened)?,
                            &self.locations(&opened)?,
                            stream,
                            session,
                        )
                    } else {
                        json!({"checkouts":[],"document_locations":[]})
                    };
                value.as_object_mut().expect("context object").extend(
                    document_context
                        .as_object()
                        .expect("document context object")
                        .clone(),
                );
                Ok(Output::value(value))
            }
        }
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
fn catalog_data(catalog: &Catalog) -> Value {
    json!({"streams":catalog.streams,"sessions":catalog.sessions})
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
        .and_then(|domain| domain.record.as_ref())
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
    if let Some(Value::Object(extra)) = extra {
        payload
            .as_object_mut()
            .expect("receipt object")
            .extend(extra);
    }
    Ok(Output::receipt(payload, ok))
}
fn binding_data(binding: &runtime::DirectoryBinding) -> Value {
    json!({"cwd":binding.cwd,"checkout_id":binding.checkout.as_ref().map(|c|&c.id),"relative_cwd":binding.checkout.as_ref().map(|c|&c.relative_cwd),"repository_url":binding.checkout.as_ref().and_then(|c|c.repository_url.as_ref())})
}
