use std::path::PathBuf;

use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand, builder::TypedValueParser};
use streams_temper::config::{ConfigKey, Strategy};
use streams_temper::frontend::FrontendRequest;
use streams_temper::model::{NamingMode, Status, TitleMode};
use streams_temper::runtime::conversations::Harness;
use streams_temper::sync::policy::SyncEvent;

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
    /// Offline reset of card/editor evidence; stop native writers and back up first.
    MigrateEvidence {
        /// Known v1 evidence root; never discovered from the current endpoint.
        #[arg(long)]
        legacy_state_dir: PathBuf,
    },
    /// Read the attested owner and lifecycle configuration without native effects.
    Observe,
    /// Capture the current attested owner's arrangement through the Rust runtime.
    Capture,
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
#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Serve typed protocol-2 requests for one Pi session over stdio.
    Serve {
        #[arg(long)]
        session_id: String,
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
        #[arg(long)]
        adopt: bool,
    },
    Config {
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
        #[arg(long, conflicts_with = "all")]
        archived: bool,
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
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Resume {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommand,
    },
    Update {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        fields: StreamFields,
    },
    Archive {
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
    Sync {
        #[arg(long, default_value = "explicit", value_parser = clap::builder::PossibleValuesParser::new(["explicit", "start", "shutdown"]).map(|value| value.parse::<SyncEvent>().expect("listed sync event")))]
        event: SyncEvent,
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
pub(super) enum WorkspaceCommand {
    Capture {
        #[command(flatten)]
        target: Target,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum ConfigCommand {
    Get { key: ConfigKey },
    Set { key: ConfigKey, value: String },
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
        #[arg(long)]
        attest: bool,
        #[arg(long)]
        native_inventory: bool,
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
    #[arg(long, num_args = 4, action = ArgAction::Append, value_names = ["REPOSITORY", "REF", "PATH", "DESCRIPTION"])]
    pub(super) add_document: Vec<String>,
    #[arg(long, num_args = 2, action = ArgAction::Append, value_names = ["REPOSITORY", "PATH"])]
    pub(super) remove_document: Vec<String>,
    #[arg(long, num_args = 3, action = ArgAction::Append, value_names = ["REPOSITORY", "PATH", "COMMIT"])]
    pub(super) document_commit: Vec<String>,
    #[arg(long, num_args = 3, action = ArgAction::Append, value_names = ["REPOSITORY", "PATH", "SESSION_ID"])]
    pub(super) document_session: Vec<String>,
    #[arg(long, num_args = 2, action = ArgAction::Append, value_names = ["URL", "DESCRIPTION"])]
    pub(super) add_issue: Vec<String>,
    #[arg(long, action = ArgAction::Append, value_name = "URL")]
    pub(super) remove_issue: Vec<String>,
    #[arg(long, num_args = 2, action = ArgAction::Append, value_names = ["URL", "DESCRIPTION"])]
    pub(super) add_pr: Vec<String>,
    #[arg(long, action = ArgAction::Append, value_name = "URL")]
    pub(super) remove_pr: Vec<String>,
}

#[derive(Debug, Default, Args)]
#[group(skip)]
#[command(group(ArgGroup::new("state-input").args(["state", "state_file"])))]
#[command(group(ArgGroup::new("remaining-input").args(["remaining", "remaining_file"])))]
pub(super) struct SessionFields {
    #[arg(long)]
    pub(super) state: Option<String>,
    #[arg(long)]
    pub(super) state_file: Option<PathBuf>,
    #[arg(long)]
    pub(super) remaining: Option<String>,
    #[arg(long)]
    pub(super) remaining_file: Option<PathBuf>,
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
                archived: false,
                all: false,
                ..
            }
        ));
        for flag in ["--all", "--archived"] {
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
                matches!(parsed.command, Command::List { query: Some(query), status, archived, all, refresh: true } if query == "query" && status == [Status::Waiting, Status::Blocked] && archived == (flag == "--archived") && all == (flag == "--all"))
            );
        }
        assert_eq!(
            parse(&["list", "--all", "--archived"]).unwrap_err().kind(),
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
            matches!(parsed.command, Command::Show { target: Target { reference: Some(reference), current: true, frontend: Some(frontend) }, .. } if reference == "effort" && frontend == FrontendRequest::Explicit(streams_temper::frontend::FrontendId("new-adapter".into())))
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
            vec!["sync", "--event", "force"],
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
    fn document_provenance_flags_preserve_repeated_groups() {
        let parsed = parse(&[
            "update",
            "work",
            "--document-commit",
            "repo",
            "doc",
            "first",
            "--document-commit",
            "repo",
            "doc",
            "latest",
            "--document-session",
            "repo",
            "doc",
            "source",
        ])
        .unwrap();
        let Command::Update { fields, .. } = parsed.command else {
            panic!("expected update");
        };
        assert_eq!(
            fields.document_commit,
            ["repo", "doc", "first", "repo", "doc", "latest"]
        );
        assert_eq!(fields.document_session, ["repo", "doc", "source"]);
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
    fn session_rejects_flags_absent_from_existing_grammar() {
        for flag in [
            "--clear-state",
            "--clear-remaining",
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
