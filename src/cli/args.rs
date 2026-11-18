use std::path::PathBuf;

use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand};
use streams::config::{ConfigKey, ConfigPath, Strategy, ViewerMode};
use streams::frontend::FrontendRequest;
use streams::model::{NamingMode, ReferenceRole, ReferenceState, Status, TitleMode};
use streams::runtime::conversations::Harness;

#[derive(Debug, Parser)]
#[command(name = "streams", about = "Manage durable work streams")]
pub(super) struct Cli {
    #[arg(long, global = true)]
    pub(super) json: bool,
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum NativeCommand {
    /// Read the attested owner and lifecycle configuration without native effects.
    Observe,
    /// Open an explicitly requested display, or a single automatic display.
    Open {
        #[arg(long)]
        automatic: bool,
    },
    /// Publish bounded native hints and synchronize enabled names.
    Project,
    /// Inspect exact native evidence without creating locks or state.
    Check,
    Focus,
    Close,
    #[command(hide = true)]
    Pane,
}
#[derive(Debug, Args)]
#[group(multiple = false)]
pub(super) struct SessionSelection {
    /// Discover a member Session by ID, title or substring.
    #[arg(id = "session", long = "session")]
    pub reference: Option<String>,
    /// Read exactly one member Session, without catalog discovery.
    #[arg(id = "session_key", long = "session-key", value_name = "HOST/ID")]
    pub key: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(super) enum IntegrationCommand {
    BindSpace {
        #[arg(long)]
        space_ref: String,
        #[arg(long)]
        stream: String,
        #[arg(long)]
        scope_json: String,
    },
    ViewDocument {
        #[arg(long)]
        scope_json: String,
        #[arg(long)]
        reference_json: String,
        #[arg(long, default_value = "less")]
        mode: ViewerMode,
    },
    Describe {
        #[arg(long)]
        frontend: FrontendRequest,
    },
    Request {
        #[arg(long)]
        request_json: String,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Read saved work through the bounded version-1 integration protocol.
    Integration {
        #[command(subcommand)]
        command: IntegrationCommand,
    },
    /// Serve typed protocol-2 requests for one Pi session over stdio.
    Serve {
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        sdk_pid: Option<u32>,
        #[arg(long)]
        plugin_root: PathBuf,
    },
    /// Inspect and replay private, frozen helper requests without live catalog writes.
    #[command(disable_help_flag = true)]
    Debug {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    Init {
        #[arg(long)]
        sync: Strategy,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long, default_value = "main")]
        branch: String,
    },
    Config {
        #[arg(long)]
        overrides: bool,
        #[command(subcommand)]
        action: Option<ConfigCommand>,
    },
    Frontend {
        #[command(subcommand)]
        command: FrontendCommand,
    },
    /// Explicit opt-in Herdr card trial; never changes installed defaults.
    Native {
        #[arg(long, global = true)]
        session_id: Option<String>,
        #[arg(long, global = true)]
        plugin_root: Option<PathBuf>,
        #[arg(long, global = true)]
        state_dir: Option<PathBuf>,
        #[command(subcommand)]
        command: NativeCommand,
    },
    List {
        query: Option<String>,
        #[arg(long, action = ArgAction::Append)]
        status: Vec<Status>,
        #[arg(long, alias = "archived", conflicts_with = "all")]
        closed: bool,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        refresh: bool,
    },
    Show {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        refresh: bool,
    },
    Create {
        title: String,
        #[command(flatten)]
        fields: StreamFields,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Start {
        title: String,
        #[command(flatten)]
        fields: StreamFields,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(short = 'f', long)]
        frontend: Option<FrontendRequest>,
    },
    Open {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        session: SessionSelection,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Resume {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        session: SessionSelection,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Update {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        fields: StreamFields,
    },
    #[command(alias = "archive")]
    Close {
        #[command(flatten)]
        target: Target,
    },
    Restore {
        #[command(flatten)]
        target: Target,
    },
    Check {
        #[arg(value_name = "REF")]
        reference: Option<String>,
    },
    Tidy {
        #[arg(long)]
        days: Option<u32>,
    },
    Sync,
    /// Inspect and manage the effort's reference relationships.
    Reference {
        #[command(subcommand)]
        command: ReferenceCommand,
    },
    /// Store or read named Session outputs in the catalog.
    Artifact {
        #[command(subcommand)]
        command: ArtifactCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Checkout {
        #[command(subcommand)]
        command: CheckoutCommand,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub(super) enum ReferenceKind {
    Document,
    Pr,
    Issue,
    Link,
}

#[derive(Debug, Subcommand)]
pub(super) enum ReferenceCommand {
    List {
        stream: String,
    },
    Add {
        stream: String,
        #[command(flatten)]
        target: ReferenceSelector,
        #[command(flatten)]
        fields: ReferenceFields,
    },
    Edit {
        stream: String,
        #[command(flatten)]
        selector: ReferenceSelector,
        #[command(flatten)]
        fields: ReferenceFields,
    },
    Drop {
        stream: String,
        #[command(flatten)]
        selector: ReferenceSelector,
    },
    Restore {
        stream: String,
        #[command(flatten)]
        selector: ReferenceSelector,
        #[arg(long)]
        state: ReferenceState,
    },
    Delete {
        stream: String,
        #[command(flatten)]
        selector: ReferenceSelector,
        #[arg(long)]
        confirm: bool,
    },
}

#[derive(Debug, Default, Args)]
#[group(required = true, multiple = true)]
pub(super) struct ReferenceSelector {
    #[arg(long, allow_hyphen_values = true, conflicts_with_all = ["repository_url", "path"])]
    pub(super) url: Option<String>,
    #[arg(
        long,
        allow_hyphen_values = true,
        requires = "path",
        conflicts_with = "url"
    )]
    pub(super) repository_url: Option<String>,
    #[arg(
        long,
        allow_hyphen_values = true,
        requires = "repository_url",
        conflicts_with = "url"
    )]
    pub(super) path: Option<String>,
}

#[derive(Debug, Default, Args)]
#[group(skip)]
pub(super) struct ReferenceFields {
    #[arg(long, allow_hyphen_values = true)]
    pub(super) name: Option<String>,
    #[arg(long, allow_hyphen_values = true)]
    pub(super) purpose: Option<String>,
    #[arg(long)]
    pub(super) kind: Option<ReferenceKind>,
    #[arg(long)]
    pub(super) role: Option<ReferenceRole>,
    #[arg(long)]
    pub(super) state: Option<ReferenceState>,
    #[arg(long = "ref", allow_hyphen_values = true, conflicts_with = "clear_ref")]
    pub(super) r#ref: Option<String>,
    #[arg(long, allow_hyphen_values = true, conflicts_with = "clear_commit")]
    pub(super) commit: Option<String>,
    #[arg(
        long,
        allow_hyphen_values = true,
        conflicts_with = "clear_source_session"
    )]
    pub(super) source_session: Option<String>,
    #[arg(long)]
    pub(super) clear_ref: bool,
    #[arg(long)]
    pub(super) clear_commit: bool,
    #[arg(long)]
    pub(super) clear_source_session: bool,
    #[arg(long, allow_hyphen_values = true)]
    pub(super) target_url: Option<String>,
    #[arg(long, allow_hyphen_values = true, requires = "target_path")]
    pub(super) target_repository_url: Option<String>,
    #[arg(long, allow_hyphen_values = true, requires = "target_repository_url")]
    pub(super) target_path: Option<String>,
}

#[derive(Debug, Subcommand)]
pub(super) enum ArtifactCommand {
    /// Create or replace an output; read stdin unless --file or --description-only is supplied.
    Write {
        name: String,
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        stream_id: Option<String>,
        #[arg(long)]
        description: String,
        #[arg(long, conflicts_with = "description_only")]
        file: Option<PathBuf>,
        #[arg(long)]
        description_only: bool,
    },
    /// Emit the original UTF-8 content to stdout, without adding a newline.
    Read {
        reference: String,
    },
    /// Open an exact output in a read-only terminal view.
    View {
        reference: String,
    },
    /// List one Session's outputs or a whole Stream; never the whole catalog.
    #[command(group(ArgGroup::new("scope").args(["stream_id", "session_id"]).required(true).multiple(true)))]
    List {
        #[arg(long)]
        host: Option<String>,
        #[arg(long)]
        stream_id: Option<String>,
        #[arg(long)]
        session_id: Option<String>,
    },
    Delete {
        reference: String,
    },
}

#[derive(Debug, Args)]
pub(super) struct Target {
    #[arg(value_name = "REF")]
    pub(super) reference: Option<String>,
    #[arg(short = 'c', long)]
    pub(super) current: bool,
    #[arg(short = 'f', long)]
    pub(super) frontend: Option<FrontendRequest>,
}

#[derive(Debug, Subcommand)]
pub(super) enum ConfigCommand {
    List {
        #[arg(long)]
        overrides: bool,
    },
    Get {
        key: ConfigPath,
    },
    Set {
        key: ConfigKey,
        value: String,
    },
    Unset {
        key: ConfigKey,
    },
    Path,
}

#[derive(Debug, Subcommand)]
pub(super) enum FrontendCommand {
    Get {
        #[arg(short = 'f', long)]
        frontend: Option<FrontendRequest>,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum CheckoutCommand {
    List,
    Register {
        directory: PathBuf,
        #[arg(long)]
        repository: Option<String>,
    },
    Show {
        checkout_id: String,
    },
    Document {
        checkout_id: String,
        path: String,
        #[arg(long)]
        revision: String,
    },
    Move {
        checkout_id: String,
        directory: PathBuf,
    },
    Default {
        checkout_id: String,
        repository: String,
    },
    Resolve {
        repository: String,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum SessionCommand {
    List {
        #[arg(long)]
        stream: Option<String>,
    },
    Show {
        #[arg(value_name = "REF")]
        reference: String,
    },
    Attach {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        harness: Harness,
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        repository: Option<String>,
        #[arg(long = "ref")]
        vcs_ref: Option<String>,
        #[arg(long, default_value = ".")]
        directory_within_checkout: String,
    },
    Update {
        #[arg(value_name = "REF")]
        reference: String,
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        fields: SessionFields,
    },
    Directory {
        #[arg(value_name = "REF")]
        reference: String,
        #[arg(long, conflicts_with = "checkout_id")]
        cwd: Option<PathBuf>,
        #[arg(long = "checkout")]
        checkout_id: Option<String>,
        #[arg(long)]
        within: Option<String>,
        #[arg(long)]
        repository: Option<String>,
    },
    Lifecycle {
        action: streams::child_protocol::LifecycleAction,
        #[arg(long)]
        harness: Harness,
        #[arg(long)]
        session_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum AgentCommand {
    Context {
        #[arg(long)]
        harness: Harness,
        #[arg(long)]
        session_id: String,
        #[arg(long)]
        stream_id: Option<String>,
    },
    Apply,
    /// Enroll the exact conversation without launching native resources.
    Enroll,
    /// Simulate captured intent without opening catalog or native runtime state.
    Preview,
    /// Inspect saved replay inputs without enrolling or repairing native resources.
    CaptureInputs {
        #[arg(long, conflicts_with = "workspace_id")]
        stream_id: Option<String>,
        #[arg(long)]
        workspace_id: Option<String>,
    },
}

#[derive(Debug, Default, Args)]
#[group(skip)]
#[command(group(ArgGroup::new("goal-input").args(["goal", "goal_file", "clear_goal"])))]
pub(super) struct StreamFields {
    #[arg(long)]
    pub(super) goal: Option<String>,
    #[arg(long)]
    pub(super) goal_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) clear_goal: bool,
    #[arg(long)]
    pub(super) goal_mode: Option<NamingMode>,
    #[arg(long)]
    pub(super) title_mode: Option<TitleMode>,
    #[arg(long)]
    pub(super) status: Option<Status>,
    #[arg(long, num_args = 2, action = ArgAction::Append, value_names = ["ALIAS", "URL"])]
    pub(super) add_repository: Vec<String>,
    #[arg(long, action = ArgAction::Append, value_name = "ALIAS")]
    pub(super) remove_repository: Vec<String>,
}

#[derive(Debug, Default, Args)]
#[group(skip)]
#[command(group(ArgGroup::new("recap-input").args(["recap", "recap_file", "clear_recap"])))]
#[command(group(ArgGroup::new("recap-short-input").args(["recap_short", "recap_short_file", "clear_recap_short"])))]
#[command(group(ArgGroup::new("state-input").args(["state", "state_file", "clear_state"])))]
#[command(group(ArgGroup::new("state-short-input").args(["state_short", "state_short_file", "clear_state_short"])))]
#[command(group(ArgGroup::new("remaining-input").args(["remaining", "remaining_file", "clear_remaining"])))]
#[command(group(ArgGroup::new("remaining-short-input").args(["remaining_short", "remaining_short_file", "clear_remaining_short"])))]
pub(super) struct SessionFields {
    #[arg(long)]
    pub(super) recap: Option<String>,
    #[arg(long)]
    pub(super) recap_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) clear_recap: bool,
    #[arg(long)]
    pub(super) recap_short: Option<String>,
    #[arg(long)]
    pub(super) recap_short_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) clear_recap_short: bool,
    #[arg(long)]
    pub(super) state: Option<String>,
    #[arg(long)]
    pub(super) state_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) clear_state: bool,
    #[arg(long)]
    pub(super) state_short: Option<String>,
    #[arg(long)]
    pub(super) state_short_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) clear_state_short: bool,
    #[arg(long)]
    pub(super) remaining: Option<String>,
    #[arg(long)]
    pub(super) remaining_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) clear_remaining: bool,
    #[arg(long)]
    pub(super) remaining_short: Option<String>,
    #[arg(long)]
    pub(super) remaining_short_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) clear_remaining_short: bool,
    #[arg(long)]
    pub(super) title_mode: Option<TitleMode>,
    #[arg(long)]
    pub(super) status: Option<Status>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("streams").chain(args.iter().copied()))
    }

    #[test]
    fn grammar_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn global_json_and_init_defaults() {
        assert_eq!(
            parse(&["init"]).unwrap_err().kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        for args in [
            vec!["--json", "init", "--sync", "none"],
            vec!["init", "--sync", "none", "--json"],
        ] {
            let parsed = parse(&args).unwrap();
            assert!(parsed.json);
            assert!(
                matches!(parsed.command, Command::Init { sync: Strategy::None, branch, .. } if branch == "main")
            );
        }
    }

    #[test]
    fn list_visibility_flags_are_exclusive_and_status_is_independent() {
        let parsed = parse(&["list"]).unwrap();
        assert!(matches!(
            parsed.command,
            Command::List {
                closed: false,
                all: false,
                ..
            }
        ));
        for flag in ["--all", "--closed", "--archived"] {
            let parsed = parse(&[
                "list",
                "query",
                flag,
                "--status",
                "waiting",
                "--status",
                "blocked",
                "--refresh",
            ])
            .unwrap();
            assert!(
                matches!(parsed.command, Command::List { query: Some(query), status, closed, all, refresh: true } if query == "query" && status == [Status::Waiting, Status::Blocked] && closed == (flag != "--all") && all == (flag == "--all"))
            );
        }
        assert_eq!(
            parse(&["list", "--all", "--closed"]).unwrap_err().kind(),
            clap::error::ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn create_accepts_cwd_and_check_reference_is_optional() {
        let parsed = parse(&["create", "title", "--cwd", "/tmp/work"]).unwrap();
        assert!(
            matches!(parsed.command, Command::Create { cwd: Some(cwd), .. } if cwd.as_path() == std::path::Path::new("/tmp/work"))
        );
        let parsed = parse(&["check"]).unwrap();
        assert!(matches!(parsed.command, Command::Check { reference: None }));
        let parsed = parse(&["check", "effort"]).unwrap();
        assert!(
            matches!(parsed.command, Command::Check { reference: Some(reference) } if reference == "effort")
        );
    }

    #[test]
    fn targets_remain_runtime_decisions_and_frontends_are_open() {
        assert!(parse(&["show"]).is_ok());
        let parsed = parse(&["show", "effort", "-c", "-f", "new-adapter"]).unwrap();
        assert!(
            matches!(parsed.command, Command::Show { target: Target { reference: Some(reference), current: true, frontend: Some(frontend) }, .. } if reference == "effort" && frontend == FrontendRequest::Explicit(streams::frontend::FrontendId("new-adapter".into())))
        );
    }

    #[test]
    fn all_frontend_overrides_are_validated_before_dispatch() {
        for command in ["show", "update", "archive", "restore"] {
            for invalid in ["", "  ", "adapter\n"] {
                assert!(parse(&[command, "effort", "--frontend", invalid]).is_err());
            }
        }
        assert!(parse(&["frontend", "get", "--frontend", ""]).is_err());
        assert!(
            parse(&[
                "session",
                "attach",
                "effort",
                "--harness",
                "pi",
                "--session-id",
                "id",
                "--title",
                "Title",
                "--frontend",
                ""
            ])
            .is_err()
        );
    }

    #[test]
    fn finite_values_and_unimplemented_commands_are_rejected() {
        for args in [
            vec!["init", "--sync", "other"],
            vec!["list", "--status", "other"],
            vec!["create", "title", "--goal-mode", "other"],
            vec!["config", "get", "other"],
            vec![
                "agent",
                "context",
                "--harness",
                "other",
                "--session-id",
                "id",
            ],
            vec!["sync", "--event", "explicit"],
            vec!["sync", "--event", "start"],
            vec!["sync", "--event", "shutdown"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn collections_have_fixed_arity_and_flatten_repeated_groups() {
        for flag in [
            "--add-repository",
            "--add-document",
            "--remove-document",
            "--document-commit",
            "--document-session",
            "--add-issue",
            "--add-pr",
        ] {
            assert!(parse(&["create", "title", flag, "one"]).is_err(), "{flag}");
        }
        let parsed = parse(&[
            "create",
            "title",
            "--add-repository",
            "a",
            "url-a",
            "--add-repository",
            "b",
            "url-b",
        ])
        .unwrap();
        assert!(
            matches!(parsed.command, Command::Create { fields, .. } if fields.add_repository == ["a", "url-a", "b", "url-b"])
        );
    }

    #[test]
    fn retired_document_added_at_flag_is_rejected() {
        for command in ["create", "update"] {
            assert!(
                parse(&[command, "work", "--document-added-at", "repo", "doc", "sha"]).is_err()
            );
        }
    }

    #[test]
    fn retired_reference_collection_flags_are_rejected() {
        for command in ["create", "update"] {
            for flag in [
                "--add-document",
                "--remove-document",
                "--document-commit",
                "--document-session",
                "--add-issue",
                "--remove-issue",
                "--add-pr",
                "--remove-pr",
            ] {
                assert!(
                    parse(&[command, "work", flag, "url", "description"]).is_err(),
                    "{flag}"
                );
            }
        }
    }

    #[test]
    fn reference_selectors_and_recovery_flags_have_exact_grammar() {
        for arguments in [
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--name",
                "one",
                "--name",
                "two",
            ],
            vec!["reference", "drop", "work"],
            vec!["reference", "drop", "work", "--repository-url", "repo"],
            vec!["reference", "drop", "work", "--path", "doc"],
            vec![
                "reference",
                "drop",
                "work",
                "--url",
                "https://example.test/item",
                "--repository-url",
                "repo",
                "--path",
                "doc",
            ],
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--target-path",
                "doc",
            ],
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--target-repository-url",
                "repo",
            ],
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--ref",
                "one",
                "--clear-ref",
            ],
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--commit",
                "one",
                "--clear-commit",
            ],
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--source-session",
                "one",
                "--clear-source-session",
            ],
            vec![
                "reference",
                "restore",
                "work",
                "--url",
                "https://example.test/item",
            ],
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--kind",
                "PR",
            ],
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--role",
                "Brief",
            ],
            vec![
                "reference",
                "edit",
                "work",
                "--url",
                "https://example.test/item",
                "--state",
                "Active",
            ],
        ] {
            assert!(parse(&arguments).is_err(), "{arguments:?}");
        }
        for flag in [
            "--url",
            "--name",
            "--purpose",
            "--kind",
            "--role",
            "--state",
            "--ref",
            "--commit",
            "--source-session",
            "--target-url",
        ] {
            let value = match flag {
                "--kind" => "pr",
                "--role" => "brief",
                "--state" => "active",
                _ => "https://example.test/item",
            };
            let mut arguments = vec!["reference", "edit", "work"];
            if flag != "--url" {
                arguments.extend(["--url", "https://example.test/item"]);
            }
            arguments.extend([flag, value, flag, value]);
            assert!(parse(&arguments).is_err(), "{flag}");
        }
        let parsed = parse(&[
            "reference",
            "edit",
            "work",
            "--repository-url=--",
            "--path",
            "--",
            "--name",
            "-name",
            "--purpose",
            "--",
            "--ref",
            "-ref",
            "--commit",
            "-commit",
            "--source-session",
            "-source",
            "--target-repository-url",
            "-repo",
            "--target-path",
            "-path",
        ])
        .unwrap();
        let Command::Reference {
            command: ReferenceCommand::Edit {
                selector, fields, ..
            },
        } = parsed.command
        else {
            panic!("edit")
        };
        assert_eq!(selector.path.as_deref(), Some("--"));
        assert_eq!(fields.name.as_deref(), Some("-name"));
        assert_eq!(fields.purpose.as_deref(), Some("--"));
        assert_eq!(fields.r#ref.as_deref(), Some("-ref"));
        assert_eq!(fields.target_path.as_deref(), Some("-path"));
    }

    #[test]
    fn every_reference_flag_is_single_use() {
        for (flag, value) in [
            ("--repository-url", "repo"),
            ("--path", "doc"),
            ("--name", "Name"),
            ("--purpose", "Purpose"),
            ("--kind", "document"),
            ("--role", "brief"),
            ("--state", "active"),
            ("--ref", "topic"),
            ("--commit", "commit"),
            ("--source-session", "source"),
            ("--target-url", "https://example.test/item"),
            ("--target-repository-url", "other"),
            ("--target-path", "other.md"),
        ] {
            let mut arguments = vec![
                "reference",
                "edit",
                "work",
                "--repository-url",
                "repo",
                "--path",
                "doc",
            ];
            if flag == "--target-repository-url" {
                arguments.extend(["--target-path", "other.md"]);
            }
            if flag == "--target-path" {
                arguments.extend(["--target-repository-url", "other"]);
            }
            arguments.extend([flag, value, flag, value]);
            assert_eq!(
                parse(&arguments).unwrap_err().kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{flag}"
            );
        }
        for flag in ["--clear-ref", "--clear-commit", "--clear-source-session"] {
            assert_eq!(
                parse(&[
                    "reference",
                    "edit",
                    "work",
                    "--url",
                    "https://example.test/item",
                    flag,
                    flag
                ])
                .unwrap_err()
                .kind(),
                clap::error::ErrorKind::ArgumentConflict,
                "{flag}"
            );
        }
        for arguments in [
            vec![
                "reference",
                "drop",
                "work",
                "--url",
                "https://example.test/item",
                "--url",
                "https://example.test/item",
            ],
            vec![
                "reference",
                "delete",
                "work",
                "--url",
                "https://example.test/item",
                "--confirm",
                "--confirm",
            ],
        ] {
            assert_eq!(
                parse(&arguments).unwrap_err().kind(),
                clap::error::ErrorKind::ArgumentConflict
            );
        }
    }

    #[test]
    fn text_alternatives_conflict_but_modes_do_not() {
        for args in [
            vec!["create", "title", "--goal", "text", "--goal-file", "file"],
            vec!["create", "title", "--goal", "text", "--clear-goal"],
            vec![
                "session",
                "update",
                "id",
                "--state-file",
                "file",
                "--state",
                "text",
            ],
            vec![
                "session",
                "update",
                "id",
                "--remaining",
                "text",
                "--remaining-file",
                "file",
            ],
            vec![
                "session",
                "directory",
                "id",
                "--cwd",
                "/tmp",
                "--checkout",
                "id",
            ],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
        assert!(
            parse(&[
                "create",
                "title",
                "--goal",
                "text",
                "--goal-mode",
                "automatic"
            ])
            .is_ok()
        );
        assert!(
            parse(&[
                "session",
                "update",
                "id",
                "--state-file",
                "-",
                "--remaining-file",
                "-"
            ])
            .is_ok()
        );
    }

    #[test]
    fn session_text_and_short_forms_have_exclusive_text_file_and_clear_flags() {
        for field in [
            "recap",
            "recap-short",
            "state",
            "state-short",
            "remaining",
            "remaining-short",
        ] {
            let flag = format!("--{field}");
            let file = format!("--{field}-file");
            let clear = format!("--clear-{field}");
            assert!(parse(&["session", "update", "id", &flag, "text"]).is_ok());
            assert!(parse(&["session", "update", "id", &file, "-"]).is_ok());
            assert!(parse(&["session", "update", "id", &clear]).is_ok());
            assert!(parse(&["session", "update", "id", &flag, "text", &file, "-"]).is_err());
            assert!(parse(&["session", "update", "id", &flag, "text", &clear]).is_err());
            assert!(parse(&["session", "update", "id", &file, "-", &clear]).is_err());
        }
    }

    #[test]
    fn session_rejects_flags_absent_from_existing_grammar() {
        for flag in [
            "--recap-mode",
            "--state-mode",
            "--remaining-mode",
            "--checkout-repository",
            "--checkout-ref",
            "--checkout-within",
            "--remove-checkout",
        ] {
            assert_eq!(
                parse(&["session", "update", "id", flag])
                    .unwrap_err()
                    .kind(),
                clap::error::ErrorKind::UnknownArgument,
                "{flag}"
            );
        }
        assert!(parse(&["session", "list", "id"]).is_err());
        assert!(parse(&["session", "list", "--refresh"]).is_err());
        assert!(parse(&["session", "show", "id", "--refresh"]).is_err());
        assert!(
            parse(&[
                "session",
                "attach",
                "id",
                "--harness",
                "pi",
                "--session-id",
                "native"
            ])
            .is_err()
        );
        for flag in ["--native-session", "--within"] {
            assert!(
                parse(&[
                    "session",
                    "attach",
                    "id",
                    "--harness",
                    "pi",
                    "--session-id",
                    "native",
                    "--title",
                    "Contribution",
                    flag,
                    "value"
                ])
                .is_err(),
                "{flag}"
            );
        }
    }

    #[test]
    fn open_and_resume_keep_human_references_separate_from_exact_session_keys() {
        for command in ["open", "resume"] {
            for selection in ["--session", "--session-key"] {
                assert!(parse(&[command, "--current", selection, "host/id"]).is_ok());
                assert!(parse(&[command, "Work", selection, "host/id"]).is_ok());
            }
            assert!(
                parse(&[
                    command,
                    "--current",
                    "--session",
                    "Lane",
                    "--session-key",
                    "host/id"
                ])
                .is_err()
            );
        }
    }

    #[test]
    fn session_list_and_attach_preserve_existing_defaults() {
        let parsed = parse(&["session", "list", "--stream", "effort"]).unwrap();
        assert!(
            matches!(parsed.command, Command::Session { command: SessionCommand::List { stream: Some(stream) } } if stream == "effort")
        );
        let parsed = parse(&[
            "session",
            "attach",
            "id",
            "--harness",
            "pi",
            "--session-id",
            "native",
            "--title",
            "Contribution",
        ])
        .unwrap();
        assert!(
            matches!(parsed.command, Command::Session { command: SessionCommand::Attach { directory_within_checkout, .. } } if directory_within_checkout == ".")
        );
    }

    #[test]
    fn nested_commands_preserve_python_option_and_positional_names() {
        let parsed = parse(&["checkout", "default", "checkout-id", "repository-url"]).unwrap();
        assert!(
            matches!(parsed.command, Command::Checkout { command: CheckoutCommand::Default { checkout_id, repository } } if checkout_id == "checkout-id" && repository == "repository-url")
        );
        let parsed = parse(&[
            "session",
            "attach",
            "stream",
            "--harness",
            "pi",
            "--session-id",
            "native",
            "--title",
            "Contribution",
            "--ref",
            "branch",
            "--directory-within-checkout",
            "src",
        ])
        .unwrap();
        assert!(
            matches!(parsed.command, Command::Session { command: SessionCommand::Attach { harness: Harness::Pi, session_id, vcs_ref: Some(vcs_ref), directory_within_checkout, title, .. } } if session_id == "native" && vcs_ref == "branch" && directory_within_checkout == "src" && title == "Contribution")
        );
    }
}
