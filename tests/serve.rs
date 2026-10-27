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
        assert_eq!(transport.next(), json!({"type":"ready","protocol":2}));
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
        let request = json!({"version":2,"operation_id":"update", "catalog_source":context["catalog_source"],
            "actor":"user", "kind":"stream", "before":stream, "changes":{"goal":"Captured"}, "inputs":[],
            "title_default":context["title_defaults"]["stream"]});
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
    fn request(&mut self, id: u64, request: Value) {
        self.send(json!({"type":"request","id":id,"request":request}));
    }
    fn next(&self) -> Value {
        self.frames
            .recv_timeout(Duration::from_secs(15))
            .expect("child frame")
            .unwrap()
    }
    fn response(&self, id: u64) -> Value {
        loop {
            let frame = self.next();
            if frame["type"] == "view" {
                continue;
            }
            assert_eq!(frame["type"], "response", "{frame}");
            assert_eq!(frame["id"], id);
            assert!(frame["outcome"].is_object());
            return frame;
        }
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
fn typed_handshake_catalog_results_and_eof_drain() {
    let f = Fixture::new();
    f.ok(&["create", "Work"], None);
    let expected = f.ok(&["list"], None);
    let mut child = f.start();
    child.request(1, json!({"kind":"list_streams"}));
    assert_eq!(
        child.response(1)["outcome"],
        json!({"status":"success","result":expected})
    );
    child.request(2, json!({"kind":"list_streams"}));
    child.input.take();
    assert_eq!(child.response(2)["outcome"]["result"], expected);
    assert_eq!(child.finish(), (0, String::new()));
    assert!(child.frames.try_recv().is_err());
}

#[test]
fn typed_preview_apply_and_enroll_keep_domain_packets_without_stdin() {
    let f = Fixture::new();
    let (context, request) = f.capture();
    let preview = json!({"catalog":context,"action":"apply","request":request});
    let expected = streams_temper::agent::preview(&preview.to_string()).unwrap();
    let mut child = f.start();
    child.request(
        1,
        json!({"kind":"preview","catalog":context,"action":"apply","request":request}),
    );
    assert_eq!(child.response(1)["outcome"]["result"], expected);
    child.request(2, json!({"kind":"apply","request":request}));
    let applied = child.response(2);
    assert_eq!(applied["outcome"]["status"], "success", "{applied}");
    assert_eq!(
        applied["outcome"]["result"]["save"]["domain"]["outcome"],
        "changes"
    );
    let id = context["stream"]["id"].as_str().unwrap();
    assert_eq!(f.ok(&["show", id], None)["stream"]["goal"], "Captured");
    let enrollment = json!({"version":2,"operation_id":"enroll", "catalog_source":context["catalog_source"],
        "harness":"pi", "harness_session_id":"pi-session", "actor":"user",
        "cwd":f.directory.path(), "stream_id":id, "session":{"title":"Session"}});
    child.request(3, json!({"kind":"enroll","request":enrollment}));
    let enrolled = child.response(3);
    assert_eq!(enrolled["outcome"]["status"], "success", "{enrolled}");
    assert_eq!(enrolled["outcome"]["result"]["session"]["title"], "Session");
    child.request(
        4,
        json!({"kind":"preview","catalog":{},"action":"apply","request":{}}),
    );
    assert_eq!(child.response(4)["outcome"]["status"], "error");
    child.request(5, json!({"kind":"list_streams"}));
    assert_eq!(child.response(5)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn preview_and_ready_never_load_poisoned_configuration() {
    let f = Fixture::new();
    let (catalog, request) = f.capture();
    fs::write(
        f.directory.path().join("config/streams/config.json"),
        "invalid config",
    )
    .unwrap();
    let mut command = f.command();
    command.env("HOME", "not-an-absolute-home");
    let mut child = Transport::start(command, "pi-session");
    assert_eq!(child.next(), json!({"type":"ready","protocol":2}));
    assert!(
        child
            .frames
            .recv_timeout(Duration::from_millis(80))
            .is_err()
    );
    child.request(
        1,
        json!({"kind":"preview","catalog":catalog,"action":"apply","request":request}),
    );
    let response = child.response(1);
    assert_eq!(response["outcome"]["status"], "success", "{response}");
    assert_eq!(response["outcome"]["result"]["receipt"]["simulated"], true);
    assert!(!f.directory.path().join("state/streams/native").exists());
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn title_default_requests_are_narrow_and_context_reads_them_fresh() {
    let f = Fixture::new();
    let mut child = f.start();
    child.request(
        1,
        json!({"kind":"set_title_default","target":"session","mode":"manual"}),
    );
    assert_eq!(
        child.response(1)["outcome"],
        json!({"status":"success","result":{
            "title_defaults":{"stream":"automatic","session":"manual"}
        }})
    );
    child.request(2, json!({"kind":"context"}));
    let context = child.response(2);
    assert_eq!(context["outcome"]["status"], "success", "{context}");
    assert_eq!(
        context["outcome"]["result"]["title_defaults"]["session"],
        "manual"
    );
    f.ok(&["config", "set", "naming.session", "automatic"], None);
    child.request(3, json!({"kind":"context"}));
    assert_eq!(
        child.response(3)["outcome"]["result"]["title_defaults"]["session"],
        "automatic"
    );
    assert!(!f.directory.path().join("state/streams/native").exists());
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn sync_and_diagnostic_capture_use_application_operations_without_saved_layouts() {
    let f = Fixture::new();
    let (context, _) = f.capture();
    let id = context["stream"]["id"].as_str().unwrap();
    let mut child = f.start();
    let expected = f.ok(&["agent", "capture-inputs", "--stream-id", id], None);
    child.request(
        1,
        json!({"kind":"capture_inputs","target":{"kind":"stream","stream_id":id}}),
    );
    assert_eq!(child.response(1)["outcome"]["result"], expected);
    child.request(2, json!({"kind":"sync","event":"start"}));
    assert_eq!(child.response(2)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn blocked_catalog_read_does_not_gate_preview_and_clean_eof_drains_it() {
    let f = Fixture::new();
    let (catalog, request) = f.capture();
    let mut child = f.start();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root().join(".catalog.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    child.request(1, json!({"kind":"list_streams"}));
    child.request(
        2,
        json!({"kind":"preview","catalog":catalog,"action":"apply","request":request}),
    );
    assert_eq!(child.response(2)["outcome"]["status"], "success");
    child.send(json!({"type":"cancel","id":999}));
    child.input.take();
    drop(lock);
    assert_eq!(child.response(1)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn native_fifo_can_wait_on_a_save_without_gating_catalog_work_and_cancelled_tail_never_starts() {
    let f = Fixture::new();
    let (_, request) = f.capture();
    let mut child = f.start();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root().join(".catalog.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    child.request(1, json!({"kind":"apply","request":request}));
    child.request(2, json!({"kind":"open_card"}));
    child.send(json!({"type":"cancel","id":2}));
    child.request(
        3,
        json!({"kind":"set_title_default","target":"stream","mode":"manual"}),
    );
    assert_eq!(child.response(3)["outcome"]["status"], "success");
    drop(lock);
    assert_eq!(child.response(1)["outcome"]["status"], "success");
    let cancelled = child.response(2);
    assert_eq!(cancelled["outcome"]["status"], "error");
    assert!(
        cancelled["outcome"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cancelled before dispatch")
    );
    assert!(!f.directory.path().join("state/streams/native").exists());
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn clean_eof_is_bounded_even_when_a_catalog_lock_never_releases() {
    let f = Fixture::new();
    let mut child = f.start();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root().join(".catalog.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    child.request(1, json!({"kind":"list_streams"}));
    child.input.take();
    let started = std::time::Instant::now();
    let (code, stderr) = child.finish();
    assert_ne!(code, 0);
    assert!(stderr.contains("shutdown timed out"), "{stderr}");
    assert!(started.elapsed() < Duration::from_secs(12));
    drop(lock);
}

#[test]
fn queued_saves_preserve_independent_changes_and_refuse_a_stale_change() {
    let f = Fixture::new();
    let (context, request) = f.capture();
    let mut child = f.start();
    let mut title = request.clone();
    title["operation_id"] = json!("title");
    title["changes"] = json!({"title":"Renamed"});
    child.request(1, json!({"kind":"apply","request":request}));
    child.request(2, json!({"kind":"apply","request":title}));
    assert_eq!(child.response(1)["outcome"]["status"], "success");
    assert_eq!(child.response(2)["outcome"]["status"], "success");
    let id = context["stream"]["id"].as_str().unwrap();
    let stream = &f.ok(&["show", id], None)["stream"];
    assert_eq!(stream["title"], "Renamed");
    assert_eq!(stream["goal"], "Captured");
    let mut stale = request;
    stale["operation_id"] = json!("stale");
    stale["changes"] = json!({"goal":"Conflicting"});
    child.request(3, json!({"kind":"apply","request":stale}));
    let refused = child.response(3);
    assert_eq!(refused["outcome"]["status"], "success");
    assert_eq!(
        refused["outcome"]["result"]["save"]["domain"]["outcome"],
        "conflict"
    );
    assert_eq!(f.ok(&["show", id], None)["stream"]["goal"], "Captured");
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn outside_herdr_lifecycle_is_inactive_and_explicit_effects_refuse() {
    let f = Fixture::new();
    let mut child = f.start();
    for (id, kind) in [(1, "session_started"), (2, "attach")] {
        child.request(id, json!({"kind":kind}));
        assert_eq!(
            child.response(id)["outcome"],
            json!({"status":"success","result":{"native":"inactive","reason":"outside_herdr"}})
        );
    }
    for (id, kind) in [(3, "open_card"), (4, "close_card")] {
        child.request(id, json!({"kind":kind}));
        assert_eq!(child.response(id)["outcome"]["status"], "error");
    }
    child.request(5, json!({"kind":"list_streams"}));
    assert_eq!(child.response(5)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn malformed_protocol_exits_even_with_a_blocked_request() {
    let f = Fixture::new();
    let mut child = f.start();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(f.root().join(".catalog.lock"))
        .unwrap();
    assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
    child.request(1, json!({"kind":"list_streams"}));
    child
        .input
        .as_mut()
        .unwrap()
        .write_all(b"not json\n")
        .unwrap();
    match child.frames.recv_timeout(Duration::from_secs(10)) {
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        Ok(Ok(frame)) => {
            // Malformed input can cancel request 1 before its worker reaches the lock.
            assert_eq!(frame["type"], "response", "{frame}");
            assert_eq!(frame["id"], 1, "{frame}");
            assert_eq!(frame["outcome"]["status"], "error", "{frame}");
            assert_eq!(
                frame["outcome"]["error"]["message"], "request cancelled before dispatch",
                "{frame}"
            );
            assert!(matches!(
                child.frames.recv_timeout(Duration::from_secs(10)),
                Err(mpsc::RecvTimeoutError::Disconnected)
            ));
        }
        other => panic!("unexpected frame before protocol shutdown: {other:?}"),
    }
    let (code, stderr) = child.finish();
    assert_ne!(code, 0);
    assert!(!stderr.is_empty());
    drop(lock);
}

#[test]
fn malformed_protocol_has_diagnostics_but_never_uses_cli_or_stdin_adapters() {
    let f = Fixture::new();
    for text in [
        "not json\n",
        "{}\n",
        "{\"type\":\"wat\",\"id\":1}\n",
        "{\"type\":\"cancel\"}\n",
        "{\"type\":\"cancel\",\"id\":1,\"extra\":true}\n",
        "{\"type\":\"request\",\"id\":1,\"argv\":[\"create\",\"Forbidden\"]}\n",
        "{\"type\":\"cancel\",\"id\":1,\"id\":2}\n",
        "{\"type\":\"request\",\"id\":1,\"request\":{\"kind\":\"apply\",\"request\":{\"changes\":{\"goal\":\"first\",\"goal\":\"second\"}}}}\n",
        "{\"type\":\"request\",\"id\":1,\"request\":{\"kind\":\"preview\",\"catalog\":{},\"action\":\"apply\",\"action\":\"enroll\",\"request\":{}}}\n",
        "{\"type\":\"request\",\"id\":1,\"request\":{\"kind\":\"enroll\",\"request\":{\"session\":{\"title\":\"first\",\"title\":\"second\"}}}}\n",
        "{\"type\":\"request\",\"id\":1,\"request\":{\"kind\":\"open_card\",\"expect\":{}}}\n",
        "{\"type\":\"request\",\"id\":1,\"request\":{\"kind\":\"set_title_default\",\"target\":\"host\",\"mode\":\"manual\"}}\n",
        "{\"type\":\"request\",\"id\":1,\"request\":{\"kind\":\"set_title_default\",\"target\":\"session\",\"mode\":\"inherit\"}}\n",
        "{\"type\":\"cancel\",\"id\":1}",
        "\n",
        "{\"type\":\"cancel\",\"id\":0}\n",
        "{\"type\":\"cancel\",\"id\":-1}\n",
        "{\"type\":\"cancel\",\"id\":1.5}\n",
        "{\"type\":\"cancel\",\"id\":9007199254740992}\n",
        "{\"type\":\"request\",\"id\":1,\"request\":{\"kind\":\"list_streams\"}}\n{\"type\":\"request\",\"id\":1,\"request\":{\"kind\":\"list_streams\"}}\n",
    ] {
        let mut child = f.start();
        child
            .input
            .as_mut()
            .unwrap()
            .write_all(text.as_bytes())
            .unwrap();
        child.input.take();
        let (code, stderr) = child.finish();
        assert_ne!(code, 0, "{text}");
        assert!(!stderr.is_empty(), "{text}");
    }
    assert!(
        f.ok(&["list"], None)["streams"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}
