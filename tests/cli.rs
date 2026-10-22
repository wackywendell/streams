use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cwd = temporary.path().join("working");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(home.join("gitconfig"), "[user]\n name = Streams Test\n email = streams@example.test\n[commit]\n gpgSign = false\n").unwrap();
        Self {
            _temporary: temporary,
            home: fs::canonicalize(home).unwrap(),
            cwd: fs::canonicalize(cwd).unwrap(),
        }
    }
    fn root(&self) -> PathBuf {
        self.home.join("data/streams/catalog")
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_streams"));
        command
            .env_clear()
            .current_dir(&self.cwd)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"));
        command
    }
    fn run(&self, args: &[&str], input: Option<&str>) -> (i32, Value) {
        let mut command = self.command();
        command
            .args(args)
            .arg("--json")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if input.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = command.spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{args:?}: {error}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), value)
    }
    fn ok(&self, args: &[&str]) -> Value {
        let (code, value) = self.run(args, None);
        assert_eq!(code, 0, "{args:?}: {value}");
        value
    }
    fn init(&self) {
        self.ok(&["init", "--sync", "none"]);
    }
    fn create(&self, title: &str) -> Value {
        self.ok(&["create", title])["stream"].clone()
    }
}

#[test]
fn accepted_offline_create_retains_publication_warning_and_durable_record() {
    let f = Fixture::new();
    use streams_temper::{
        config::{Marker, Strategy},
        model::Record,
        storage::{StorageError, Transaction},
        sync::{
            git::GitCatalog,
            policy::Policy,
            state::{GitConfig, State},
        },
        yaml_store::{YamlStore, relative_record_path},
    };
    // Seed an existing managed catalog; this test needs an offline save, not a fresh clone.
    f.init();
    let store = YamlStore::new(f.root());
    let marker = store
        .with_inspection::<_, StorageError>(|tx| Ok(Marker::read(tx.journal()).unwrap().unwrap()))
        .unwrap();
    let mut backend = GitCatalog::new(f.root(), marker.host.as_str(), Policy::default()).unwrap();
    crate::support::git::install_hook(&mut backend, |_, _, _, _| None);
    crate::support::git::seed_bootstrap(&backend);
    let remote = f.home.join("offline.git");
    backend
        .git()
        .run(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            None,
            None,
            &[0],
        )
        .unwrap();
    store
        .with_transaction::<_, StorageError>(|tx| {
            let state = State::from_journal(tx.journal());
            state
                .write_config(
                    &GitConfig::new(
                        Some(remote.to_str().unwrap().into()),
                        "main",
                        marker.host.as_str(),
                    )
                    .unwrap(),
                )
                .unwrap();
            Marker {
                strategy: Strategy::Git,
                ..marker
            }
            .write(&state)
            .unwrap();
            Ok(())
        })
        .unwrap();
    // Still cross the real CLI's Git/remote dispatch; the configured remote is already offline.
    f.ok(&[
        "init",
        "--sync",
        "git",
        "--remote",
        remote.to_str().unwrap(),
    ]);
    let result = f.ok(&["create", "Saved offline"]);
    assert_eq!(result["save"]["local"], "committed");
    assert_eq!(result["save"]["domain"]["outcome"], "changes");
    assert_eq!(result["publication"]["sharing"], "failed");
    assert_eq!(result["publication"]["local"], "unchanged");
    // Read persisted state directly: show/sync behavior has backend-level tests.
    let snapshot = YamlStore::new(f.root())
        .with_inspection::<_, StorageError>(|tx| tx.read())
        .unwrap();
    assert_eq!(snapshot.catalog.streams().len(), 1);
    let stream = &snapshot.catalog.streams()[0];
    assert_eq!(serde_json::to_value(stream).unwrap(), result["stream"]);
    let path = relative_record_path(&Record::Stream(stream.clone()));
    let committed = backend
        .git()
        .run(
            &["show", &format!("HEAD:{}", path.display())],
            None,
            None,
            &[0],
        )
        .unwrap();
    assert_eq!(committed, fs::read(f.root().join(path)).unwrap());
}

#[test]
fn rejected_reads_adoption_and_missing_sync_leave_uninitialized_catalog_untouched() {
    let f = Fixture::new();
    for args in [
        vec!["list"],
        vec!["init"],
        vec!["init", "--sync", "git", "--adopt"],
        vec![
            "agent",
            "context",
            "--harness",
            "pi",
            "--session-id",
            "unknown",
        ],
    ] {
        let (code, value) = f.run(&args, None);
        assert_eq!(code, 2, "{value}");
        assert!(value.get("error").is_some());
        assert!(!f.root().exists());
        assert!(!f.home.join("config/streams/host.json").exists());
    }
}

#[test]
fn current_target_requires_supported_policy_even_with_an_explicit_environment_identity() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("Current");
    let id = stream["id"].as_str().unwrap();
    assert_eq!(
        f.ok(&["show", id, "--frontend", "third-party"])["stream"],
        stream
    );
    for (frontend, code) in [("auto", 0), ("third-party", 2)] {
        let output = f
            .command()
            .env("STREAMS_STREAM_ID", id)
            .args(["show", "--current", "--frontend", frontend, "--json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(code), "{output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        if code == 0 {
            assert_eq!(value["stream"], stream);
        } else {
            assert!(
                value["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("unsupported")
            );
        }
    }
}

#[test]
fn session_attach_captures_explicit_native_id_and_keeps_local_directory_state_private() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("Work");
    let id = stream["id"].as_str().unwrap();
    let attached = f.ok(&[
        "session",
        "attach",
        id,
        "--harness",
        "pi",
        "--session-id",
        "opaque-native-id",
        "--title",
        "Contribution",
    ]);
    let session = &attached["session"];
    let session_id = session["id"].as_str().unwrap();
    assert_eq!(session["harness_session_id"], "opaque-native-id");
    assert_eq!(
        f.ok(&["session", "list", "--stream", id])["sessions"],
        json!([session])
    );
    assert_eq!(f.ok(&["session", "show", session_id])["session"], *session);
    let directory = f.ok(&["session", "directory", session_id]);
    assert_eq!(directory["directory"]["cwd"], json!(f.cwd));
    let changed = f.ok(&[
        "session",
        "update",
        session_id,
        "--state",
        "Working",
        "--remaining-file",
        "-",
    ]);
    assert_eq!(changed["session"]["state"], "Working");
    let files = catalog_files(&f.root());
    assert!(
        !files
            .iter()
            .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains(f.cwd.to_str().unwrap()))
    );
    let context = f.ok(&[
        "agent",
        "context",
        "--harness",
        "pi",
        "--session-id",
        "opaque-native-id",
    ]);
    assert_eq!(context["session"]["id"], session_id);
    assert_eq!(context["stream"]["id"], id);
    assert_eq!(context["checkouts"], json!([]));
    assert_eq!(context["document_locations"], json!([]));
}

#[test]
fn membership_move_keeps_checkout_selection_and_never_uses_destination_alias_url() {
    use streams_temper::{operations::Operation, sync::local::LocalCatalog, yaml_store::YamlStore};
    let f = Fixture::new();
    f.init();
    let initialized = Command::new("git")
        .args(["init", "--initial-branch=main"])
        .arg(&f.cwd)
        .env("GIT_CONFIG_GLOBAL", f.home.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(initialized.status.success());
    let source = f.create("Source");
    let destination = f.create("Destination");
    let source_id = source["id"].as_str().unwrap();
    let destination_id = destination["id"].as_str().unwrap();
    let source_url = "https://example.test/original.git";
    f.ok(&["update", source_id, "--add-repository", "app", source_url]);
    let destination = f.ok(&[
        "update",
        destination_id,
        "--add-repository",
        "app",
        "https://example.test/different.git",
    ])["stream"]
        .clone();
    let attached = f.ok(&[
        "session",
        "attach",
        source_id,
        "--harness",
        "pi",
        "--session-id",
        "native",
        "--title",
        "Work",
        "--repository",
        "app",
        "--ref",
        "main",
    ]);
    let session = &attached["session"];
    let session_id = session["id"].as_str().unwrap();
    let selected = f.ok(&["session", "directory", session_id])["directory"].clone();
    let checkout_id = selected["checkout_id"].as_str().unwrap();
    let backend = LocalCatalog::new(
        YamlStore::new(f.root()),
        session["host_id"].as_str().unwrap(),
    )
    .unwrap();
    let snapshot = backend.snapshot().unwrap();
    let before = snapshot
        .catalog
        .session(session["host_id"].as_str().unwrap(), session_id)
        .unwrap();
    let source = snapshot.catalog.stream(source_id).unwrap();
    let token = backend.authority(source_id).unwrap().unwrap().token;
    let operation = Operation::reassociate("move", before, source, destination_id, token).unwrap();
    assert!(backend.save(&operation, None).unwrap().accepted());
    assert_eq!(
        f.ok(&["session", "directory", session_id])["directory"],
        selected
    );
    let selected_again = f.ok(&[
        "session",
        "directory",
        session_id,
        "--checkout",
        checkout_id,
    ]);
    assert_eq!(selected_again["directory"], selected);
    let (code, _) = f.run(
        &[
            "session",
            "directory",
            session_id,
            "--checkout",
            checkout_id,
            "--repository",
            "app",
        ],
        None,
    );
    assert_eq!(code, 2);
    assert_eq!(
        f.ok(&["session", "directory", session_id])["directory"],
        selected
    );
    let moved = f.ok(&["session", "show", session_id]);
    assert_eq!(moved["session"]["checkout"]["repository_url"], source_url);
    assert_eq!(f.ok(&["show", destination_id])["stream"], destination);
}

#[test]
fn captured_apply_persists_stdin_and_rejects_wrong_source_without_enrollment() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("Work");
    let id = stream["id"].as_str().unwrap();
    let context = f.ok(&[
        "agent",
        "context",
        "--harness",
        "pi",
        "--session-id",
        "not-enrolled",
        "--stream-id",
        id,
    ]);
    assert_eq!(context["session"], Value::Null);
    assert_eq!(context["stream"], stream);
    let request = json!({"operation_id":"captured-update","catalog_source":context["catalog_source"],"actor":"user","kind":"stream","before":stream,"changes":{"goal":"Captured"},"inputs":[]});
    let (code, first) = f.run(&["agent", "apply"], Some(&request.to_string()));
    assert_eq!(code, 0, "{first}");
    assert_eq!(first["operation_id"], "captured-update");
    assert_eq!(first["save"]["domain"]["outcome"], "changes");
    let before = catalog_files(&f.root());
    let mut bad = request.clone();
    bad["catalog_source"] = json!("wrong");
    let (code, error) = f.run(&["agent", "apply"], Some(&bad.to_string()));
    assert_eq!(code, 2);
    assert!(error.get("error").is_some());
    assert_eq!(catalog_files(&f.root()), before);
    assert_eq!(f.ok(&["show", id])["stream"]["goal"], "Captured");
}

#[test]
fn accepted_save_keeps_receipt_when_directory_completion_fails() {
    let f = Fixture::new();
    f.init();
    let missing = f.home.join("missing-cwd");
    let (code, value) = f.run(
        &["create", "Saved", "--cwd", missing.to_str().unwrap()],
        None,
    );
    assert_eq!(code, 2, "{value}");
    assert_eq!(value["save"]["local"], "persisted");
    assert!(value["runtime"]["error"]["message"].is_string());
    assert_eq!(
        f.ok(&["show", value["stream"]["id"].as_str().unwrap()])["stream"],
        value["stream"]
    );
    let stream_id = value["stream"]["id"].as_str().unwrap();
    let (code, attached) = f.run(
        &[
            "session",
            "attach",
            stream_id,
            "--harness",
            "pi",
            "--session-id",
            "completion-native",
            "--title",
            "Saved session",
            "--cwd",
            missing.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code, 2, "{attached}");
    assert_eq!(attached["save"]["local"], "persisted");
    assert!(attached["runtime"]["error"].is_string());
    assert_eq!(
        f.ok(&[
            "session",
            "show",
            attached["session"]["id"].as_str().unwrap()
        ])["session"],
        attached["session"]
    );
}

#[test]
fn git_import_requires_explicit_adoption_and_returns_baseline_receipt() {
    let f = Fixture::new();
    f.init();
    let marker_path = f.root().join(".catalog-sync/cli.json");
    let before = fs::read(&marker_path).unwrap();
    let (code, rejected) = f.run(&["init", "--sync", "git"], None);
    assert_eq!(code, 2, "{rejected}");
    assert!(
        rejected["error"]["message"]
            .as_str()
            .unwrap()
            .contains("explicit adoption")
    );
    assert_eq!(fs::read(&marker_path).unwrap(), before);
    assert!(!f.root().join(".git").exists());
    assert!(!f.root().join(".catalog-sync/save.json").exists());

    let imported = f.ok(&["init", "--sync", "git", "--adopt"]);
    imported["baseline_revision"]
        .as_str()
        .unwrap()
        .parse::<streams_temper::sync::Oid>()
        .unwrap();
    assert!(f.root().join(".git").is_dir());
    let marker: Value = serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
    let original: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(marker["strategy"], "git");
    assert_eq!(marker["host"], original["host"]);
}

#[test]
fn frontend_policy_errors_keep_typed_candidates_without_native_calls() {
    let f = Fixture::new();
    let output = f
        .command()
        .env("CMUX_WORKSPACE_ID", "cmux-work")
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "herdr-pane")
        .args(["frontend", "get", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["kind"], "ambiguous");
    assert_eq!(error["error"]["candidates"], json!(["cmux", "herdr"]));
    assert!(!f.root().exists());
    assert_eq!(
        f.ok(&["frontend", "get", "--frontend", "third-party"]),
        json!({"configured":"auto","requested":"third-party","resolved":"third-party"})
    );
}

#[test]
fn local_catalog_does_not_require_git_in_plain_directories() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let bin = f.home.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\nprintf called >> \"$STREAMS_TEST_GIT_PROBE\"\nexit 99\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o700)).unwrap();
    let probe = f.home.join("git-called");
    for args in [
        vec!["init", "--sync", "none"],
        vec!["create", "No subprocess"],
        vec!["list"],
        vec!["archive", "No subprocess"],
        vec!["restore", "No subprocess"],
        vec!["check"],
        vec!["sync"],
    ] {
        let output = f
            .command()
            .env("PATH", &bin)
            .env("STREAMS_TEST_GIT_PROBE", &probe)
            .args(&args)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let data: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(data.get("runtime").is_none(), "{data}");
        if matches!(args[0], "archive" | "restore") {
            assert_eq!(data["stream"]["archived"], args[0] == "archive");
        }
        assert!(!probe.exists());
    }
}

#[test]
fn session_references_and_candidates_keep_the_host_qualified_identity() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("Sessions");
    let local = f.ok(&[
        "session",
        "attach",
        stream["id"].as_str().unwrap(),
        "--harness",
        "pi",
        "--session-id",
        "native",
        "--title",
        "Contribution",
    ])["session"]
        .clone();
    let id = local["id"].as_str().unwrap();
    let host = local["host_id"].as_str().unwrap();
    let foreign_host = "00000000-0000-4000-8000-000000000000";
    let mut foreign = local.clone();
    foreign["host_id"] = json!(foreign_host);
    foreign["harness_session_id"] = json!("foreign-native");
    let directory = f
        .root()
        .join("sessions")
        .join(stream["id"].as_str().unwrap())
        .join(foreign_host);
    fs::create_dir_all(&directory).unwrap();
    fs::write(
        directory.join(format!("{id}.yaml")),
        serde_json::to_vec_pretty(
            &json!({"version":1,"revision":"foreign-revision","kind":"session","record":foreign}),
        )
        .unwrap(),
    )
    .unwrap();
    let (code, error) = f.run(&["session", "show", id], None);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["error"]["kind"], "ambiguous", "{error}");
    let mut candidates: Vec<_> = error["error"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    candidates.sort();
    let qualified = format!("{host}/{id}");
    let foreign_qualified = format!("{foreign_host}/{id}");
    let mut expected = vec![qualified.clone(), foreign_qualified.clone()];
    expected.sort();
    assert_eq!(candidates, expected);
    assert_eq!(f.ok(&["session", "show", &qualified])["session"], local);
    assert_eq!(
        f.ok(&["session", "show", &foreign_qualified])["session"],
        foreign
    );
    assert_eq!(
        f.ok(&["session", "update", &qualified, "--state", "Active"])["session"]["state"],
        "Active"
    );
    assert_eq!(
        f.ok(&["session", "directory", &qualified])["directory"]["cwd"],
        f.cwd.to_str().unwrap()
    );
}

#[test]
fn invalid_utf8_captured_input_is_rejected_before_any_record_changes() {
    let f = Fixture::new();
    f.init();
    f.create("Unchanged");
    let before = catalog_files(&f.root());
    let mut child = f
        .command()
        .args(["agent", "apply", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&[0xff]).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(error["error"]["message"].is_string());
    assert_eq!(catalog_files(&f.root()), before);
}

fn catalog_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), files);
            } else {
                files.push((
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                ));
            }
        }
    }
    let mut files = Vec::new();
    for directory in ["streams", "sessions", "conversations", "hosts"] {
        let path = root.join(directory);
        if path.exists() {
            visit(root, &path, &mut files);
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}
