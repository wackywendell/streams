use super::*;
use crate::support::herdr::{FakeHerdr, Reply};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
};
use streams::{application, config::Config, runtime::native_markers::stream_tokens};

#[path = "cards.rs"]
mod cards;

type PendingSnapshot = (mpsc::Sender<()>, Receiver<()>);

#[allow(dead_code)]
struct SnapshotGate {
    entered: Receiver<()>,
    release: Option<mpsc::Sender<()>>,
}
impl Drop for SnapshotGate {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

#[allow(dead_code)]
struct NativeFixture {
    server: FakeHerdr,
    fixture: Fixture,
    snapshot: Arc<Mutex<Value>>,
    rejected: Arc<AtomicBool>,
    fail_next_snapshot: Arc<AtomicBool>,
    snapshot_gate: Arc<Mutex<Option<PendingSnapshot>>>,
    viewers: Arc<Mutex<Vec<crate::support::pty::Pty>>>,
    stream: Value,
    /// `null` until the fixture enrolls the conversation.
    session: Value,
    id: String,
    _socket_directory: tempfile::TempDir,
}

impl NativeFixture {
    fn new() -> Self {
        Self::with_enrollment(true)
    }

    fn with_enrollment(enroll: bool) -> Self {
        let fixture = Fixture::new();
        let id = uuid::Uuid::new_v4().to_string();
        let created_stream = fixture.ok(&["stream", "create", "--title", "Work"], None);
        let stream_key = created_stream["key"].as_str().unwrap();
        let stream = fixture.ok(&["stream", "show", stream_key], None);
        let session = if enroll {
            let pair = format!("pi/{id}");
            let created_session = fixture.ok(
                &[
                    "session", "create", &pair, "--stream", stream_key, "--title", "Before",
                ],
                None,
            );
            fixture.ok(
                &["session", "show", created_session["key"].as_str().unwrap()],
                None,
            )
        } else {
            Value::Null
        };
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let socket = directory.path().join("herdr.sock");
        let pane = json!({"pane_id":"pane", "terminal_id":"terminal", "workspace_id":"workspace", "tab_id":"tab",
            "cwd":fixture.directory.path(), "agent":"pi", "agent_session":{"agent":"pi","kind":"id","value":id}, "tokens":{}});
        let snapshot = Arc::new(Mutex::new(json!({
            "workspaces":[{"workspace_id":"workspace","label":"Fixture","number":1,"tokens":{}}],
            "tabs":[{"workspace_id":"workspace","tab_id":"tab","label":"1","number":1}],
            "panes":[pane], "agents":[pane], "layouts":[{"workspace_id":"workspace","tab_id":"tab","panes":[{"pane_id":"pane"}]}]
        })));
        let rejected = Arc::new(AtomicBool::new(false));
        let fail_next_snapshot = Arc::new(AtomicBool::new(false));
        let snapshot_gate = Arc::new(Mutex::new(None::<PendingSnapshot>));
        let viewers = Arc::new(Mutex::new(Vec::new()));
        let server = FakeHerdr::start(&socket, {
            let snapshot = snapshot.clone();
            let rejected = rejected.clone();
            let fail_next_snapshot = fail_next_snapshot.clone();
            let snapshot_gate = snapshot_gate.clone();
            let viewers = viewers.clone();
            move |method, params| match method {
                "events.subscribe" if rejected.load(Ordering::Acquire) => {
                    Reply::Error(json!({"code":"denied","message":"fixture subscription denied"}))
                }
                "events.subscribe" => Reply::Result(json!({"type":"subscription_started"})),
                "session.snapshot" => {
                    if fail_next_snapshot.swap(false, Ordering::AcqRel) {
                        return Reply::Error(
                            json!({"code":"unavailable","message":"fixture native observation failed"}),
                        );
                    }
                    if let Some((entered, release)) = snapshot_gate.lock().unwrap().take() {
                        entered.send(()).unwrap();
                        release.recv().unwrap();
                    }
                    Reply::Result(
                        json!({"type":"session_snapshot","snapshot":snapshot.lock().unwrap().clone()}),
                    )
                }
                "workspace.report_metadata" | "pane.report_metadata" => {
                    let mut snapshot = snapshot.lock().unwrap();
                    let (collections, key) = if method == "workspace.report_metadata" {
                        (&["workspaces"][..], "workspace_id")
                    } else {
                        (&["panes", "agents"][..], "pane_id")
                    };
                    for collection in collections {
                        if let Some(target) = snapshot[*collection]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .find(|row| row[key] == params[key])
                        {
                            let tokens = target["tokens"].as_object_mut().unwrap();
                            for (key, value) in params["tokens"].as_object().unwrap() {
                                if value.is_null() {
                                    tokens.remove(key);
                                } else {
                                    tokens.insert(key.clone(), value.clone());
                                }
                            }
                        }
                    }
                    Reply::Result(json!({"type":"ok"}))
                }
                "plugin.list" => {
                    let root = PathBuf::from(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/integrations/herdr-work-cards"
                    ))
                    .canonicalize()
                    .unwrap();
                    Reply::Result(json!({"type":"plugin_list","plugins":[{
                        "plugin_id":"streams-rust-work-cards","enabled":true,
                        "plugin_root":root,"manifest_path":root.join("streams-rust-herdr-plugin.toml")
                    }]}))
                }
                "plugin.pane.open" if params["entrypoint"] == "artifact" => {
                    let mut command = Command::new("/bin/sh");
                    command
                        .arg(concat!(
                            env!("CARGO_MANIFEST_DIR"),
                            "/integrations/herdr-work-cards/bin/open-artifact.sh"
                        ))
                        .current_dir(params["cwd"].as_str().unwrap())
                        .env_clear()
                        .env("TERM", "xterm-256color")
                        .envs(
                            params["env"]
                                .as_object()
                                .unwrap()
                                .iter()
                                .map(|(key, value)| (key, value.as_str().unwrap())),
                        );
                    viewers
                        .lock()
                        .unwrap()
                        .push(crate::support::pty::Pty::spawn(command, false));
                    let mut snapshot = snapshot.lock().unwrap();
                    let pane = json!({"pane_id":"artifact", "terminal_id":"artifact-terminal",
                        "workspace_id":params["workspace_id"], "tab_id":"artifact-tab", "cwd":params["cwd"], "tokens":{}});
                    snapshot["panes"].as_array_mut().unwrap().push(pane.clone());
                    snapshot["tabs"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"workspace_id":params["workspace_id"],
                        "tab_id":"artifact-tab", "number":2, "label":"Session output"}));
                    snapshot["layouts"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"workspace_id":params["workspace_id"],
                        "tab_id":"artifact-tab", "panes":[{"pane_id":"artifact"}]}));
                    Reply::Result(json!({"type":"plugin_pane_opened","plugin_pane":{
                        "plugin_id":"streams-rust-work-cards","entrypoint":"artifact","pane":pane}}))
                }
                "tab.focus" => {
                    let snapshot = snapshot.lock().unwrap();
                    let tab = snapshot["tabs"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|tab| tab["tab_id"] == params["tab_id"])
                        .unwrap();
                    Reply::Result(json!({"type":"tab_info","tab":tab}))
                }
                "plugin.pane.open" => {
                    let launch: streams::runtime::native_cards::LaunchedCardContext =
                        serde_json::from_str(
                            params["env"]["STREAMS_CARDS_CONTEXT"].as_str().unwrap(),
                        )
                        .unwrap();
                    let mut snapshot = snapshot.lock().unwrap();
                    let owner = snapshot["panes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|pane| pane["pane_id"] == params["target_pane_id"])
                        .unwrap()
                        .clone();
                    let mut tokens = stream_tokens(
                        &launch.scope.catalog_source,
                        &launch.scope.host_id,
                        &launch.scope.stream_id,
                    );
                    tokens.remove("strm_stream");
                    tokens.insert("strm_session".into(), launch.scope.session_id);
                    tokens.insert("strm_role".into(), "card".into());
                    let number = snapshot["panes"].as_array().unwrap().len();
                    let pane = json!({"pane_id":format!("card-{number}"),
                        "terminal_id":format!("card-terminal-{number}"),"workspace_id":owner["workspace_id"],
                        "tab_id":owner["tab_id"],"cwd":params["cwd"],"label":"card","tokens":tokens});
                    snapshot["panes"].as_array_mut().unwrap().push(pane.clone());
                    snapshot["layouts"][0]["panes"]
                        .as_array_mut()
                        .unwrap()
                        .push(json!({"pane_id":pane["pane_id"]}));
                    Reply::Result(json!({"type":"plugin_pane_opened","plugin_pane":{
                        "plugin_id":"streams-rust-work-cards","entrypoint":"card","pane":pane
                    }}))
                }
                "plugin.pane.close" => {
                    let mut snapshot = snapshot.lock().unwrap();
                    snapshot["panes"]
                        .as_array_mut()
                        .unwrap()
                        .retain(|pane| pane["pane_id"] != params["pane_id"]);
                    for layout in snapshot["layouts"].as_array_mut().unwrap() {
                        layout["panes"]
                            .as_array_mut()
                            .unwrap()
                            .retain(|pane| pane["pane_id"] != params["pane_id"]);
                    }
                    Reply::Result(json!({"type":"plugin_pane_closed","pane_id":params["pane_id"]}))
                }
                "pane.rename" => {
                    let mut snapshot = snapshot.lock().unwrap();
                    let pane = snapshot["panes"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|pane| pane["pane_id"] == params["pane_id"])
                        .unwrap();
                    pane["label"] = params["label"].clone();
                    Reply::Result(json!({"type":"pane_info","pane":pane}))
                }
                "tab.rename" => {
                    let mut snapshot = snapshot.lock().unwrap();
                    let tab = snapshot["tabs"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|tab| tab["tab_id"] == params["tab_id"])
                        .unwrap();
                    tab["label"] = params["label"].clone();
                    Reply::Result(json!({"type":"tab_info","tab":tab}))
                }
                "workspace.rename" => {
                    let mut snapshot = snapshot.lock().unwrap();
                    let workspace = snapshot["workspaces"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|ws| ws["workspace_id"] == params["workspace_id"])
                        .unwrap();
                    workspace["label"] = params["label"].clone();
                    Reply::Result(json!({"type":"workspace_info","workspace":workspace}))
                }
                other => panic!("unexpected card watch effect: {other}"),
            }
        });
        let home = fixture.directory.path();
        let environment = BTreeMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            (
                "XDG_CONFIG_HOME".into(),
                home.join("config").to_string_lossy().into_owned(),
            ),
            (
                "XDG_DATA_HOME".into(),
                home.join("data").to_string_lossy().into_owned(),
            ),
            (
                "XDG_STATE_HOME".into(),
                home.join("state").to_string_lossy().into_owned(),
            ),
        ]);
        let config = Config::new(&environment, home).unwrap();
        let opened = application::open_records(&config).unwrap();
        snapshot.lock().unwrap()["workspaces"][0]["tokens"] = json!(stream_tokens(
            opened.source_id(),
            opened.host().as_str(),
            stream["id"].as_str().unwrap()
        ));
        fixture.ok(&["config", "set", "frontend.mode", "herdr"], None);
        fixture.ok(&["config", "set", "herdr.cards.auto_open", "manual"], None);
        fixture.ok(&["config", "set", "herdr.naming.workspace", "false"], None);
        fixture.ok(&["config", "set", "herdr.naming.tab", "false"], None);
        fixture.ok(&["config", "set", "pi.naming.session", "true"], None);
        fixture.ok(
            &["config", "set", "herdr.pane_height_fraction", "0.5"],
            None,
        );
        Self {
            server,
            fixture,
            snapshot,
            rejected,
            fail_next_snapshot,
            snapshot_gate,
            viewers,
            stream,
            session,
            id,
            _socket_directory: directory,
        }
    }

    fn command(&self) -> Command {
        let mut command = self.fixture.command();
        command
            .env("HERDR_ENV", "1")
            .env("HERDR_SOCKET_PATH", &self.server.socket)
            .env("HERDR_PANE_ID", "pane")
            .env("HERDR_TAB_ID", "tab")
            .env("HERDR_WORKSPACE_ID", "workspace");
        command
    }

    fn target(&self) -> String {
        format!("pi/{}", self.id)
    }

    fn watch(&self) -> Watcher {
        Watcher::start(self.command(), &self.id)
    }

    fn calls(&self, method: &str) -> usize {
        self.server
            .requests()
            .iter()
            .filter(|(m, _)| m == method)
            .count()
    }

    fn wait_for(&self, predicate: impl Fn(&[(String, Value)]) -> bool, message: &str) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !predicate(&self.server.requests()) {
            assert!(
                std::time::Instant::now() < deadline,
                "{message}; requests={:?}",
                self.server
                    .requests()
                    .iter()
                    .map(|(m, _)| m)
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn block_next_snapshot(&self) -> SnapshotGate {
        let (entered, observed) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        *self.snapshot_gate.lock().unwrap() = Some((entered, wait));
        SnapshotGate {
            entered: observed,
            release: Some(release),
        }
    }
}

#[test]
fn card_open_outside_herdr_fails() {
    let fixture = Fixture::new();
    let target = format!("pi/{}", uuid::Uuid::new_v4());
    let (code, value) = fixture.cli(&["card", "open", &target], None);
    assert_ne!(code, 0, "{value}");
    assert!(
        value["error"]["message"]
            .as_str()
            .unwrap()
            .contains("require Herdr"),
        "{value}"
    );
}

#[test]
fn card_open_for_an_unenrolled_session_fails_and_saves_automatic_mode() {
    let f = NativeFixture::with_enrollment(false);
    let target = f.target();
    let (code, value) = f
        .fixture
        .cli_with(f.command(), &["card", "open", &target], None);
    assert_ne!(code, 0, "{value}");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("native action failed"),
        "expected failure report, got: {message}"
    );
    let mode = f.fixture.ok(&["card", "mode", &target], None);
    assert_eq!(mode["session"], "automatic");
}

#[test]
fn card_close_keeps_watch_from_reopening() {
    let f = NativeFixture::new();
    f.fixture.ok(
        &["config", "set", "herdr.cards.auto_open", "automatic"],
        None,
    );
    let watcher = f.watch();
    f.wait_for(
        |requests| requests.iter().any(|(m, _)| m == "plugin.pane.open"),
        "card watch should auto-open on startup",
    );

    // Close via `card close`: closes the pane and saves manual mode.
    let target = f.target();
    let (code, value) = f
        .fixture
        .cli_with(f.command(), &["card", "close", &target], None);
    assert_eq!(code, 0, "{value}");
    assert_eq!(value["closed"], true);
    let mode = f.fixture.ok(&["card", "mode", &target], None);
    assert_eq!(mode["session"], "manual");
    assert_eq!(mode["effective"], "manual");

    // A Session save starts a refresh pass, which asks Herdr for a snapshot.
    let snapshots_before = f.calls("session.snapshot");
    f.fixture.ok(
        &[
            "session",
            "edit",
            f.session["key"].as_str().unwrap(),
            "--state",
            "Updated after close",
        ],
        None,
    );
    f.wait_for(
        |_| f.calls("session.snapshot") > snapshots_before,
        "card watch should observe the Session save",
    );
    // The watch handles one wake at a time, so it finishes that pass before the end of
    // stdin; only then is the count of opens final.
    let (code, stderr) = watcher.finish();
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(
        f.calls("plugin.pane.open"),
        1,
        "card watch must not reopen a card after card close saved manual mode"
    );
}

#[test]
fn watch_closes_card_when_stdin_ends() {
    let f = NativeFixture::new();
    f.fixture.ok(
        &["config", "set", "herdr.cards.auto_open", "automatic"],
        None,
    );
    let watcher = f.watch();
    f.wait_for(
        |requests| requests.iter().any(|(m, _)| m == "plugin.pane.open"),
        "card watch should auto-open on startup",
    );
    assert!(
        !f.server
            .requests()
            .iter()
            .any(|(m, _)| m == "plugin.pane.close")
    );

    // Closing stdin simulates Pi exiting: card watch closes the card and exits 0.
    let (code, stderr) = watcher.finish();
    assert_eq!(code, 0, "{stderr}");
    assert!(
        f.server
            .requests()
            .iter()
            .any(|(m, _)| m == "plugin.pane.close"),
        "card watch must close the card when stdin ends"
    );
    // Closing on Pi exit does not flip the saved preference to manual.
    let mode = f.fixture.ok(&["card", "mode", &f.target()], None);
    assert_eq!(mode["session"], "automatic");
}

#[test]
fn card_watch_opens_the_card_when_the_session_is_enrolled_after_it_starts() {
    let f = NativeFixture::with_enrollment(false);
    f.fixture.ok(
        &["config", "set", "herdr.cards.auto_open", "automatic"],
        None,
    );
    let watcher = f.watch();
    // The first snapshot follows the file watch's baseline, so the enrollment below is
    // seen as a file event, not at startup.
    f.wait_for(
        |_| f.calls("session.snapshot") > 0,
        "card watch should observe Herdr after watching its files",
    );

    f.fixture.ok(
        &[
            "session",
            "create",
            &f.target(),
            "--stream",
            f.stream["key"].as_str().unwrap(),
            "--title",
            "Enrolled later",
        ],
        None,
    );
    f.wait_for(
        |_| f.calls("plugin.pane.open") > 0,
        "card watch should open the card once the Session is enrolled",
    );
    let (code, stderr) = watcher.finish();
    assert_eq!(code, 0, "{stderr}");
}

#[test]
fn ordinary_native_refreshes_associate_an_unmarked_workspace_without_changing_membership() {
    let f = NativeFixture::new();
    f.snapshot.lock().unwrap()["workspaces"][0]["tokens"] = json!({});
    let watcher = f.watch();
    f.wait_for(
        |requests| {
            requests
                .iter()
                .any(|(m, _)| m == "workspace.report_metadata")
        },
        "card watch should associate an unmarked workspace with its Stream",
    );
    let current = f.fixture.ok(
        &["session", "show", f.session["key"].as_str().unwrap()],
        None,
    );
    assert_eq!(current["stream_id"], f.session["stream_id"]);
    let (code, stderr) = watcher.finish();
    assert_eq!(code, 0, "{stderr}");
}
