//! Entity-organized CLI command tree and argument validation.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use streams::{
    config::{CardMode, Strategy},
    model::{NamingMode, ReferenceRole, Status},
};

#[derive(Debug, Parser)]
#[command(
    name = "streams",
    about = "Manage Streams, Sessions, Artifacts, Config and the Catalog",
    subcommand_required = true,
    arg_required_else_help = true
)]
pub(super) struct Cli {
    /// Emit structured JSON instead of plain text
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// List Streams (alias for `stream list`)
    List(StreamListArgs),

    /// Resume a Session in this terminal (alias for `session resume`)
    Resume {
        /// Session full key, HARNESS/ID, ID, or unique ID prefix
        session: String,
    },

    /// Manage Streams
    Stream {
        #[command(subcommand)]
        command: StreamCommand,
    },

    /// Manage Sessions
    Session {
        #[command(subcommand)]
        command: Box<SessionCommand>,
    },

    /// Manage Session artifacts
    Artifact {
        #[command(subcommand)]
        command: ArtifactCommand,
    },

    /// Read or change host-local configuration
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// Initialize, sync, check or tidy the catalog
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },

    /// Classic Herdr work-card commands
    #[command(hide = true)]
    Card {
        #[command(subcommand)]
        command: CardCommand,
    },

    /// Developer debugging tools
    #[command(hide = true)]
    Debug {
        #[command(subcommand)]
        command: DebugCommand,
    },
}

#[derive(Debug, Args, Clone)]
pub(super) struct ListOutputArgs {
    /// Comma-separated field names to include (key is always included)
    #[arg(long)]
    pub fields: Option<String>,

    /// Maximum number of items to return
    #[arg(long)]
    pub limit: Option<usize>,
}

#[derive(Debug, Args, Clone)]
pub(super) struct ShowOutputArgs {
    /// Comma-separated field names to include (key is always included)
    #[arg(long)]
    pub fields: Option<String>,
}

#[derive(Debug, Args, Clone)]
pub(super) struct StreamListArgs {
    /// Substring filter against title and goal
    pub query: Option<String>,

    /// Filter by derived Stream status (repeatable)
    #[arg(long, value_parser = parse_status)]
    pub status: Vec<Status>,

    /// Include closed Streams
    #[arg(long)]
    pub all: bool,

    /// Keep only Streams owned by HOST
    #[arg(long, conflicts_with = "local")]
    pub host: Option<String>,

    /// Keep only Streams owned by this host
    #[arg(long, conflicts_with = "host")]
    pub local: bool,

    #[command(flatten)]
    pub output: ListOutputArgs,
}

#[derive(Debug, Args, Clone)]
pub(super) struct StreamCreateFieldArgs {
    /// Stream title
    #[arg(long)]
    pub title: String,

    /// Whether automatic reports may update the title (manual or automatic)
    #[arg(long, value_parser = parse_naming_mode)]
    pub title_mode: Option<NamingMode>,

    /// Stream goal text
    #[arg(long, conflicts_with_all = ["goal_file", "clear_goal"])]
    pub goal: Option<String>,

    /// Read Stream goal text from FILE
    #[arg(long, conflicts_with_all = ["goal", "clear_goal"])]
    pub goal_file: Option<PathBuf>,

    /// Clear the Stream goal
    #[arg(long, conflicts_with_all = ["goal", "goal_file", "goal_short", "goal_mode"])]
    pub clear_goal: bool,

    /// Short goal for narrow displays (only with --goal or --goal-file)
    #[arg(long, help_heading = "Advanced")]
    pub goal_short: Option<String>,

    /// Whether automatic reports may update the goal (manual or automatic)
    #[arg(long, value_parser = parse_naming_mode, conflicts_with = "clear_goal")]
    pub goal_mode: Option<NamingMode>,
}

#[derive(Debug, Args, Clone, Default)]
pub(super) struct StreamEditFieldArgs {
    /// Set Stream title
    #[arg(long)]
    pub title: Option<String>,

    /// Set title manual mark (manual or automatic)
    #[arg(long, value_parser = parse_naming_mode)]
    pub title_mode: Option<NamingMode>,

    /// Set Stream goal text
    #[arg(long, conflicts_with_all = ["goal_file", "clear_goal"])]
    pub goal: Option<String>,

    /// Read Stream goal text from FILE
    #[arg(long, conflicts_with_all = ["goal", "clear_goal"])]
    pub goal_file: Option<PathBuf>,

    /// Clear the Stream goal
    #[arg(long, conflicts_with_all = ["goal", "goal_file", "goal_short", "goal_mode"])]
    pub clear_goal: bool,

    /// Short goal for narrow displays (only with --goal or --goal-file)
    #[arg(long, help_heading = "Advanced")]
    pub goal_short: Option<String>,

    /// Set goal manual mark (manual or automatic)
    #[arg(long, value_parser = parse_naming_mode, conflicts_with = "clear_goal")]
    pub goal_mode: Option<NamingMode>,
}

impl StreamEditFieldArgs {
    pub fn any_set(&self) -> bool {
        self.title.is_some()
            || self.title_mode.is_some()
            || self.goal.is_some()
            || self.goal_file.is_some()
            || self.clear_goal
            || self.goal_short.is_some()
            || self.goal_mode.is_some()
    }
}

#[derive(Debug, Subcommand)]
pub(super) enum StreamCommand {
    /// List Streams
    List(StreamListArgs),

    /// Show one Stream and its computed fields
    Show {
        /// Stream full key, ID, or unique ID prefix
        stream: String,

        #[command(flatten)]
        output: ShowOutputArgs,
    },

    /// Create a new Stream
    Create {
        #[command(flatten)]
        fields: StreamCreateFieldArgs,
    },

    /// Edit a Stream using flags or a JSON request on stdin
    Edit {
        /// Stream full key, ID, or unique ID prefix
        stream: String,

        /// Read a JSON edit request (`{"changes": ..., "before": ...}`) from stdin (`-`)
        #[arg(long)]
        input: Option<String>,

        #[command(flatten)]
        fields: StreamEditFieldArgs,
    },

    /// Delete an empty Stream owned by this host
    Delete {
        /// Stream full key, ID, or unique ID prefix
        stream: String,
    },
}

#[derive(Debug, Args, Clone, Default)]
pub(super) struct SessionProgressArgs {
    /// Set Session recap text
    #[arg(long, conflicts_with_all = ["recap_file", "clear_recap"])]
    pub recap: Option<String>,

    /// Read Session recap text from FILE
    #[arg(long, conflicts_with_all = ["recap", "clear_recap"])]
    pub recap_file: Option<PathBuf>,

    /// Clear Session recap
    #[arg(long, conflicts_with_all = ["recap", "recap_file", "recap_short"])]
    pub clear_recap: bool,

    /// Short recap for narrow displays (only with --recap or --recap-file)
    #[arg(long, help_heading = "Advanced")]
    pub recap_short: Option<String>,

    /// Set Session state text
    #[arg(long, conflicts_with_all = ["state_file", "clear_state"])]
    pub state: Option<String>,

    /// Read Session state text from FILE
    #[arg(long, conflicts_with_all = ["state", "clear_state"])]
    pub state_file: Option<PathBuf>,

    /// Clear Session state
    #[arg(long, conflicts_with_all = ["state", "state_file", "state_short"])]
    pub clear_state: bool,

    /// Short state for narrow displays (only with --state or --state-file)
    #[arg(long, help_heading = "Advanced")]
    pub state_short: Option<String>,

    /// Set Session remaining work text
    #[arg(long, conflicts_with_all = ["remaining_file", "clear_remaining"])]
    pub remaining: Option<String>,

    /// Read Session remaining work text from FILE
    #[arg(long, conflicts_with_all = ["remaining", "clear_remaining"])]
    pub remaining_file: Option<PathBuf>,

    /// Clear Session remaining work
    #[arg(long, conflicts_with_all = ["remaining", "remaining_file", "remaining_short"])]
    pub clear_remaining: bool,

    /// Short remaining work for narrow displays (only with --remaining or --remaining-file)
    #[arg(long, help_heading = "Advanced")]
    pub remaining_short: Option<String>,
}

impl SessionProgressArgs {
    pub fn any_set(&self) -> bool {
        self.recap.is_some()
            || self.recap_file.is_some()
            || self.clear_recap
            || self.recap_short.is_some()
            || self.state.is_some()
            || self.state_file.is_some()
            || self.clear_state
            || self.state_short.is_some()
            || self.remaining.is_some()
            || self.remaining_file.is_some()
            || self.clear_remaining
            || self.remaining_short.is_some()
    }
}

#[derive(Debug, Args, Clone, Default)]
pub(super) struct SessionReferenceArgs {
    /// Add a pull request reference URL
    #[arg(long, conflicts_with_all = ["add_issue", "add_link", "add_document"])]
    pub add_pr: Option<String>,

    /// Add an issue reference URL
    #[arg(long, conflicts_with_all = ["add_pr", "add_link", "add_document"])]
    pub add_issue: Option<String>,

    /// Add a web link reference URL
    #[arg(long, conflicts_with_all = ["add_pr", "add_issue", "add_document"])]
    pub add_link: Option<String>,

    /// Add a document reference (path relative to Session directory, or absolute path inside it)
    #[arg(long, conflicts_with_all = ["add_pr", "add_issue", "add_link"])]
    pub add_document: Option<String>,

    /// Short display name for the added reference
    #[arg(long)]
    pub name: Option<String>,

    /// One-sentence explanation of why the added reference matters
    #[arg(long)]
    pub purpose: Option<String>,

    /// Role of the added reference (deliverable, dependency, brief, background)
    #[arg(long, value_parser = parse_reference_role)]
    pub role: Option<ReferenceRole>,

    /// Mark an existing reference key as dropped (repeatable)
    #[arg(long)]
    pub drop_reference: Vec<String>,
}

impl SessionReferenceArgs {
    pub fn any_add(&self) -> bool {
        self.add_pr.is_some()
            || self.add_issue.is_some()
            || self.add_link.is_some()
            || self.add_document.is_some()
    }

    pub fn any_set(&self) -> bool {
        self.any_add()
            || self.name.is_some()
            || self.purpose.is_some()
            || self.role.is_some()
            || !self.drop_reference.is_empty()
    }
}

#[derive(Debug, Subcommand)]
pub(super) enum SessionCommand {
    /// List Sessions
    List {
        /// Substring filter against Session title
        query: Option<String>,

        /// Filter to Sessions belonging to STREAM
        #[arg(long)]
        stream: Option<String>,

        /// Read exact Session keys (repeatable; missing keys are omitted)
        #[arg(long = "key")]
        keys: Vec<String>,

        /// Filter by Session status (repeatable)
        #[arg(long, value_parser = parse_status)]
        status: Vec<Status>,

        /// Include closed Sessions
        #[arg(long)]
        all: bool,

        #[command(flatten)]
        output: ListOutputArgs,
    },

    /// Show one Session and its computed fields
    Show {
        /// Session full key, HARNESS/ID, ID, or unique ID prefix
        session: String,

        #[command(flatten)]
        output: ShowOutputArgs,
    },

    /// Create a Session record for an existing conversation
    Create {
        /// Exact HARNESS/ID pair for this host (never a bare ID, prefix, or full key)
        session: String,

        /// Existing same-host Stream for this Session
        #[arg(long)]
        stream: String,

        /// Session title
        #[arg(long)]
        title: String,

        /// Whether automatic reports may update the title (manual or automatic)
        #[arg(long, value_parser = parse_naming_mode)]
        title_mode: Option<NamingMode>,

        /// Initial Session status
        #[arg(long, value_parser = parse_status)]
        status: Option<Status>,

        /// Working directory for this Session (inspected once for Git/jj metadata)
        #[arg(long)]
        directory: Option<PathBuf>,

        #[command(flatten)]
        progress: SessionProgressArgs,
    },

    /// Edit a Session using flags or a JSON request on stdin
    Edit {
        /// Session full key, HARNESS/ID, ID, or unique ID prefix
        session: String,

        /// Read a JSON edit request (`{"changes": ..., "before": ...}`) from stdin (`-`)
        #[arg(long)]
        input: Option<String>,

        /// Set Session title
        #[arg(long)]
        title: Option<String>,

        /// Set title manual mark (manual or automatic)
        #[arg(long, value_parser = parse_naming_mode)]
        title_mode: Option<NamingMode>,

        /// Set Session status
        #[arg(long, value_parser = parse_status)]
        status: Option<Status>,

        /// Move Session to another same-host Stream
        #[arg(long)]
        stream: Option<String>,

        /// Set Session directory (canonicalized and inspected once for Git/jj metadata)
        #[arg(long)]
        directory: Option<PathBuf>,

        #[command(flatten)]
        progress: SessionProgressArgs,

        #[command(flatten)]
        references: SessionReferenceArgs,
    },

    /// Resume a Session's harness in its recorded directory
    Resume {
        /// Session full key, HARNESS/ID, ID, or unique ID prefix
        session: String,
    },

    /// Read or view a Session's document reference
    Document {
        #[command(subcommand)]
        command: DocumentCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum DocumentCommand {
    /// Print a document reference's exact bytes to stdout
    Read {
        /// Session full key, HARNESS/ID, ID, or unique ID prefix
        session: String,
        /// Relative document path key on the Session
        path: String,
    },

    /// Open a document reference in the configured terminal viewer
    View {
        /// Session full key, HARNESS/ID, ID, or unique ID prefix
        session: String,
        /// Relative document path key on the Session
        path: String,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum ArtifactCommand {
    /// List artifacts for a Session or Stream
    List {
        /// List artifacts belonging to SESSION
        #[arg(long, conflicts_with = "stream", required_unless_present = "stream")]
        session: Option<String>,

        /// List artifacts across all Sessions in STREAM
        #[arg(long, conflicts_with = "session", required_unless_present = "session")]
        stream: Option<String>,

        #[command(flatten)]
        output: ListOutputArgs,
    },

    /// Print an artifact's exact bytes to stdout
    Read {
        /// Artifact key (HOST/HARNESS/ID/NAME or partial with leading parts omitted)
        artifact: String,
    },

    /// Open an artifact in the configured terminal viewer
    View {
        /// Artifact key (HOST/HARNESS/ID/NAME or partial with leading parts omitted)
        artifact: String,
    },

    /// Write or update an artifact on a local Session
    Write {
        /// Artifact filename (1-64 lowercase ASCII letters, digits, `.`, `_`, `-`)
        name: String,

        /// Local Session key or selector
        #[arg(long)]
        session: String,

        /// One-line description (at most 240 characters)
        #[arg(long)]
        description: String,

        /// Read content from FILE instead of stdin
        #[arg(long, conflicts_with = "description_only")]
        file: Option<PathBuf>,

        /// Update only the description without replacing content
        #[arg(long, conflicts_with = "file")]
        description_only: bool,
    },

    /// Delete an artifact from a local Session
    Delete {
        /// Artifact key (HOST/HARNESS/ID/NAME or partial with leading parts omitted)
        artifact: String,

        /// Required confirmation to delete the artifact
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum ConfigCommand {
    /// Show effective configuration, one key, or configured overrides
    Get {
        /// Setting key or section name
        #[arg(conflicts_with = "overrides")]
        key: Option<String>,

        /// Show only values explicitly configured in config.toml
        #[arg(long, conflicts_with = "key")]
        overrides: bool,
    },

    /// Set a configuration key
    Set { key: String, value: String },

    /// Remove a configured override and restore its default
    Unset { key: String },

    /// Print the config.toml file path
    Path,
}

#[derive(Debug, Subcommand)]
pub(super) enum CatalogCommand {
    /// Initialize a new empty catalog
    Init {
        /// Sync backend: none or git
        #[arg(long, value_parser = parse_strategy)]
        sync: Strategy,

        /// Git remote URL (optional when --sync git; omit for local-only commits)
        #[arg(long)]
        remote: Option<String>,

        /// Git branch name
        #[arg(long, default_value = "main")]
        branch: String,

        /// Human-readable label for this host (defaults to the machine's hostname)
        #[arg(long)]
        label: Option<String>,
    },

    /// Show or update this host's human-readable label in hosts/<host>/host.json
    Label {
        /// New label for this host (omit to show the current label)
        label: Option<String>,
    },

    /// Configure sync transport on an existing catalog (none -> git, or update git remote)
    Configure {
        /// Sync backend: none or git
        #[arg(long, value_parser = parse_strategy)]
        sync: Strategy,

        /// Set or update the Git remote URL
        #[arg(long, conflicts_with = "clear_remote")]
        remote: Option<String>,

        /// Remove the configured Git remote (keep local-only Git commits)
        #[arg(long, conflicts_with = "remote")]
        clear_remote: bool,

        /// Git branch name (defaults to main when enabling git)
        #[arg(long)]
        branch: Option<String>,
    },

    /// Synchronize the catalog with its configured sync backend
    Sync {
        /// Non-blocking background sync invoked by automatic post-write or lifecycle triggers
        #[arg(long, hide = true)]
        background: bool,

        /// Exit cleanly without scanning when the catalog is uninitialized or uses `--sync none`
        #[arg(long, hide = true)]
        if_configured: bool,

        /// Total deadline for the sync operation in seconds
        #[arg(long)]
        timeout: Option<f64>,
    },

    /// Check catalog records and sync state
    Check {
        /// Optional Stream key or selector to focus on
        stream: Option<String>,
    },

    /// Close stale open Sessions untouched past lifecycle.decay_days
    Tidy {
        /// Override lifecycle.decay_days for this run
        #[arg(long)]
        days: Option<u32>,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum CardCommand {
    /// Run the classic Herdr work-card TUI pane
    Pane,

    /// Watch a Pi Session and project its classic Herdr work card
    Watch {
        /// Exact pi/ID conversation selector
        session: String,

        /// Classic Herdr work-cards plugin directory
        #[arg(long)]
        plugin_root: PathBuf,
    },

    /// Set card mode to automatic and open the classic Herdr card
    Open {
        /// Exact pi/ID conversation selector
        session: String,

        /// Classic Herdr work-cards plugin directory
        #[arg(long)]
        plugin_root: Option<PathBuf>,
    },

    /// Set card mode to manual and close the classic Herdr card
    Close {
        /// Exact pi/ID conversation selector
        session: String,
    },

    /// Read or set classic Herdr card mode (automatic or manual)
    Mode {
        /// Exact HARNESS/ID conversation selector
        session: String,

        /// Mode to save (omit to read current mode)
        #[arg(value_parser = parse_card_mode)]
        mode: Option<CardMode>,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum DebugCommand {
    /// Forward prompt-lab commands to the Node runner
    #[command(disable_help_flag = true)]
    Prompts {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
}

fn parse_status(raw: &str) -> std::result::Result<Status, String> {
    raw.parse().map_err(|_| {
        "invalid status; expected one of: active, waiting, blocked, parked, done, closed".into()
    })
}

fn parse_naming_mode(raw: &str) -> std::result::Result<NamingMode, String> {
    raw.parse()
        .map_err(|_| "invalid mode; expected automatic or manual".into())
}

fn parse_reference_role(raw: &str) -> std::result::Result<ReferenceRole, String> {
    raw.parse().map_err(|_| {
        "invalid role; expected one of: deliverable, dependency, brief, background".into()
    })
}

fn parse_strategy(raw: &str) -> std::result::Result<Strategy, String> {
    raw.parse()
        .map_err(|_| "invalid sync strategy; expected none or git".into())
}

fn parse_card_mode(raw: &str) -> std::result::Result<CardMode, String> {
    raw.parse()
        .map_err(|_| "invalid card mode; expected automatic or manual".into())
}
