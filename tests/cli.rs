//! End-to-end CLI-by-entity contract tests covering all 6 CUJs, selectors, edits,
//! documents, artifacts, config, catalog, and exit codes (0, 1, 2, 3).

use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use serde_json::{Value, json};

struct Fixture {
    directory: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        fixture.ok_json(&["catalog", "init", "--sync", "none"], None);
        fixture.ok_json(&["config", "set", "frontend.mode", "none"], None);
        fixture
    }

    fn home(&self) -> &Path {
        self.directory.path()
    }

    fn root(&self) -> PathBuf {
        self.home().join("data/streams/catalog")
    }

    fn command(&self) -> Command {
        let home = self.home();
        let mut command = Command::new(env!("CARGO_BIN_EXE_streams"));
        command
            .env_clear()
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_STATE_HOME", home.join("state"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .current_dir(home);
        command
    }

    fn run_raw(&self, argv: &[&str], input: Option<&str>) -> (i32, Vec<u8>, String) {
        self.run_with(self.command(), argv, input)
    }

    fn run_with(
        &self,
        mut command: Command,
        argv: &[&str],
        input: Option<&str>,
    ) -> (i32, Vec<u8>, String) {
        command.args(argv);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(text) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        (
            output.status.code().unwrap(),
            output.stdout,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    fn run_json(&self, argv: &[&str], input: Option<&str>) -> (i32, Value, String) {
        let mut full: Vec<&str> = argv.to_vec();
        if !full.contains(&"--json") {
            full.push("--json");
        }
        let (code, stdout, stderr) = self.run_raw(&full, input);
        let value = serde_json::from_slice(&stdout).unwrap_or_else(|e| {
            panic!(
                "{full:?} (exit {code}) invalid JSON: {e}\nstdout={}\nstderr={stderr}",
                String::from_utf8_lossy(&stdout)
            )
        });
        (code, value, stderr)
    }

    fn ok_json(&self, argv: &[&str], input: Option<&str>) -> Value {
        let (code, value, stderr) = self.run_json(argv, input);
        assert_eq!(code, 0, "{argv:?} failed: {value} stderr={stderr}");
        value
    }
}

#[test]
fn removed_commands_fail_with_parser_usage_exit_code_2() {
    let f = Fixture::new();
    for old in [
        &["open"][..],
        &["start", "Work"][..],
        &["agent", "context"][..],
        &["integration", "describe"][..],
        &["native", "pane"][..],
        &["serve"][..],
        &["reference", "list"][..],
        &["checkout", "list"][..],
        &["frontend", "get"][..],
    ] {
        let (code, _, _) = f.run_raw(old, None);
        assert_eq!(code, 2, "old command {old:?} should exit 2");
    }
}

#[test]
fn cuj1_list_filter_prefix_select_and_session_resume_runs_harness_in_directory() {
    let f = Fixture::new();
    let workdir = f.home().join("work-repo");
    fs::create_dir_all(&workdir).unwrap();

    // Fake `pi` executable on PATH that records cwd, argv, and env (verifying no STREAMS_* env set).
    let bin_dir = f.home().join("fake-bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let log_file = f.home().join("pi-invocation.txt");
    let fake_pi = bin_dir.join("pi");
    fs::write(
        &fake_pi,
        format!(
            "#!/bin/sh\npwd > \"{}\"\nprintf '%s\\n' \"$@\" >> \"{}\"\nprintf 'ENV_STREAMS=%s\\n' \"${{STREAMS_SESSION:-}}\" \"${{STREAMS_STREAM_ID:-}}\" >> \"{}\"\n",
            log_file.display(),
            log_file.display(),
            log_file.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755)).unwrap();

    let s1 = f.ok_json(&["stream", "create", "--title", "First Effort"], None);
    let s1_key = s1["key"].as_str().unwrap();
    let s1_id = s1_key.split('/').nth(1).unwrap();
    let s1_prefix = &s1_id[..8];

    let created = f.ok_json(
        &[
            "session",
            "create",
            "pi/01a1beef",
            "--stream",
            s1_prefix,
            "--title",
            "Build CLI",
            "--directory",
            workdir.to_str().unwrap(),
        ],
        None,
    );
    assert!(created["key"].as_str().unwrap().ends_with("/pi/01a1beef"));

    // Root `list` alias matches `stream list`.
    let listed = f.ok_json(&["list"], None);
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(listed["items"][0]["key"], s1_key);
    assert_eq!(listed["items"][0]["status"], "active");
    assert_eq!(listed["more"], false);

    // `session list --stream PREFIX` lists member Sessions.
    let sessions = f.ok_json(&["session", "list", "--stream", s1_prefix], None);
    assert_eq!(sessions["items"].as_array().unwrap().len(), 1);
    assert_eq!(sessions["items"][0]["key"], created["key"]);

    // `resume` rejects `--json` with exit 2 before running anything.
    let (code, _, _) = f.run_raw(&["resume", "01a1", "--json"], None);
    assert_eq!(code, 2);

    // Root `resume` runs `pi --session 01a1beef` in `directory.path` without setting env vars.
    let mut cmd = f.command();
    let path_env = format!(
        "{}:{}",
        bin_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    cmd.env("PATH", path_env);
    let (code, _, stderr) = f.run_with(cmd, &["resume", "01a1"], None);
    assert_eq!(code, 0, "stderr={stderr}");
    let logged = fs::read_to_string(&log_file).unwrap();
    let lines: Vec<&str> = logged.lines().collect();
    assert_eq!(
        Path::new(lines[0]).canonicalize().unwrap(),
        workdir.canonicalize().unwrap()
    );
    assert_eq!(lines[1], "--session");
    assert_eq!(lines[2], "01a1beef");
    assert_eq!(lines[3], "ENV_STREAMS=");
}

#[test]
fn cuj2_stream_edit_by_prefix_saves_both_fields_and_ambiguity_lists_candidates() {
    let f = Fixture::new();
    let s1 = f.ok_json(&["stream", "create", "--title", "Initial"], None);
    let s1_key = s1["key"].as_str().unwrap();
    let s1_id = s1_key.split('/').nth(1).unwrap();
    let prefix = &s1_id[..8];

    // Edit both title and goal by prefix in one save.
    let edited = f.ok_json(
        &[
            "stream",
            "edit",
            prefix,
            "--title",
            "CLI by entity",
            "--goal",
            "Ship the CLI redesign",
        ],
        None,
    );
    assert_eq!(edited["key"], s1_key);
    assert_eq!(edited["not_written"], json!({}));

    let shown = f.ok_json(&["stream", "show", s1_key], None);
    assert_eq!(
        shown["title"],
        json!({"text": "CLI by entity", "manual": true})
    );
    assert_eq!(
        shown["goal"],
        json!({"text": "Ship the CLI redesign", "short": null, "manual": true})
    );
    assert_eq!(shown["revision"], 2);

    // `--goal-mode` beside `--clear-goal` is a parser usage error (exit 2).
    let (code, _, _) = f.run_raw(
        &[
            "stream",
            "edit",
            s1_key,
            "--clear-goal",
            "--goal-mode",
            "automatic",
        ],
        None,
    );
    assert_eq!(code, 2);

    // Clear goal, then `--goal-mode` alone on null goal fails edit (exit 1).
    f.ok_json(&["stream", "edit", s1_key, "--clear-goal"], None);
    let (code, err, _) = f.run_json(&["stream", "edit", s1_key, "--goal-mode", "manual"], None);
    assert_eq!(code, 1);
    assert_eq!(err["error"]["kind"], "error");

    // Ambiguous prefix lists all matching full keys with `kind: "ambiguous"`.
    // Create two Sessions with a shared prefix `abc-` to test deterministically.
    f.ok_json(
        &[
            "session",
            "create",
            "pi/abc-one",
            "--stream",
            s1_key,
            "--title",
            "One",
        ],
        None,
    );
    f.ok_json(
        &[
            "session",
            "create",
            "pi/abc-two",
            "--stream",
            s1_key,
            "--title",
            "Two",
        ],
        None,
    );
    let (code, err, _) = f.run_json(&["session", "show", "abc-"], None);
    assert_eq!(code, 1);
    assert_eq!(err["error"]["kind"], "ambiguous");
    assert_eq!(err["error"]["candidates"].as_array().unwrap().len(), 2);
}

#[test]
fn cuj3_and_cuj6_agent_report_partial_exit_3_fields_and_not_found() {
    let f = Fixture::new();
    let s = f.ok_json(&["stream", "create", "--title", "Work"], None);
    let s_key = s["key"].as_str().unwrap();
    let sess = f.ok_json(
        &[
            "session",
            "create",
            "claude/conv-1",
            "--stream",
            s_key,
            "--title",
            "Auto Title",
            "--title-mode",
            "automatic",
        ],
        None,
    );
    let sess_key = sess["key"].as_str().unwrap();

    // CUJ 6: `--fields` returns only `key` + requested fields.
    let partial = f.ok_json(
        &["session", "show", sess_key, "--fields", "title,status"],
        None,
    );
    assert_eq!(
        partial,
        json!({
            "key": sess_key,
            "title": { "text": "Auto Title", "manual": false },
            "status": "active"
        })
    );

    // Unknown key gives structured `not-found` and exit 1.
    let (code, err, _) = f.run_json(&["session", "show", "claude/missing"], None);
    assert_eq!(code, 1);
    assert_eq!(err["error"]["kind"], "not-found");

    // Unknown `--fields` name fails listing valid field names.
    let (code, err, _) = f.run_json(&["session", "show", sess_key, "--fields", "bogus"], None);
    assert_eq!(code, 1);
    assert!(err["error"]["message"].as_str().unwrap().contains("title"));

    // CUJ 3: agent reads with `claude/conv-1`, human edits title concurrently, agent saves with `before` -> exit 3!
    let before = f.ok_json(&["session", "show", "claude/conv-1"], None);
    f.ok_json(
        &["session", "edit", sess_key, "--title", "Human Title"],
        None,
    );
    let report = json!({
        "changes": {
            "title": { "text": "Model Title" },
            "state": { "text": "Done implementing", "short": "done" }
        },
        "before": {
            "title": before["title"],
            "state": before["state"]
        }
    })
    .to_string();
    let (code, result, _) = f.run_json(
        &["session", "edit", sess_key, "--input", "-"],
        Some(&report),
    );
    assert_eq!(code, 3, "partial edit must exit 3: {result}");
    assert_eq!(
        result["not_written"]["title"],
        json!({ "text": "Human Title", "manual": true })
    );
    assert!(result["not_written"].get("state").is_none());

    // Replaying exact same request gives same result and exits 3 without bumping revision.
    let rev_before = f.ok_json(&["session", "show", sess_key], None)["revision"].clone();
    let (code2, result2, _) = f.run_json(
        &["session", "edit", sess_key, "--input", "-"],
        Some(&report),
    );
    assert_eq!(code2, 3);
    assert_eq!(result2, result);
    assert_eq!(
        f.ok_json(&["session", "show", sess_key], None)["revision"],
        rev_before
    );
}

#[test]
fn cuj4_and_cuj5_session_list_keys_references_documents_and_artifacts() {
    let f = Fixture::new();
    let workdir = f.home().join("repo");
    fs::create_dir_all(workdir.join("docs")).unwrap();
    fs::write(workdir.join("docs/plan.md"), b"# The Plan\n").unwrap();

    let s = f.ok_json(&["stream", "create", "--title", "Effort"], None);
    let s_key = s["key"].as_str().unwrap();
    let sess = f.ok_json(
        &[
            "session",
            "create",
            "pi/01a10000",
            "--stream",
            s_key,
            "--title",
            "Main",
            "--directory",
            workdir.to_str().unwrap(),
        ],
        None,
    );
    let sess_key = sess["key"].as_str().unwrap();

    // CUJ 5: add document & PR references via flags, read raw bytes via `session document read`.
    f.ok_json(
        &[
            "session",
            "edit",
            "01a1",
            "--add-document",
            "docs/plan.md",
            "--name",
            "Plan",
            "--purpose",
            "The plan",
            "--role",
            "brief",
        ],
        None,
    );
    let shown = f.ok_json(&["session", "show", sess_key], None);
    assert_eq!(shown["references"]["docs/plan.md"]["manual"], true);
    assert_eq!(
        Path::new(
            shown["document_files"]["docs/plan.md"]["path"]
                .as_str()
                .unwrap()
        )
        .canonicalize()
        .unwrap(),
        workdir.join("docs/plan.md").canonicalize().unwrap()
    );

    // `session document read` rejects `--json` (exit 2) and prints exact raw bytes without it.
    let (code, _, _) = f.run_raw(
        &[
            "session",
            "document",
            "read",
            "01a1",
            "docs/plan.md",
            "--json",
        ],
        None,
    );
    assert_eq!(code, 2);
    let (code, bytes, _) = f.run_raw(
        &["session", "document", "read", "01a1", "docs/plan.md"],
        None,
    );
    assert_eq!(code, 0);
    assert_eq!(bytes, b"# The Plan\n");

    // Dropping a reference keeps it in the map with status `dropped`.
    f.ok_json(
        &[
            "session",
            "edit",
            sess_key,
            "--drop-reference",
            "docs/plan.md",
        ],
        None,
    );
    let shown2 = f.ok_json(&["session", "show", sess_key], None);
    assert_eq!(shown2["references"]["docs/plan.md"]["status"], "dropped");

    // CUJ 4: `session list --key` omits missing keys without failing.
    let batch = f.ok_json(
        &[
            "session",
            "list",
            "--key",
            sess_key,
            "--key",
            "host/pi/does-not-exist",
            "--all",
        ],
        None,
    );
    assert_eq!(batch["items"].as_array().unwrap().len(), 1);
    assert_eq!(batch["items"][0]["key"], sess_key);

    // Artifact write/list/read/delete using new entity flags.
    let written = f.ok_json(
        &[
            "artifact",
            "write",
            "notes.md",
            "--session",
            "01a1",
            "--description",
            "Notes for work",
        ],
        Some("artifact body"),
    );
    let art_key = written["key"].as_str().unwrap();
    assert!(art_key.ends_with("/pi/01a10000/notes.md"));

    let (code, _, _) = f.run_raw(&["artifact", "read", art_key, "--json"], None);
    assert_eq!(code, 2);
    let (code, bytes, _) = f.run_raw(&["artifact", "read", "notes.md"], None);
    assert_eq!(code, 0);
    assert_eq!(bytes, b"artifact body");

    let by_stream = f.ok_json(&["artifact", "list", "--stream", s_key], None);
    assert_eq!(by_stream["items"].as_array().unwrap().len(), 1);
    assert_eq!(by_stream["items"][0]["key"], art_key);

    // `artifact delete` requires `--confirm` (exit 2 without it).
    let (code, _, _) = f.run_raw(&["artifact", "delete", art_key], None);
    assert_eq!(code, 2);
    f.ok_json(&["artifact", "delete", art_key, "--confirm"], None);
    assert!(
        f.ok_json(&["artifact", "list", "--session", sess_key], None)["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn session_create_refuses_bare_id_prefix_or_full_key() {
    let f = Fixture::new();
    let s = f.ok_json(&["stream", "create", "--title", "Effort"], None);
    let s_key = s["key"].as_str().unwrap();
    for bad in ["bare-id", "host/pi/id", "pi/", "/id", "pi/bad..id/extra"] {
        let (code, err, _) = f.run_json(
            &["session", "create", bad, "--stream", s_key, "--title", "T"],
            None,
        );
        assert_eq!(code, 1, "{bad}");
        assert_eq!(err["error"]["kind"], "invalid-ref", "{bad}: {err}");
    }
}

#[test]
fn list_text_rejects_collection_fields_and_limit_reports_more() {
    let f = Fixture::new();
    f.ok_json(&["stream", "create", "--title", "One"], None);
    f.ok_json(&["stream", "create", "--title", "Two"], None);

    let (code, _, stderr) = f.run_raw(&["stream", "list", "--fields", "sessions"], None);
    assert_eq!(code, 1);
    assert!(stderr.contains("--json"), "{stderr}");

    let limited = f.ok_json(&["stream", "list", "--limit", "1"], None);
    assert_eq!(limited["items"].as_array().unwrap().len(), 1);
    assert_eq!(limited["more"], true);

    let (code, stdout, _) = f.run_raw(&["stream", "list", "--limit", "1"], None);
    assert_eq!(code, 0);
    let text = String::from_utf8(stdout).unwrap();
    assert!(text.contains("1 more; raise --limit"), "{text}");

    // `catalog check` and `catalog tidy` work on format 5.
    let check = f.ok_json(&["catalog", "check"], None);
    assert_eq!(check["valid"], true);
    assert_eq!(check["streams"], 2);
    let tidy = f.ok_json(&["catalog", "tidy"], None);
    assert_eq!(tidy["closed_sessions"], 0);

    // `config get` / `config path`.
    let (code, stdout, _) = f.run_raw(&["config", "get", "frontend.mode"], None);
    assert_eq!(code, 0);
    assert_eq!(String::from_utf8(stdout).unwrap().trim(), "none");
    assert!(f.root().join(".catalog-local/records.json").is_file());
}

#[test]
fn top_level_help_hides_card_and_debug_while_subcommands_remain_callable() {
    let f = Fixture::new();
    let (code, stdout, _) = f.run_raw(&["--help"], None);
    assert_eq!(code, 0);
    let help = String::from_utf8(stdout).unwrap();
    for visible in [
        "list", "resume", "stream", "session", "artifact", "config", "catalog",
    ] {
        assert!(help.contains(visible), "expected {visible} in:\n{help}");
    }
    assert!(!help.contains("  card "), "card should be hidden:\n{help}");
    assert!(
        !help.contains("  debug "),
        "debug should be hidden:\n{help}"
    );

    for hidden in ["card", "debug"] {
        let (code, stdout, _) = f.run_raw(&[hidden, "--help"], None);
        assert_eq!(code, 0, "{hidden} --help should work");
        assert!(String::from_utf8(stdout).unwrap().contains("Usage:"));
    }
}
