use std::path::PathBuf;

use clap::{ArgAction, ArgGroup, Args, Parser, Subcommand};
use streams::config::{CardMode, ConfigKey, ConfigPath, Strategy};
use streams::frontend::FrontendRequest;
use streams::model::{NamingMode, Status, TitleMode};
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
pub(super) enum CardCommand {
    /// Run the classic Herdr work card pane.
    Pane,
    /// Watch a Pi conversation and manage its classic Herdr card until stdin ends.
    Watch {
        #[arg(value_name = "HARNESS/ID")]
        session: String,
        #[arg(long)]
        plugin_root: PathBuf,
    },
    /// Open the classic Herdr card beside the caller's Pi pane and save automatic mode.
    Open {
        #[arg(value_name = "HARNESS/ID")]
        session: String,
        #[arg(long)]
        plugin_root: Option<PathBuf>,
    },
    /// Close the classic Herdr card beside the caller's Pi pane and save manual mode.
    Close {
        #[arg(value_name = "HARNESS/ID")]
        session: String,
    },
    /// Show or set a conversation's card auto-open mode (automatic or manual).
    Mode {
        #[arg(value_name = "HARNESS/ID")]
        session: String,
        mode: Option<CardMode>,
    },
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
    /// Inspect and replay private, frozen helper requests without live catalog writes.
    #[command(disable_help_flag = true)]
    Debug {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
    /// Manage classic Herdr work cards.
    Card {
        #[command(subcommand)]
        command: CardCommand,
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
    Update {
        #[command(flatten)]
        target: Target,
        #[arg(long)]
        title: Option<String>,
        #[command(flatten)]
        fields: StreamFields,
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
    /// Store or read named Session outputs in the catalog.
    Artifact {
        #[command(subcommand)]
        command: ArtifactCommand,
    },
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
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
    },
    Apply,
    /// Enroll the exact conversation without launching native resources.
    Enroll,
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
    fn targets_remain_runtime_decisions_and_frontends_are_closed() {
        assert!(parse(&["show"]).is_ok());
        let parsed = parse(&["show", "effort", "-c", "-f", "hc"]).unwrap();
        assert!(
            matches!(parsed.command, Command::Show { target: Target { reference: Some(reference), current: true, frontend: Some(frontend) }, .. } if reference == "effort" && frontend == FrontendRequest::Explicit(streams::frontend::FrontendId::Hc))
        );
        assert!(parse(&["show", "effort", "-f", "new-adapter"]).is_err());
    }

    #[test]
    fn all_frontend_overrides_are_validated_before_dispatch() {
        for command in ["show", "update"] {
            for invalid in ["", "  ", "adapter\n"] {
                assert!(parse(&[command, "effort", "--frontend", invalid]).is_err());
            }
        }
        // Retired commands fail with parser errors.
        for retired in [
            "close",
            "archive",
            "restore",
            "open",
            "start",
            "resume",
            "serve",
            "integration",
            "native",
            "frontend",
            "reference",
            "checkout",
        ] {
            assert!(parse(&[retired]).is_err(), "{retired}");
        }
        assert!(parse(&["create", "Effort", "--status", "active"]).is_err());
        assert!(parse(&["update", "effort", "--status", "done"]).is_err());
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
    fn card_commands_parse_harness_id_and_mode() {
        assert!(parse(&["card", "pane"]).is_ok());
        assert!(parse(&["card", "watch", "pi/abc", "--plugin-root", "/tmp/plugin"]).is_ok());
        assert!(parse(&["card", "open", "pi/abc"]).is_ok());
        assert!(parse(&["card", "close", "pi/abc"]).is_ok());
        assert!(parse(&["card", "mode", "pi/abc"]).is_ok());
        assert!(parse(&["card", "mode", "pi/abc", "manual"]).is_ok());
        assert!(parse(&["card", "mode", "pi/abc", "automatic"]).is_ok());
        assert!(parse(&["card", "mode", "pi/abc", "inherit"]).is_err());
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
