use std::{
    io::Write,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    time::Duration,
};

use serde_json::{Value, json};

#[path = "card_watch/subscriptions.rs"]
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
        self.cli_with(self.command(), argv, input)
    }

    fn cli_with(&self, mut command: Command, argv: &[&str], input: Option<&str>) -> (i32, Value) {
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
}

struct Watcher {
    child: Option<Child>,
    input: Option<ChildStdin>,
}

impl Watcher {
    fn start(mut command: Command, session: &str) -> Self {
        command.args(["card", "watch", &format!("pi/{session}"), "--plugin-root"]);
        let mut child = command
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
        Self {
            child: Some(child),
            input,
        }
    }

    fn close_stdin(&mut self) {
        self.input.take();
    }

    fn finish(mut self) -> (i32, String) {
        self.close_stdin();
        let output = self.child.take().unwrap().wait_with_output().unwrap();
        (
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.input.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn card_watch_exits_at_once_when_frontend_is_not_herdr() {
    let fixture = Fixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    let mut watcher = Watcher::start(fixture.command(), &id);
    // Keep stdin open briefly: card watch should still exit immediately without waiting for EOF.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = watcher.child.as_mut().unwrap().try_wait().unwrap() {
            break status;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "card watch did not exit immediately when frontend.mode=none"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "unexpected exit status: {status}");
}

#[test]
fn save_writes_streams_saved_on_hc_terminal() {
    use crate::support::hc::{FakeCore, Reply};
    let fixture = Fixture::new();
    let core = FakeCore::with_handler(
        |request, _| {
            assert_eq!(request["method"], "terminal.label");
            assert_eq!(request["params"]["terminal"], "t42");
            let token = request["params"]["labels"]["streams.saved"]
                .as_str()
                .expect("streams.saved token")
                .to_owned();
            assert!(!token.is_empty());
            Reply::Value(json!({
                "address": "t42",
                "state": "live",
                "labels": { "streams.saved": token }
            }))
        },
        1,
    );
    let mut command = fixture.command();
    command
        .env("HC_TERMINAL_ID", "t42")
        .env("HC_SOCKET", &core.socket);
    let (code, value) = fixture.cli_with(command, &["create", "Save Notice Work"], None);
    assert_eq!(code, 0, "{value}");
    let calls = core.finish();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["method"], "terminal.label");
    assert_eq!(calls[1]["params"]["terminal"], "t42");
    assert!(
        calls[1]["params"]["labels"]["streams.saved"]
            .as_str()
            .is_some_and(|token| !token.is_empty())
    );
}
