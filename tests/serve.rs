use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::fd::AsRawFd,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::Duration,
};

use serde_json::{Value, json};

#[path = "serve/subscriptions.rs"]
mod subscriptions;

struct Fixture {
    directory: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let fixture = Self {
            directory: tempfile::tempdir().unwrap(),
        };
        fixture.ok(&["init", "--sync", "none"], None);
        fixture.ok(&["config", "set", "frontend.mode", "none"], None);
        fixture
    }
    fn root(&self) -> PathBuf {
        self.directory.path().join("data/streams/catalog")
    }
    fn command(&self) -> Command {
        let home = self.directory.path();
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
    fn cli(&self, argv: &[&str], input: Option<&str>) -> (i32, Value) {
        let mut command = self.command();
        command.args(argv);
        if !argv.contains(&"--json") {
            command.arg("--json");
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
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
                "{argv:?}: {error}; stderr={}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), value)
    }
    fn ok(&self, argv: &[&str], input: Option<&str>) -> Value {
        let (code, value) = self.cli(argv, input);
        assert_eq!(code, 0, "{value}");
        value
    }
    fn start(&self) -> Transport {
        let transport = Transport::start(self.command(), "pi-session");
        assert_eq!(transport.next(), json!({"type":"ready","protocol":1}));
        let scope = transport.next();
        assert_eq!(scope["type"], "event");
        assert_eq!(scope["event"], "scope");
        assert_eq!(scope["value"], Value::Null);
        assert!(scope["reason"].is_string());
        transport
    }
    fn capture(&self) -> (Value, Value) {
        let stream = self.ok(&["create", "Work"], None)["stream"].clone();
        let mut context = self.ok(
            &[
                "agent",
                "context",
                "--harness",
                "pi",
                "--session-id",
                "pi-session",
                "--stream-id",
                stream["id"].as_str().unwrap(),
            ],
            None,
        );
        context
            .as_object_mut()
            .unwrap()
            .remove("document_locations");
        let request = json!({"operation_id":"update", "catalog_source":context["catalog_source"],
            "actor":"user", "kind":"stream", "before":stream, "changes":{"goal":"Captured"}, "inputs":[]});
        (context, request)
    }
}

struct Transport {
    child: Child,
    input: Option<ChildStdin>,
    frames: Receiver<Result<Value, String>>,
}
impl Transport {
    fn start(mut command: Command, session: &str) -> Self {
        let mut child = command
            .args(["serve", "--session-id", session, "--plugin-root"])
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/integrations/herdr-work-cards"
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let frame = line.map_err(|e| e.to_string()).and_then(|line| {
                    serde_json::from_str(&line).map_err(|e| format!("{line:?}: {e}"))
                });
                if tx.send(frame).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            frames,
        }
    }
    fn send(&mut self, frame: Value) {
        writeln!(self.input.as_mut().unwrap(), "{frame}").unwrap();
    }
    fn request(&mut self, id: u64, argv: &[&str], input: Option<Value>) {
        let mut frame = json!({"type":"request", "id":id, "argv":argv});
        if let Some(input) = input {
            frame["input"] = input;
        }
        self.send(frame);
    }
    fn next(&self) -> Value {
        self.frames
            .recv_timeout(Duration::from_secs(15))
            .expect("child frame")
            .unwrap()
    }
    fn response(&self, id: u64) -> Value {
        let frame = self.next();
        assert_eq!(frame["type"], "response", "{frame}");
        assert_eq!(frame["id"], id);
        assert!(frame["notices"].is_array());
        frame
    }
    fn finish(&mut self) -> (i32, String) {
        self.input.take();
        let status = self.child.wait().unwrap();
        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        (status.code().unwrap(), stderr)
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn handshake_catalog_json_and_eof_drain() {
    let f = Fixture::new();
    f.ok(&["create", "Work"], None);
    let expected = f.ok(&["list"], None);
    let mut child = f.start();
    child.request(1, &["list"], None);
    assert_eq!(
        child.response(1),
        json!({"type":"response","id":1,"code":0,"value":expected,"notices":[]})
    );
    child.request(2, &["--json", "list"], None);
    child.input.take();
    assert_eq!(child.response(2)["value"], expected);
    assert_eq!(child.finish(), (0, String::new()));
    assert!(child.frames.try_recv().is_err());
}

#[test]
fn input_is_private_stdin_for_preview_apply_and_enroll() {
    let f = Fixture::new();
    let (context, request) = f.capture();
    let preview = json!({"catalog":context,"action":"apply","request":request});
    let expected = streams_temper::agent::preview(&preview.to_string()).unwrap();
    let mut child = f.start();
    child.request(1, &["agent", "preview", "--json"], Some(preview));
    assert_eq!(child.response(1)["value"], expected);
    child.request(2, &["agent", "apply"], Some(request));
    let applied = child.response(2);
    assert_eq!(applied["code"], 0, "{applied}");
    assert_eq!(applied["value"]["save"]["domain"]["outcome"], "changes");
    let id = context["stream"]["id"].as_str().unwrap();
    assert_eq!(f.ok(&["show", id], None)["stream"]["goal"], "Captured");
    let enrollment = json!({"version":2,"operation_id":"enroll", "catalog_source":context["catalog_source"],
        "harness":"pi", "harness_session_id":"pi-session", "actor":"user",
        "cwd":f.directory.path(), "stream_id":id, "session":{"title":"Session"}});
    child.request(3, &["agent", "enroll"], Some(enrollment));
    let enrolled = child.response(3);
    assert_eq!(enrolled["code"], 0, "{enrolled}");
    assert_eq!(enrolled["value"]["session"]["title"], "Session");
    for (id, input) in [(4, None), (5, Some(Value::Null))] {
        let text = input.as_ref().map(Value::to_string).unwrap_or_default();
        let (code, expected) = f.cli(&["agent", "preview"], Some(&text));
        assert_eq!(code, 2);
        child.request(id, &["agent", "preview"], input);
        let response = child.response(id);
        assert_eq!(response["code"], 1);
        assert_eq!(response["value"], expected);
    }
    assert_eq!(child.finish(), (0, String::new()));
    // Initial null scope was unchanged after apply and enroll: no duplicate events.
    assert!(child.frames.try_recv().is_err());
}

#[test]
fn errors_and_allowlist_are_serve_only_and_never_dispatch_effects() {
    let f = Fixture::new();
    let mut child = f.start();
    let refused: &[&[&str]] = &[
        &["debug", "run"],
        &[
            "serve",
            "--session-id",
            "pi-session",
            "--plugin-root",
            "/plugin",
        ],
        &["native", "pane"],
        &["native", "focus"],
        &["native", "check"],
        &["create", "Forbidden"],
        &["init", "--sync", "none"],
        &["show", "missing"],
        &["checkout", "list"],
        &["session", "list"],
        &["config"],
        &["frontend", "get"],
        &["replay"],
        &[],
        &["--help"],
        &["-h"],
        &["help"],
        &["list", "--help"],
        &["list", "-h"],
        &["create", "--help"],
        &["native", "help"],
        &["agent", "help"],
        &["workspace", "help"],
        &["native", "focus", "--bad"],
        &["native", "--session-id", "pi-session", "focus", "--bad"],
        &["serve"],
        &["init"],
        &["create"],
        &["open"],
        &["resume"],
        &["update"],
        &["archive"],
        &["restore"],
        &["session"],
        &["checkout"],
        &["frontend"],
        &["agent", "unknown"],
        &["workspace", "unknown"],
    ];
    for (offset, argv) in refused.iter().enumerate() {
        let id = offset as u64 + 100;
        child.request(id, argv, None);
        let response = child.response(id);
        assert_eq!(response["code"], 1, "{argv:?}: {response}");
        assert_eq!(
            response["value"]["error"]["kind"], "unsupported-command",
            "{argv:?}"
        );
    }
    for (id, argv) in [
        (20, vec!["native", "--session-id=other", "open"]),
        (21, vec!["native", "open", "--session-id", "other"]),
        (24, vec!["native", "--session-id", "help", "open"]),
    ] {
        child.request(id, &argv, None);
        let response = child.response(id);
        assert_eq!(response["code"], 1);
        assert_eq!(response["value"]["error"]["kind"], "session-mismatch");
    }
    let argv = ["agent", "context", "--json"];
    let (cli_code, expected) = f.cli(&argv, None);
    assert_eq!(cli_code, 2);
    child.request(22, &argv, None);
    let response = child.response(22);
    assert_eq!(response["code"], 1);
    assert_eq!(response["value"], expected);
    child.request(23, &["list", "--json"], None);
    assert_eq!(child.response(23)["value"]["streams"], json!([]));
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn sync_capture_inputs_and_workspace_capture_share_cli_dispatch() {
    let f = Fixture::new();
    let (context, _) = f.capture();
    let id = context["stream"]["id"].as_str().unwrap();
    let mut child = f.start();
    for (request_id, argv) in [
        (1, vec!["sync", "--json"]),
        (2, vec!["agent", "capture-inputs", "--stream-id", id]),
        (3, vec!["workspace", "capture", id]),
        (4, vec!["list", "help"]),
        (5, vec!["workspace", "capture", "help"]),
    ] {
        let (cli_code, expected) = f.cli(&argv, None);
        child.request(request_id, &argv, None);
        let response = child.response(request_id);
        assert_eq!(response["value"], expected, "{argv:?}");
        assert_eq!(response["code"], if cli_code == 0 { 0 } else { 1 });
    }
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn blocked_catalog_request_does_not_block_responses_and_eof_finishes_it() {
    let f = Fixture::new();
    let (context, request) = f.capture();
    let preview = json!({"catalog":context,"action":"apply","request":request});
    let mut child = f.start();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root().join(".catalog.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    child.request(1, &["list"], None);
    child.request(2, &["agent", "preview"], Some(preview));
    assert_eq!(child.response(2)["code"], 0);
    child.send(json!({"type":"cancel","id":1}));
    child.send(json!({"type":"cancel","id":999}));
    child.input.take();
    drop(lock);
    assert_eq!(child.response(1)["code"], 0);
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn concurrent_saves_keep_both_changes_and_receipt_not_ok_keeps_code_two() {
    let f = Fixture::new();
    let (context, request) = f.capture();
    let mut child = f.start();
    let mut title = request.clone();
    title["operation_id"] = json!("title");
    title["changes"] = json!({"title":"Renamed"});
    child.request(1, &["agent", "apply"], Some(request.clone()));
    child.request(2, &["agent", "apply"], Some(title));
    let replies = [child.next(), child.next()];
    for reply in replies {
        assert_eq!(reply["type"], "response");
        assert_eq!(reply["code"], 0, "{reply}");
    }
    let id = context["stream"]["id"].as_str().unwrap();
    let stream = &f.ok(&["show", id], None)["stream"];
    assert_eq!(stream["title"], "Renamed");
    assert_eq!(stream["goal"], "Captured");
    let mut stale = request;
    stale["operation_id"] = json!("stale");
    stale["changes"] = json!({"goal":"Conflicting"});
    let (cli_code, expected) = f.cli(&["agent", "apply"], Some(&stale.to_string()));
    assert_eq!(cli_code, 2, "{expected}");
    child.request(3, &["agent", "apply"], Some(stale));
    let response = child.response(3);
    assert_eq!(response["code"], 2);
    assert_eq!(response["value"], expected);
    assert!(response["value"].get("error").is_none());
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn list_returns_notices_from_an_offline_catalog() {
    use streams_temper::{
        config::{Marker, Strategy},
        storage::{StorageError, Transaction},
        sync::{
            git::GitCatalog,
            policy::Policy,
            state::{GitConfig, State},
        },
        yaml_store::YamlStore,
    };
    let f = Fixture::new();
    let store = YamlStore::new(f.root());
    let marker = store
        .with_inspection::<_, StorageError>(|tx| Ok(Marker::read(tx.journal()).unwrap().unwrap()))
        .unwrap();
    let mut backend = GitCatalog::new(f.root(), marker.host.as_str(), Policy::default()).unwrap();
    crate::support::git::install_hook(&mut backend, |_, _, _, _| None);
    crate::support::git::seed_bootstrap(&backend);
    let remote = f.directory.path().join("offline.git");
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
    f.ok(
        &[
            "init",
            "--sync",
            "git",
            "--remote",
            remote.to_str().unwrap(),
        ],
        None,
    );
    let mut child = f.start();
    child.request(1, &["list", "--refresh", "--json"], None);
    let response = child.response(1);
    assert_eq!(response["code"], 0, "{response}");
    let detail = response["value"]["sharing"]["detail"].as_str().unwrap();
    assert!(!detail.is_empty());
    assert!(
        response["notices"]
            .as_array()
            .unwrap()
            .contains(&json!(detail))
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn malformed_frame_crashes_even_with_a_blocked_request() {
    let f = Fixture::new();
    let mut child = f.start();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root().join(".catalog.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    child.request(1, &["list"], None);
    child
        .input
        .as_mut()
        .unwrap()
        .write_all(b"not json\n")
        .unwrap();
    assert!(matches!(
        child.frames.recv_timeout(Duration::from_secs(15)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    ));
    let (code, stderr) = child.finish();
    assert_ne!(code, 0);
    assert!(!stderr.is_empty());
    drop(lock);
}

#[test]
fn malformed_frames_fail_with_diagnostics_not_protocol_stdout() {
    let f = Fixture::new();
    for frame in [
        "not json\n",
        "{}\n",
        "{\"type\":\"wat\",\"id\":1}\n",
        "{\"type\":\"cancel\"}\n",
        "{\"type\":\"cancel\",\"id\":1,\"extra\":true}\n",
        "{\"type\":\"request\",\"id\":1,\"argv\":[\"list\"],\"stdin\":{}}\n",
        "{\"type\":\"cancel\",\"id\":1,\"id\":2}\n",
        "{\"type\":\"request\",\"id\":1,\"argv\":[\"agent\",\"apply\"],\"input\":{\"changes\":{\"goal\":\"first\",\"goal\":\"second\"}}}\n",
        "{\"type\":\"request\",\"id\":1,\"argv\":[\"agent\",\"preview\"],\"input\":{\"action\":\"apply\",\"action\":\"enroll\"}}\n",
        "{\"type\":\"request\",\"id\":1,\"argv\":[\"agent\",\"enroll\"],\"input\":{\"session\":{\"title\":\"first\",\"title\":\"second\"}}}\n",
        "{\"type\":\"request\",\"id\":1,\"argv\":[42]}\n",
        "{\"type\":\"cancel\",\"id\":1}",
        "\n",
        "{\"type\":\"cancel\",\"id\":0}\n",
        "{\"type\":\"cancel\",\"id\":-1}\n",
        "{\"type\":\"cancel\",\"id\":1.5}\n",
        "{\"type\":\"cancel\",\"id\":9007199254740992}\n",
        "{\"type\":\"request\",\"id\":1,\"argv\":[\"list\"]}\n{\"type\":\"request\",\"id\":1,\"argv\":[\"list\"]}\n",
    ] {
        let mut child = f.start();
        child
            .input
            .take()
            .unwrap()
            .write_all(frame.as_bytes())
            .unwrap();
        let (code, stderr) = child.finish();
        assert_ne!(code, 0, "{frame:?}");
        assert!(!stderr.is_empty(), "{frame:?}");
        for value in child.frames.try_iter() {
            assert_eq!(value.unwrap()["type"], "response");
        }
    }
}
