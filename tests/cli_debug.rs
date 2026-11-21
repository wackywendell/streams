use serde_json::{Value, json};
use std::{fs, process::Command};

struct Fixture {
    root: tempfile::TempDir,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config/streams");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("config.toml"), "not valid TOML").unwrap();
        Self { root }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_streams"));
        command
            .env_clear()
            .env("HOME", self.root.path())
            .env("XDG_CONFIG_HOME", self.root.path().join("config"))
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_STATE_HOME", self.root.path().join("state"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default());
        command
    }
    fn runner(&self, source: &str) -> std::path::PathBuf {
        let runner = self.root.path().join("runner.mjs");
        fs::write(&runner, source).unwrap();
        runner
    }
    fn assert_no_catalog(&self) {
        assert!(!self.root.path().join("data").exists());
        assert!(!self.root.path().join("state").exists());
        assert_eq!(
            fs::read_to_string(self.root.path().join("config/streams/config.toml")).unwrap(),
            "not valid TOML"
        );
    }
}

#[test]
fn debug_help_needs_neither_catalog_nor_sdk() {
    let fixture = Fixture::new();
    let output = fixture
        .command()
        .env("STREAMS_PI_SDK_ENTRY", "/missing/sdk.mjs")
        .args(["debug", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    assert!(String::from_utf8_lossy(&output.stdout).contains("Usage:"));
    fixture.assert_no_catalog();
}

#[test]
fn debug_preserves_literal_arguments_and_pins_the_cli_selector() {
    let fixture = Fixture::new();
    let runner = fixture.runner(
        "console.log(JSON.stringify({args:process.argv.slice(2),bin:process.env.STREAMS_BIN_PATH}));",
    );
    for arguments in [
        vec![
            "debug",
            "prompts",
            "prepare",
            "literal identity",
            "--prompt",
            "a file",
            "--json",
        ],
        vec![
            "--json",
            "debug",
            "prompts",
            "prepare",
            "literal identity",
            "--prompt",
            "a file",
        ],
    ] {
        let output = fixture
            .command()
            .env("STREAMS_PROMPT_LAB_RUNNER", &runner)
            .env("STREAMS_BIN_PATH", "/wrong/installed/streams")
            .args(arguments)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value["args"],
            json!([
                "prepare",
                "literal identity",
                "--prompt",
                "a file",
                "--json"
            ])
        );
        assert_eq!(
            fs::canonicalize(value["bin"].as_str().unwrap()).unwrap(),
            fs::canonicalize(env!("CARGO_BIN_EXE_streams")).unwrap()
        );
        fixture.assert_no_catalog();
    }
}

#[test]
fn debug_retains_runner_exit_status_without_initializing_catalog() {
    let fixture = Fixture::new();
    let runner = fixture.runner("process.exit(7);");
    let status = fixture
        .command()
        .env("STREAMS_PROMPT_LAB_RUNNER", runner)
        .args(["debug", "prompts", "list", "request"])
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(7));
    fixture.assert_no_catalog();
}

#[test]
fn invalid_explicit_runner_does_not_fall_back() {
    let fixture = Fixture::new();
    for runner in ["relative.mjs", "/missing/runner.mjs"] {
        let output = fixture
            .command()
            .env("STREAMS_PROMPT_LAB_RUNNER", runner)
            .args(["debug", "prompts", "--help"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("STREAMS_PROMPT_LAB_RUNNER"));
        fixture.assert_no_catalog();
    }
}
