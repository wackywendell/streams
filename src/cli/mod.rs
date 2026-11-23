//! Entity-organized CLI entry point, dispatch, formatting, and exit codes.

mod args;
mod artifacts;
mod cards;
mod catalog;
mod config;
mod debug;
mod file_watch;
mod pager;
mod select;
mod sessions;
mod settings;
mod stream_cmd;

use std::{
    collections::BTreeMap,
    env, fs,
    io::{self, Read, Write},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

use clap::Parser;
use serde::Serialize;
use serde_json::{Map, Value, json};
use streams::{
    application::{self, ApplicationError, EditResult, fields::format_cell},
    codec::load_json,
    config::{Config, ConfigError},
    frontend::{self, FrontendMode, FrontendRequest, FrontendSelection, FrontendSelectionError},
    model::ValidationError,
    operations::edits::EditRequest,
    runtime::{RuntimeError, hc as runtime_hc},
    storage::{StorageError, records::UnreadableRecord},
    sync::state::RepairRequired,
};

use args::{CardCommand, Cli, Command, DebugCommand};

#[derive(Debug, thiserror::Error)]
pub(super) enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{message}")]
    Ambiguous {
        message: String,
        candidates: Vec<String>,
    },
    #[error("{0}")]
    InvalidRef(String),
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl CliError {
    pub(super) fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            _ => 1,
        }
    }

    fn data(&self) -> Value {
        match self {
            Self::NotFound(message) => json!({
                "error": { "kind": "not-found", "message": message }
            }),
            Self::Ambiguous {
                message,
                candidates,
            } => json!({
                "error": {
                    "kind": "ambiguous",
                    "message": message,
                    "candidates": candidates,
                }
            }),
            Self::InvalidRef(message) => json!({
                "error": { "kind": "invalid-ref", "message": message }
            }),
            other => json!({
                "error": { "kind": "error", "message": other.to_string() }
            }),
        }
    }
}

impl From<ApplicationError> for CliError {
    fn from(error: ApplicationError) -> Self {
        match error {
            ApplicationError::NotFound(message) => Self::NotFound(message),
            ApplicationError::Invalid(message) => Self::Message(message),
            ApplicationError::Ambiguous {
                message,
                candidates,
            } => Self::Ambiguous {
                message,
                candidates,
            },
            ApplicationError::Config(e) => Self::Config(e),
            ApplicationError::Storage(e) => Self::Storage(e),
            ApplicationError::Validation(e) => Self::Validation(e),
            ApplicationError::Repair(RepairRequired(message)) => Self::Message(message),
            ApplicationError::Runtime(e) => Self::Runtime(e),
        }
    }
}

impl From<frontend::FrontendError> for CliError {
    fn from(error: frontend::FrontendError) -> Self {
        Self::Runtime(RuntimeError::Frontend(error))
    }
}

impl From<FrontendSelectionError> for CliError {
    fn from(error: FrontendSelectionError) -> Self {
        Self::Message(error.to_string())
    }
}

type Result<T> = std::result::Result<T, CliError>;

#[derive(Clone)]
enum Input {
    Stdin,
}

impl Input {
    fn read(&self) -> Result<String> {
        match self {
            Self::Stdin => {
                let mut text = String::new();
                io::stdin().read_to_string(&mut text)?;
                Ok(text)
            }
        }
    }

    fn read_bounded(&self, max_bytes: usize) -> Result<String> {
        match self {
            Self::Stdin => {
                let mut bytes = Vec::new();
                io::stdin()
                    .take((max_bytes + 1) as u64)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > max_bytes {
                    return Err(CliError::message(format!(
                        "input exceeds maximum size of {max_bytes} bytes"
                    )));
                }
                String::from_utf8(bytes).map_err(|_| CliError::message("input must be valid UTF-8"))
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct HerdrRead {
    pub client: streams::frontend::herdr::HerdrClient,
    pub snapshot: Option<streams::frontend::herdr::HerdrNativeSnapshot>,
    pub owner: Option<streams::runtime::native_cards::PiOwner>,
}

#[derive(Clone)]
pub(super) struct App {
    pub(super) config: Config,
    pub(super) environment: BTreeMap<String, String>,
    pub(super) cwd: PathBuf,
    input: Input,
    pub(super) herdr: Option<HerdrRead>,
}

pub(super) struct Output {
    pub code: i32,
    pub value: Value,
    pub plain: Option<String>,
    pub raw_bytes: Option<Vec<u8>>,
    pub silent: bool,
    pub notices: Vec<String>,
    /// Update the Herdr `streams.saved` label for local UI refresh.
    pub notify_ui: bool,
    /// Portable `hosts/` catalog record or artifact change was written (implies `notify_ui` and
    /// checks automatic background sync).
    pub wrote_catalog: bool,
}

impl Output {
    pub(super) fn value(value: Value) -> Self {
        Self {
            code: 0,
            value,
            plain: None,
            raw_bytes: None,
            silent: false,
            notices: Vec::new(),
            notify_ui: false,
            wrote_catalog: false,
        }
    }

    pub(super) fn plain(value: Value, plain: String) -> Self {
        Self {
            code: 0,
            value,
            plain: Some(plain),
            raw_bytes: None,
            silent: false,
            notices: Vec::new(),
            notify_ui: false,
            wrote_catalog: false,
        }
    }

    pub(super) fn raw_bytes(bytes: Vec<u8>) -> Self {
        Self {
            code: 0,
            value: Value::Null,
            plain: None,
            raw_bytes: Some(bytes),
            silent: false,
            notices: Vec::new(),
            notify_ui: false,
            wrote_catalog: false,
        }
    }

    pub(super) fn silent() -> Self {
        Self {
            code: 0,
            value: Value::Null,
            plain: None,
            raw_bytes: None,
            silent: true,
            notices: Vec::new(),
            notify_ui: false,
            wrote_catalog: false,
        }
    }
}

pub fn run() -> ExitCode {
    ExitCode::from(run_code() as u8)
}

fn run_code() -> i32 {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return code;
        }
    };
    let json_mode = cli.json;
    let app = match App::from_process() {
        Ok(app) => app,
        Err(error) => return print_error(&error, json_mode),
    };

    if let Command::Card {
        command: CardCommand::Watch {
            session,
            plugin_root,
        },
    } = cli.command
    {
        return match app.card_watch(&session, plugin_root) {
            Ok(()) => 0,
            Err(error) => print_error(&error, json_mode),
        };
    }

    match app.dispatch(cli.command, json_mode) {
        Ok(mut output) => {
            if (output.notify_ui || output.wrote_catalog)
                && let Some(warning) = app.notify_saved()
            {
                output.notices.push(warning);
            }
            if output.wrote_catalog
                && let Some(warning) = app.maybe_trigger_auto_sync()
            {
                output.notices.push(warning);
            }
            if let Some(bytes) = output.raw_bytes {
                if let Err(error) = io::stdout().write_all(&bytes) {
                    return print_error(&CliError::Io(error), false);
                }
                return output.code;
            }
            if !output.silent {
                if json_mode {
                    match serde_json::to_string_pretty(&output.value) {
                        Ok(text) => println!("{text}"),
                        Err(error) => return print_error(&CliError::Json(error), true),
                    }
                } else if let Some(plain) = &output.plain {
                    if !plain.is_empty() {
                        println!("{plain}");
                    }
                } else if !output.value.is_null() {
                    match serde_json::to_string_pretty(&output.value) {
                        Ok(text) => println!("{text}"),
                        Err(error) => return print_error(&CliError::Json(error), false),
                    }
                }
            }
            for notice in &output.notices {
                eprintln!("{notice}");
            }
            output.code
        }
        Err(error) => print_error(&error, json_mode),
    }
}

fn print_error(error: &CliError, json_mode: bool) -> i32 {
    let code = error.exit_code();
    if json_mode && !matches!(error, CliError::Usage(_)) {
        if let Ok(text) = serde_json::to_string_pretty(&error.data()) {
            println!("{text}");
        }
    } else {
        eprintln!("error: {error}");
    }
    code
}

impl App {
    fn from_process() -> Result<Self> {
        let cwd = env::current_dir()?;
        let mut environment = BTreeMap::new();
        for (key, value) in env::vars_os() {
            if let (Some(key), Some(value)) = (key.to_str(), value.to_str()) {
                environment.insert(key.to_owned(), value.to_owned());
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

    pub(super) fn selection(
        &self,
        configured: FrontendMode,
        requested: Option<FrontendRequest>,
    ) -> Result<FrontendSelection> {
        Ok(frontend::resolve_frontend(
            configured,
            requested,
            &self.environment,
        )?)
    }

    fn notify_saved(&self) -> Option<String> {
        match runtime_hc::notify_saved(&self.environment, &self.cwd, 2.0) {
            Ok(()) => None,
            Err(error) => Some(format!("warning: could not update hc saved label: {error}")),
        }
    }

    fn maybe_trigger_auto_sync(&self) -> Option<String> {
        let settings = self.config.load().ok()?;
        let root = &settings.catalog_root;
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let status = match streams::sync::auto_sync_status(
            root,
            settings.sync.auto_interval_minutes,
            now_ms,
        ) {
            Ok(status) => status,
            Err(error) => {
                return Some(format!(
                    "warning: could not check catalog sync schedule: {error}"
                ));
            }
        };
        let prior_attention = status
            .last_sync
            .filter(|state| state.needs_attention)
            .map(|state| {
                if state.detail.is_empty() {
                    format!("local: {}, sharing: {}", state.local, state.sharing)
                } else {
                    format!(
                        "local: {}, sharing: {}: {}",
                        state.local, state.sharing, state.detail
                    )
                }
            });
        if !status.due {
            return prior_attention.map(|detail| {
                format!(
                    "warning: last catalog sync needs attention ({detail}); run `streams catalog sync` to inspect"
                )
            });
        }
        let exe = match env::current_exe() {
            Ok(exe) => exe,
            Err(error) => {
                return Some(format!(
                    "warning: could not locate streams binary for background sync: {error}"
                ));
            }
        };
        let mut command = ProcessCommand::new(exe);
        command
            .args(["catalog", "sync", "--background"])
            .current_dir(&self.cwd)
            .env_clear()
            .envs(&self.environment)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Detach into its own session so callers waiting on pipes or process groups never block.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        if let Err(error) = command.spawn() {
            return Some(format!(
                "warning: could not start background catalog sync: {error}"
            ));
        }
        prior_attention.map(|detail| {
            format!(
                "warning: previous catalog sync needed attention ({detail}); retrying in background"
            )
        })
    }

    fn dispatch(&self, command: Command, json_mode: bool) -> Result<Output> {
        match command {
            Command::List(args) => self.stream_list(args, json_mode),
            Command::Resume { session } => self.session_resume(&session, json_mode),
            Command::Stream { command } => self.stream(command, json_mode),
            Command::Session { command } => self.session(*command, json_mode),
            Command::Artifact { command } => self.artifact(command, json_mode),
            Command::Config { command } => self.config_cmd(command),
            Command::Catalog { command } => self.catalog_cmd(command),
            Command::Card { command } => self.card(command),
            Command::Debug { command } => match command {
                DebugCommand::Prompts { arguments } => debug::forward(arguments, json_mode),
            },
        }
    }

    pub(super) fn resolve_cwd_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }

    pub(super) fn read_text_file(&self, path: &Path) -> Result<String> {
        let resolved = self.resolve_cwd_path(path);
        let text = fs::read_to_string(&resolved).map_err(|error| {
            CliError::message(format!("cannot read {}: {error}", resolved.display()))
        })?;
        Ok(text.trim_end_matches(['\r', '\n']).to_owned())
    }

    pub(super) fn read_stdin_bounded(&self, max_bytes: usize) -> Result<String> {
        self.input.read_bounded(max_bytes)
    }

    pub(super) fn read_edit_input(&self, source: &str) -> Result<EditRequest> {
        if source != "-" {
            return Err(CliError::Usage(
                "--input accepts only \"-\" (JSON edit request on stdin)".into(),
            ));
        }
        let raw = self.input.read()?;
        let parsed =
            load_json(&raw).map_err(|e| CliError::message(format!("invalid JSON: {e}")))?;
        Ok(EditRequest::parse(&parsed)?)
    }

    pub(super) fn render_list_output(
        &self,
        items: Vec<Map<String, Value>>,
        fields: &[&str],
        total: usize,
        unreadable: Vec<UnreadableRecord>,
        local_host: &str,
    ) -> Output {
        let more = items.len() < total;
        let remaining = total.saturating_sub(items.len());

        let mut lines = Vec::new();
        let header: Vec<String> = fields.iter().map(|f| f.to_ascii_uppercase()).collect();
        lines.push(header.join("  "));
        for item in &items {
            let cells: Vec<String> = fields
                .iter()
                .map(|field| {
                    let cell = item
                        .get(*field)
                        .map(|v| format_cell(field, v))
                        .unwrap_or_else(|| "-".into());
                    printable(&cell)
                })
                .collect();
            lines.push(cells.join("  "));
        }
        if more {
            lines.push(format!("{remaining} more; raise --limit"));
        }

        let json_items: Vec<Value> = items.into_iter().map(Value::Object).collect();
        let value = json!({
            "items": json_items,
            "unreadable": unreadable,
            "more": more,
        });
        let mut output = Output::plain(value, lines.join("\n"));
        output.notices.extend(
            unreadable
                .iter()
                .map(|record| format!("{record}\n  {}", record.repair)),
        );
        if unreadable.iter().any(|record| record.host != local_host) {
            output.notices.push(
                "/streams reset repairs only this host's records; repair another host's records on that host."
                    .into(),
            );
        }
        output
    }

    pub(super) fn render_show_output(&self, object: Map<String, Value>, fields: &[&str]) -> Output {
        let mut lines = Vec::new();
        for &field in fields {
            let Some(val) = object.get(field) else {
                continue;
            };
            let rendered = match val {
                Value::Array(_) | Value::Object(_)
                    if matches!(
                        field,
                        "sessions" | "artifacts" | "references" | "document_files"
                    ) =>
                {
                    serde_json::to_string_pretty(val).unwrap_or_else(|_| val.to_string())
                }
                _ => printable(&format_cell(field, val)),
            };
            lines.push(format!("{field}: {rendered}"));
        }
        Output::plain(Value::Object(object), lines.join("\n"))
    }

    pub(super) fn render_create_output(
        &self,
        key: &str,
        files: Vec<PathBuf>,
        wrote: bool,
    ) -> Output {
        let file_strings: Vec<String> = files
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let plain = if wrote {
            format!("saved {key}")
        } else {
            format!("unchanged {key}")
        };
        let mut output = Output::plain(json!({ "key": key, "files": file_strings }), plain);
        output.wrote_catalog = wrote;
        output
    }

    pub(super) fn render_edit_output(&self, result: EditResult) -> Output {
        let complete = result.is_complete();
        let value = json!({
            "key": result.key,
            "not_written": result.not_written,
        });
        let plain = if complete {
            if result.written {
                format!("saved {}", result.key)
            } else {
                format!("unchanged {}", result.key)
            }
        } else {
            let skipped: Vec<&str> = result.not_written.keys().map(String::as_str).collect();
            format!(
                "partial edit on {}; not written: {} (re-read and retry if needed)",
                result.key,
                skipped.join(", ")
            )
        };
        let mut output = Output::plain(value, plain);
        if !complete {
            output.code = 3;
        }
        output.wrote_catalog = result.written;
        output
    }
}

pub(super) fn printable(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub(super) fn result_data<T: Serialize>(value: &T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}
