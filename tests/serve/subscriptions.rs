use super::*;
use crate::support::herdr::{FakeHerdr, Reply};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use streams_temper::{application, config::Config, runtime::native_markers::stream_tokens};

#[path = "cards.rs"]
mod cards;

type PendingSnapshot = (mpsc::Sender<()>, Receiver<()>);

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

struct NativeFixture {
    server: FakeHerdr,
    fixture: Fixture,
    snapshot: Arc<Mutex<Value>>,
    rejected: Arc<AtomicBool>,
    fail_next_snapshot: Arc<AtomicBool>,
    snapshot_gate: Arc<Mutex<Option<PendingSnapshot>>>,
    session: Value,
    id: String,
    _socket_directory: tempfile::TempDir,
}
impl NativeFixture {
    fn new() -> Self {
        let fixture = Fixture::new();
        let id = uuid::Uuid::new_v4().to_string();
        let stream = fixture.ok(&["create", "Work"], None)["stream"].clone();
        let session = fixture.ok(
            &[
                "session",
                "attach",
                stream["id"].as_str().unwrap(),
                "--harness",
                "pi",
                "--session-id",
                &id,
                "--title",
                "Before",
            ],
            None,
        )["session"]
            .clone();
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
        let server = FakeHerdr::start(&socket, {
            let snapshot = snapshot.clone();
            let rejected = rejected.clone();
            let fail_next_snapshot = fail_next_snapshot.clone();
            let snapshot_gate = snapshot_gate.clone();
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
                "plugin.pane.open" => {
                    let launch: streams_temper::runtime::native_cards::LaunchedCardContext =
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
                    // Stand in for the card process publishing its launch identity.
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
                other => panic!("unexpected serve effect: {other}"),
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
        fixture.ok(&["config", "set", "cards.auto_open", "manual"], None);
        fs::write(
            home.join("work-cards.json"),
            json!({
                "workspace_naming":false,"tab_naming":false,"session_naming":true,"pane_height_fraction":0.5
            })
            .to_string(),
        )
        .unwrap();
        Self {
            server,
            fixture,
            snapshot,
            rejected,
            fail_next_snapshot,
            snapshot_gate,
            session,
            id,
            _socket_directory: directory,
        }
    }
    fn command(&self) -> Command {
        let mut command = self.fixture.command();
        command
            .env("HERDR_ENV", "1")
            .env(
                "WORK_CARDS_CONFIG_PATH",
                self.fixture.directory.path().join("work-cards.json"),
            )
            .env("HERDR_SOCKET_PATH", &self.server.socket)
            .env("HERDR_PANE_ID", "pane")
            .env("HERDR_TAB_ID", "tab")
            .env("HERDR_WORKSPACE_ID", "workspace");
        command
    }
    fn begin(&self) -> Transport {
        let child = Transport::start(self.command(), &self.id);
        assert_eq!(child.next(), json!({"type":"ready","protocol":2}));
        child
    }
    fn start(&self) -> (Transport, Value) {
        let mut child = self.begin();
        child.request(10_000, json!({"kind":"attach"}));
        let view = child.next();
        assert_eq!(view["type"], "view", "{view}");
        let response = child.response(10_000);
        assert_eq!(response["outcome"]["status"], "success", "{response}");
        (child, view)
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
    fn set_location(&self, pane: &str, tab: &str) {
        let mut snapshot = self.snapshot.lock().unwrap();
        for key in ["panes", "agents"] {
            snapshot[key][0]["pane_id"] = json!(pane);
            snapshot[key][0]["tab_id"] = json!(tab);
        }
        snapshot["tabs"][0]["tab_id"] = json!(tab);
        snapshot["layouts"][0]["tab_id"] = json!(tab);
        snapshot["layouts"][0]["panes"][0]["pane_id"] = json!(pane);
    }
    fn move_to(&self, previous: &str, pane: &str, tab: &str) {
        self.set_location(pane, tab);
        self.server.emit(
            json!({"event":"pane_moved", "data":{"type":"pane_moved", "previous_pane_id":previous,
            "previous_tab_id":"tab","previous_workspace_id":"workspace",
            "pane":{"pane_id":pane,"tab_id":tab,"workspace_id":"workspace"}}}),
        );
    }
    fn move_to_workspace(&self, workspace: &str) {
        let mut snapshot = self.snapshot.lock().unwrap();
        let previous = snapshot["panes"][0].clone();
        let pane = format!("{workspace}-pane");
        let tab = format!("{workspace}-tab");
        for key in ["panes", "agents"] {
            snapshot[key][0]["workspace_id"] = json!(workspace);
            snapshot[key][0]["tab_id"] = json!(tab);
            snapshot[key][0]["pane_id"] = json!(pane);
        }
        snapshot["tabs"][0]["workspace_id"] = json!(workspace);
        snapshot["tabs"][0]["tab_id"] = json!(tab);
        snapshot["layouts"][0]["workspace_id"] = json!(workspace);
        snapshot["layouts"][0]["tab_id"] = json!(tab);
        snapshot["layouts"][0]["panes"][0]["pane_id"] = json!(pane);
        drop(snapshot);
        self.server.emit(json!({"event":"pane_moved","data":{
            "type":"pane_moved","previous_pane_id":previous["pane_id"],
            "previous_tab_id":previous["tab_id"],"previous_workspace_id":previous["workspace_id"],
            "pane":{"pane_id":pane,"tab_id":tab,"workspace_id":workspace,"terminal_id":"terminal"}
        }}));
    }
}

#[test]
fn idle_move_emits_only_ui_facts_and_irrelevant_events_spend_zero_observations() {
    let f = NativeFixture::new();
    let (mut child, initial) = f.start();
    assert_eq!(
        initial,
        json!({"type":"view","view":{"desired_pi_name":"Before"}})
    );
    f.server.clear();
    for _ in 0..32 {
        f.server.emit(json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"unrelated"}}}));
    }
    f.move_to("pane", "moved", "next-tab");
    assert_eq!(child.next(), initial);
    assert_eq!(f.server.methods(), ["session.snapshot"]);
    f.server.emit(
        json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"pane"}}}),
    );
    f.move_to("moved", "again", "last-tab");
    assert_eq!(child.next(), initial);
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["session.snapshot", "session.snapshot"]);
    assert!(child.frames.try_recv().is_err());
}

#[test]
fn native_refresh_does_not_resolve_unrelated_pi_paths_and_rereads_the_owner_next_time() {
    let f = NativeFixture::new();
    let owner = f.fixture.directory.path().join("owner.jsonl");
    fs::write(
        &owner,
        json!({"type":"session","id":f.id}).to_string() + "\n",
    )
    .unwrap();
    {
        let mut snapshot = f.snapshot.lock().unwrap();
        for key in ["panes", "agents"] {
            snapshot[key][0]["agent_session"] = json!({"agent":"pi","kind":"path","value":owner});
        }
        for index in 0..64 {
            let pane = format!("neighbor-{index}");
            let row = json!({"pane_id":pane,"terminal_id":format!("terminal-{index}"),"workspace_id":"workspace","tab_id":"tab",
                "agent":"pi","agent_session":{"agent":"pi","kind":"path","value":f.fixture.directory.path().join(format!("unavailable-{index}.jsonl"))},"tokens":{}});
            snapshot["panes"].as_array_mut().unwrap().push(row.clone());
            snapshot["agents"].as_array_mut().unwrap().push(row);
            snapshot["layouts"][0]["panes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"pane_id":pane}));
        }
    }
    let (mut child, _) = f.start();
    f.server.clear();
    {
        let mut snapshot = f.snapshot.lock().unwrap();
        for key in ["panes", "agents"] {
            snapshot[key][0]["pane_id"] = json!("moved");
        }
        snapshot["layouts"][0]["panes"][0]["pane_id"] = json!("moved");
    }
    f.server.emit(json!({"event":"pane_moved","data":{"type":"pane_moved","previous_pane_id":"pane","previous_tab_id":"tab","previous_workspace_id":"workspace","pane":{"pane_id":"moved","tab_id":"tab","workspace_id":"workspace"}}}));
    let view = child.next();
    assert_eq!(view["type"], "view", "{view}");
    assert_eq!(f.server.methods(), ["session.snapshot"]);
    fs::write(
        &owner,
        json!({"type":"session","id":uuid::Uuid::new_v4().to_string()}).to_string() + "\n",
    )
    .unwrap();
    f.server.emit(json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"moved","tab_id":"tab","workspace_id":"workspace"}}}));
    let warning = child.next();
    assert_eq!(warning["type"], "warning", "{warning}");
    assert!(
        warning["message"]
            .as_str()
            .unwrap()
            .contains("Pi exited or changed conversation"),
        "{warning}"
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn slow_native_observation_never_gates_catalog_responses() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    let gate = f.block_next_snapshot();
    f.server.emit(json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"pane","tab_id":"tab","workspace_id":"workspace"}}}));
    gate.entered.recv_timeout(Duration::from_secs(15)).unwrap();
    child.request(
        1,
        json!({"kind":"preview","catalog":{},"action":"apply","request":{}}),
    );
    child.request(2, json!({"kind":"list_streams"}));
    let mut replies = [child.next(), child.next()];
    replies.sort_by_key(|reply| reply["id"].as_u64().unwrap());
    assert_eq!(replies[0]["outcome"]["status"], "error");
    assert_eq!(replies[1]["outcome"]["status"], "success");
    drop(gate);
    assert_eq!(child.next()["type"], "view");
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["session.snapshot"]);
}

#[test]
fn restarted_child_locates_its_bound_conversation_without_an_inherited_pane_baseline() {
    let f = NativeFixture::new();
    f.set_location("already-moved", "next-tab");
    let (mut child, view) = f.start();
    assert_eq!(view["view"]["desired_pi_name"], "Before");
    f.server.clear();
    f.move_to("already-moved", "moved-again", "last-tab");
    assert_eq!(child.next(), view);
    child.request(1, json!({"kind":"attach"}));
    assert_eq!(
        child.response(1)["outcome"]["result"],
        json!({"native":"active"})
    );
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["session.snapshot", "session.snapshot"]);
}

#[test]
fn terminal_replacement_stops_only_the_affected_pass_and_never_mutates_its_replacement() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    for key in ["panes", "agents"] {
        f.snapshot.lock().unwrap()[key][0]["terminal_id"] = json!("unrelated-terminal");
    }
    f.move_to("pane", "moved", "next-tab");
    let before = f.snapshot.lock().unwrap().clone();
    let warning = child.next();
    assert_eq!(warning["type"], "warning");
    assert!(
        warning["message"]
            .as_str()
            .unwrap()
            .contains("Pi exited or changed conversation")
    );
    for (id, kind) in [(1, "open_card"), (2, "close_card")] {
        child.request(id, json!({"kind":kind}));
        assert_eq!(child.response(id)["outcome"]["status"], "error");
    }
    assert_eq!(*f.snapshot.lock().unwrap(), before);
    assert_eq!(
        f.server.methods(),
        ["session.snapshot", "session.snapshot", "session.snapshot"]
    );
    for key in ["panes", "agents"] {
        f.snapshot.lock().unwrap()[key][0]["terminal_id"] = json!("terminal");
    }
    child.request(3, json!({"kind":"attach"}));
    assert_eq!(
        child.response(3)["outcome"]["result"],
        json!({"native":"active"})
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn external_session_save_updates_ui_and_unrelated_records_do_not_observe() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    f.fixture.ok(&["create", "Unrelated"], None);
    assert!(
        child
            .frames
            .recv_timeout(Duration::from_millis(150))
            .is_err()
    );
    assert!(f.server.methods().is_empty());
    f.fixture.ok(
        &[
            "session",
            "update",
            f.session["id"].as_str().unwrap(),
            "--title",
            "After",
        ],
        None,
    );
    assert_eq!(
        child.next(),
        json!({"type":"view","view":{"desired_pi_name":"After"}})
    );
    assert_eq!(child.finish(), (0, String::new()));
    assert!(
        f.server
            .methods()
            .iter()
            .any(|method| method == "session.snapshot")
    );
    assert!(child.frames.try_recv().is_err());
}

#[test]
fn external_attach_into_an_unmarked_workspace_does_not_create_or_move_work() {
    let f = NativeFixture::new();
    let id = uuid::Uuid::new_v4().to_string();
    {
        let mut snapshot = f.snapshot.lock().unwrap();
        snapshot["workspaces"][0]["tokens"] = json!({});
        for key in ["panes", "agents"] {
            snapshot[key][0]["agent_session"]["value"] = json!(id);
        }
    }
    let streams = f.fixture.ok(&["list"], None);
    let mut child = Transport::start(f.command(), &id);
    assert_eq!(child.next(), json!({"type":"ready","protocol":2}));
    child.request(1, json!({"kind":"session_started"}));
    assert_eq!(
        child.response(1)["outcome"]["result"]["reason"],
        "not_enrolled"
    );
    assert_eq!(
        f.snapshot.lock().unwrap()["workspaces"][0]["tokens"],
        json!({})
    );
    let attached = f.fixture.ok(
        &[
            "session",
            "attach",
            f.session["stream_id"].as_str().unwrap(),
            "--harness",
            "pi",
            "--session-id",
            &id,
            "--title",
            "Externally attached",
        ],
        None,
    )["session"]
        .clone();
    assert_eq!(
        child.next()["view"]["desired_pi_name"],
        "Externally attached"
    );
    child.request(2, json!({"kind":"context"}));
    assert_eq!(child.response(2)["outcome"]["result"]["session"], attached);
    assert_eq!(f.fixture.ok(&["list"], None), streams);
    assert_eq!(
        f.fixture.ok(
            &["session", "show", f.session["id"].as_str().unwrap()],
            None
        )["session"],
        f.session
    );
    assert_eq!(
        f.snapshot.lock().unwrap()["workspaces"][0]["tokens"]["strm_stream"],
        attached["stream_id"]
    );
    assert_eq!(
        f.snapshot.lock().unwrap()["panes"][0]["terminal_id"],
        "terminal"
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn automatic_cards_open_on_startup_or_outside_attach_and_not_again_on_refresh() {
    for initially_bound in [false, true] {
        let f = NativeFixture::new();
        f.fixture
            .ok(&["config", "set", "cards.auto_open", "automatic"], None);
        fs::write(
            f.fixture.directory.path().join("work-cards.json"),
            json!({"workspace_naming":false,
                "tab_naming":false,"session_naming":true,"pane_height_fraction":0.5})
            .to_string(),
        )
        .unwrap();
        let id = if initially_bound {
            f.id.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        {
            let mut snapshot = f.snapshot.lock().unwrap();
            snapshot["workspaces"][0]["tokens"] = json!({});
            for key in ["panes", "agents"] {
                snapshot[key][0]["agent_session"]["value"] = json!(id);
            }
        }
        let mut child = Transport::start(f.command(), &id);
        assert_eq!(child.next(), json!({"type":"ready","protocol":2}));
        child.request(1, json!({"kind":"session_started"}));
        let started = child.response(1);
        assert_eq!(started["outcome"]["status"], "success", "{started}");
        let attached = if initially_bound {
            assert_eq!(started["outcome"]["result"]["display"], "opened");
            f.session.clone()
        } else {
            assert_eq!(started["outcome"]["result"]["reason"], "not_enrolled");
            let stream = f.session["stream_id"].as_str().unwrap();
            f.fixture.ok(
                &[
                    "session",
                    "attach",
                    stream,
                    "--harness",
                    "pi",
                    "--session-id",
                    &uuid::Uuid::new_v4().to_string(),
                    "--title",
                    "Unrelated",
                ],
                None,
            );
            assert!(
                child
                    .frames
                    .recv_timeout(Duration::from_millis(150))
                    .is_err()
            );
            assert!(
                !f.server
                    .methods()
                    .iter()
                    .any(|method| method == "plugin.pane.open")
            );
            let attached = f.fixture.ok(
                &[
                    "session",
                    "attach",
                    stream,
                    "--harness",
                    "pi",
                    "--session-id",
                    &id,
                    "--title",
                    "Outside attach",
                ],
                None,
            )["session"]
                .clone();
            assert_eq!(child.next()["view"]["desired_pi_name"], "Outside attach");
            // FIFO barrier: the file-event pass must finish its native effects.
            child.request(2, json!({"kind":"attach"}));
            assert_eq!(child.response(2)["outcome"]["status"], "success");
            attached
        };
        let opens = f
            .server
            .requests()
            .into_iter()
            .filter(|(method, _)| method == "plugin.pane.open")
            .collect::<Vec<_>>();
        assert_eq!(opens.len(), 1, "{opens:?}");
        let params = &opens[0].1;
        assert_eq!(params["target_pane_id"], "pane");
        assert_eq!(params["placement"], "split");
        assert_eq!(params["direction"], "down");
        assert_eq!(params["focus"], false);
        let launch: streams_temper::runtime::native_cards::LaunchedCardContext =
            serde_json::from_str(params["env"]["STREAMS_CARDS_CONTEXT"].as_str().unwrap()).unwrap();
        assert_eq!(launch.scope.session_id, attached["id"]);
        assert_eq!(launch.scope.stream_id, attached["stream_id"]);
        assert_eq!(launch.owner.native_session_id, id);
        {
            let snapshot = f.snapshot.lock().unwrap();
            assert_eq!(snapshot["panes"][1]["tab_id"], "tab");
            assert_eq!(
                snapshot["panes"][1]["tokens"]["strm_session"],
                attached["id"]
            );
            assert_eq!(snapshot["panes"][1]["tokens"]["strm_role"], "card");
            assert_eq!(snapshot["panes"][0]["tokens"]["strm_status"], "active");
            assert_eq!(
                snapshot["workspaces"][0]["tokens"]["strm_stream"],
                attached["stream_id"]
            );
        }
        // A second, distinct Session write must refresh rather than open again.
        f.fixture.ok(
            &[
                "session",
                "update",
                attached["id"].as_str().unwrap(),
                "--title",
                "After attach",
            ],
            None,
        );
        assert_eq!(child.next()["view"]["desired_pi_name"], "After attach");
        child.request(3, json!({"kind":"attach"}));
        assert_eq!(child.response(3)["outcome"]["status"], "success");
        assert_eq!(
            f.server
                .methods()
                .iter()
                .filter(|method| *method == "plugin.pane.open")
                .count(),
            1
        );
        assert_eq!(child.finish(), (0, String::new()));
    }
}

#[test]
fn outside_attach_opens_after_delayed_pi_transcript_detection() {
    for (detected_at_startup, initially_bound) in [(true, false), (false, false), (false, true)] {
        let f = NativeFixture::new();
        f.fixture
            .ok(&["config", "set", "cards.auto_open", "automatic"], None);
        let id = if initially_bound {
            f.id.clone()
        } else {
            uuid::Uuid::new_v4().to_string()
        };
        let transcript = f.fixture.directory.path().join("pi-session.jsonl");
        fs::write(
            &transcript,
            json!({"type":"session","id":id}).to_string() + "\n",
        )
        .unwrap();
        let owner = {
            let mut snapshot = f.snapshot.lock().unwrap();
            for key in ["panes", "agents"] {
                snapshot[key][0]["agent_session"] =
                    json!({"agent":"pi","kind":"path","value":transcript});
            }
            let owner = snapshot["agents"][0].clone();
            if !detected_at_startup {
                snapshot["agents"] = json!([]);
            }
            owner
        };
        let mut child = Transport::start(f.command(), &id);
        assert_eq!(child.next(), json!({"type":"ready","protocol":2}));
        child.request(1, json!({"kind":"session_started"}));
        let started = child.response(1);
        if detected_at_startup {
            assert_eq!(started["outcome"]["result"]["reason"], "not_enrolled");
        } else {
            assert_eq!(started["outcome"]["status"], "error", "{started}");
            assert!(
                started["outcome"]["error"]["message"]
                    .as_str()
                    .unwrap()
                    .contains("Pi pane reports a different conversation"),
                "{started}"
            );
        }
        let directory = f.fixture.directory.path().to_str().unwrap();
        let attached = if initially_bound {
            f.session.clone()
        } else {
            f.fixture.ok(
                &[
                    "session",
                    "attach",
                    f.session["stream_id"].as_str().unwrap(),
                    "--harness",
                    "pi",
                    "--session-id",
                    &id,
                    "--title",
                    "Outside attach",
                    "--cwd",
                    directory,
                ],
                None,
            )["session"]
                .clone()
        };
        f.fixture.ok(
            &[
                "session",
                "directory",
                attached["id"].as_str().unwrap(),
                "--cwd",
                directory,
            ],
            None,
        );
        if !detected_at_startup {
            if !initially_bound {
                // The catalog event arrives while native identity is still unavailable.
                assert_eq!(child.next()["type"], "warning");
            }
            f.snapshot.lock().unwrap()["agents"] = json!([owner]);
            f.server.emit(json!({"event":"pane_agent_detected","data":{
                "type":"pane_agent_detected","pane_id":"pane","workspace_id":"workspace"
            }}));
        }
        assert_eq!(child.next()["view"]["desired_pi_name"], attached["title"]);
        // The normal refresh finishes the event pass without supplying a startup trigger.
        child.request(2, json!({"kind":"attach"}));
        assert_eq!(child.response(2)["outcome"]["status"], "success");
        let opens = usize::from(!initially_bound);
        {
            let mut snapshot = f.snapshot.lock().unwrap();
            assert_eq!(snapshot["panes"].as_array().unwrap().len(), 1 + opens);
            if !initially_bound {
                assert_eq!(
                    snapshot["panes"][1]["tokens"]["strm_session"],
                    attached["id"]
                );
                assert_eq!(snapshot["panes"][1]["tab_id"], "tab");
                // An outside close is not another automatic-opening trigger.
                snapshot["panes"].as_array_mut().unwrap().truncate(1);
                snapshot["layouts"][0]["panes"]
                    .as_array_mut()
                    .unwrap()
                    .truncate(1);
            }
        }
        f.server
            .emit(json!({"event":"pane_updated","data":{"type":"pane_updated",
            "pane":{"pane_id":"pane","tab_id":"tab","workspace_id":"workspace"}}}));
        assert_eq!(child.next()["view"]["desired_pi_name"], attached["title"]);
        child.request(3, json!({"kind":"attach"}));
        assert_eq!(child.response(3)["outcome"]["status"], "success");
        assert_eq!(
            f.snapshot.lock().unwrap()["panes"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            f.server
                .methods()
                .iter()
                .filter(|method| *method == "plugin.pane.open")
                .count(),
            opens
        );
        assert_eq!(child.finish(), (0, String::new()));
    }
}

#[test]
fn card_preferences_and_the_control_endpoint_survive_close_refresh_and_child_restart() {
    use streams_temper::{
        child_protocol::{Outcome, Request},
        runtime::native_cards::control,
    };
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    child.request(1, json!({"kind":"open_card"}));
    assert_eq!(child.response(1)["outcome"]["result"]["display"], "opened");
    let opens = f.server.requests();
    let launch: streams_temper::runtime::native_cards::LaunchedCardContext = serde_json::from_str(
        opens
            .iter()
            .find(|(method, _)| method == "plugin.pane.open")
            .unwrap()
            .1["env"]["STREAMS_CARDS_CONTEXT"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let endpoint = launch.control_socket.unwrap();
    let read = || match control::request(&endpoint, Request::CardSettings {}).unwrap() {
        Outcome::Success { result } => Value::Object(result),
        other => panic!("{other:?}"),
    };
    assert_eq!(
        read(),
        json!({"global":"manual","session":"automatic","effective":"automatic"})
    );
    control::closed(&endpoint).unwrap();
    assert_eq!(read()["session"], "manual");
    child.request(2, json!({"kind":"close_card"}));
    assert_eq!(child.response(2)["outcome"]["result"]["closed"], true);
    child.request(
        3,
        json!({"kind":"set_card_mode","target":"session","mode":"automatic"}),
    );
    assert_eq!(
        child.response(3)["outcome"]["result"]["effective"],
        "automatic"
    );
    f.fixture.ok(
        &[
            "session",
            "update",
            f.session["id"].as_str().unwrap(),
            "--title",
            "Still closed",
        ],
        None,
    );
    assert_eq!(child.next()["view"]["desired_pi_name"], "Still closed");
    child.request(4, json!({"kind":"attach"}));
    assert_eq!(child.response(4)["outcome"]["status"], "success");
    assert_eq!(
        f.snapshot.lock().unwrap()["panes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(child.finish(), (0, String::new()));
    let (mut restarted, _) = f.start();
    // A child restart is not a startup trigger, even with effective automatic mode.
    assert_eq!(
        f.snapshot.lock().unwrap()["panes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(read()["session"], "automatic");
    // An already-running card's launch endpoint still addresses the new child.
    control::closed(&endpoint).unwrap();
    restarted.request(5, json!({"kind":"session_started"}));
    assert_eq!(
        restarted.response(5)["outcome"]["result"]["display"],
        "suppressed"
    );
    restarted.request(
        6,
        json!({"kind":"set_card_mode","target":"session","mode":"inherit"}),
    );
    assert_eq!(
        restarted.response(6)["outcome"]["result"]["effective"],
        "manual"
    );
    restarted.request(
        7,
        json!({"kind":"set_card_mode","target":"global","mode":"automatic"}),
    );
    assert_eq!(
        restarted.response(7)["outcome"]["result"]["effective"],
        "automatic"
    );
    restarted.request(8, json!({"kind":"session_started"}));
    assert_eq!(
        restarted.response(8)["outcome"]["result"]["display"],
        "opened"
    );
    restarted.request(
        9,
        json!({"kind":"set_card_mode","target":"global","mode":"inherit"}),
    );
    assert_eq!(restarted.response(9)["outcome"]["status"], "error");
    // A failed preference write is a typed error on either connection.
    let config = Config::new(&launch.reader.environment, &launch.reader.cwd).unwrap();
    let records = application::open_records(&config).unwrap();
    let store = streams_temper::runtime::card_settings::CardSettings::new(
        &config,
        records.root(),
        records.host().as_str(),
        &f.id,
    )
    .unwrap();
    fs::remove_file(store.path()).unwrap();
    fs::create_dir(store.path()).unwrap();
    assert!(control::closed(&endpoint).is_err());
    restarted.request(
        10,
        json!({"kind":"set_card_mode","target":"session","mode":"manual"}),
    );
    assert_eq!(restarted.response(10)["outcome"]["status"], "error");
    assert_eq!(restarted.finish(), (0, String::new()));
}

#[test]
fn a_restarted_child_projects_saved_membership_into_an_unmarked_workspace() {
    let f = NativeFixture::new();
    f.snapshot.lock().unwrap()["workspaces"][0]["tokens"] = json!({});
    let streams = f.fixture.ok(&["list"], None);
    let (mut child, _) = f.start();
    child.request(1, json!({"kind":"context"}));
    assert_eq!(child.response(1)["outcome"]["result"]["session"], f.session);
    assert_eq!(f.fixture.ok(&["list"], None), streams);
    assert_eq!(
        f.snapshot.lock().unwrap()["workspaces"][0]["tokens"]["strm_stream"],
        f.session["stream_id"]
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn ordinary_native_refreshes_associate_an_unmarked_workspace_without_changing_membership() {
    for event in [
        "pane_updated",
        "tab_moved",
        "workspace_metadata_updated",
        "pane_moved",
    ] {
        let f = NativeFixture::new();
        let (mut child, _) = f.start();
        let streams = f.fixture.ok(&["list"], None);
        f.snapshot.lock().unwrap()["workspaces"][0]["tokens"] = json!({});
        match event {
            "pane_updated" => f.server.emit(json!({"event":event,"data":{"type":event,
                "pane":{"pane_id":"pane","tab_id":"tab","workspace_id":"workspace"}}})),
            "tab_moved" => f.server.emit(json!({"event":event,"data":{"type":event,"tab_id":"tab","workspace_id":"workspace"}})),
            "workspace_metadata_updated" => f.server.emit(json!({"event":event,"data":{"type":event,
                "workspace":{"workspace_id":"workspace"}}})),
            "pane_moved" => f.move_to("pane", "within-workspace", "next-tab"),
            _ => unreachable!(),
        }
        assert_eq!(child.next()["view"]["desired_pi_name"], "Before");
        child.request(1, json!({"kind":"context"}));
        assert_eq!(
            child.response(1)["outcome"]["result"]["session"],
            f.session,
            "{event}"
        );
        assert_eq!(f.fixture.ok(&["list"], None), streams, "{event}");
        assert_eq!(
            f.snapshot.lock().unwrap()["workspaces"][0]["tokens"]["strm_stream"],
            f.session["stream_id"]
        );
        assert_eq!(child.finish(), (0, String::new()));
    }
}

#[test]
fn a_workspace_marker_change_never_pulls_its_sessions_into_another_stream() {
    let f = NativeFixture::new();
    let destination = f.fixture.ok(&["create", "Destination"], None)["stream"].clone();
    let (mut child, _) = f.start();
    f.snapshot.lock().unwrap()["workspaces"][0]["tokens"]["strm_stream"] =
        destination["id"].clone();
    let before = f.snapshot.lock().unwrap().clone();
    f.server.emit(
        json!({"event":"workspace_metadata_updated","data":{"type":"workspace_metadata_updated",
        "workspace":{"workspace_id":"workspace"}}}),
    );
    assert_eq!(child.next()["view"]["desired_pi_name"], "Before");
    let warning = child.next();
    assert_eq!(warning["type"], "warning", "{warning}");
    child.request(1, json!({"kind":"context"}));
    assert_eq!(child.response(1)["outcome"]["result"]["session"], f.session);
    assert_eq!(*f.snapshot.lock().unwrap(), before);
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn a_pi_workspace_move_selects_the_exact_destination_and_moves_only_its_session() {
    let f = NativeFixture::new();
    let original = f.session["stream_id"].as_str().unwrap();
    let before = f.fixture.ok(&["show", original], None)["stream"].clone();
    let neighbor = f.fixture.ok(
        &[
            "session",
            "attach",
            original,
            "--harness",
            "pi",
            "--session-id",
            &uuid::Uuid::new_v4().to_string(),
            "--title",
            "Unaffected conversation",
        ],
        None,
    )["session"]
        .clone();
    let destination = f.fixture.ok(&["create", "Destination"], None)["stream"].clone();
    {
        let mut snapshot = f.snapshot.lock().unwrap();
        let mut workspace = snapshot["workspaces"][0].clone();
        workspace["workspace_id"] = json!("destination");
        workspace["tokens"]["strm_stream"] = destination["id"].clone();
        snapshot["workspaces"]
            .as_array_mut()
            .unwrap()
            .push(workspace);
    }
    let (mut child, _) = f.start();
    f.move_to_workspace("destination");
    assert_eq!(child.next()["view"]["desired_pi_name"], "Before");
    child.request(1, json!({"kind":"context"}));
    let moved = child.response(1);
    assert_eq!(
        moved["outcome"]["result"]["session"]["stream_id"],
        destination["id"]
    );
    assert_eq!(
        moved["outcome"]["result"]["session"]["previous_stream_ids"],
        json!([original])
    );
    assert_eq!(
        f.fixture
            .ok(&["session", "show", neighbor["id"].as_str().unwrap()], None)["session"],
        neighbor
    );
    assert_eq!(f.fixture.ok(&["show", original], None)["stream"], before);
    assert_eq!(
        f.snapshot.lock().unwrap()["panes"][0]["workspace_id"],
        "destination"
    );
    assert_eq!(
        f.snapshot.lock().unwrap()["panes"][0]["terminal_id"],
        "terminal"
    );
    assert!(
        !f.fixture
            .directory
            .path()
            .join("state/streams/native")
            .exists()
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn a_pi_workspace_move_into_an_unmarked_destination_creates_one_stream() {
    let f = NativeFixture::new();
    f.snapshot.lock().unwrap()["workspaces"]
        .as_array_mut()
        .unwrap()
        .push(json!({"workspace_id":"destination","label":"Unmarked","number":2,"tokens":{}}));
    let (mut child, _) = f.start();
    f.move_to_workspace("destination");
    assert_eq!(child.next()["view"]["desired_pi_name"], "Before");
    child.request(1, json!({"kind":"context"}));
    let context = child.response(1)["outcome"]["result"].clone();
    assert_eq!(context["stream"]["title"], "Untitled effort");
    assert_ne!(context["session"]["stream_id"], f.session["stream_id"]);
    assert_eq!(
        f.fixture.ok(&["list"], None)["streams"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        f.snapshot.lock().unwrap()["workspaces"][1]["tokens"]["strm_stream"],
        context["stream"]["id"]
    );
    f.server.emit(json!({"event":"pane_updated","data":{"type":"pane_updated",
        "pane":{"pane_id":"destination-pane","tab_id":"destination-tab","workspace_id":"destination"}}}));
    assert_eq!(child.next()["type"], "view");
    child.request(2, json!({"kind":"context"}));
    assert_eq!(
        child.response(2)["outcome"]["result"]["session"],
        context["session"]
    );
    assert_eq!(
        f.fixture.ok(&["list"], None)["streams"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn workspace_markers_select_the_existing_stream_for_unenrolled_lifecycle_and_enrollment() {
    for lifecycle in ["session_started", "attach"] {
        let f = NativeFixture::new();
        let id = uuid::Uuid::new_v4().to_string();
        for key in ["panes", "agents"] {
            f.snapshot.lock().unwrap()[key][0]["agent_session"]["value"] = json!(id);
        }
        let stream_id = f.session["stream_id"].as_str().unwrap();
        let before = f.fixture.ok(&["show", stream_id], None)["stream"].clone();
        let mut child = Transport::start(f.command(), &id);
        assert_eq!(child.next(), json!({"type":"ready","protocol":2}));
        child.request(1, json!({"kind":lifecycle}));
        let started = child.response(1);
        assert_eq!(started["outcome"]["status"], "success", "{started}");
        child.request(2, json!({"kind":"context"}));
        let captured = child.response(2);
        let context = &captured["outcome"]["result"];
        assert_eq!(context["stream"], before, "{captured}");
        assert!(context["session"].is_null());
        assert_eq!(context["sessions"], json!([]));
        child.request(3, json!({"kind":"enroll","request":{
            "version":2,"operation_id":"enroll-second","catalog_source":context["catalog_source"],
            "harness":"pi","harness_session_id":id,"actor":"user",
            "cwd":f.fixture.directory.path(),"stream_id":context["stream"]["id"],
            "session":{"title":"Second conversation"}
        }}));
        let enrolled = child.response(3);
        assert_eq!(enrolled["outcome"]["status"], "success", "{enrolled}");
        assert_eq!(
            enrolled["outcome"]["result"]["session"]["stream_id"],
            stream_id
        );
        child.request(4, json!({"kind":"context"}));
        assert_eq!(
            child.response(4)["outcome"]["result"]["stream"]["id"],
            stream_id
        );
        assert_eq!(f.fixture.ok(&["show", stream_id], None)["stream"], before);
        assert_eq!(child.finish(), (0, String::new()));
    }
}

#[test]
fn lifecycle_subscription_failure_is_a_domain_error_and_catalog_remains_available() {
    let f = NativeFixture::new();
    f.rejected.store(true, Ordering::Release);
    let mut child = f.begin();
    child.request(1, json!({"kind":"attach"}));
    let response = child.response(1);
    assert_eq!(response["outcome"]["status"], "error");
    assert!(
        response["outcome"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("subscription")
    );
    child.request(2, json!({"kind":"list_streams"}));
    assert_eq!(child.response(2)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["ping", "events.subscribe"]);
}

#[test]
fn resubscribe_reads_once_and_uses_private_terminal_continuity_without_startup_follow() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    f.set_location("during-gap", "tab");
    f.server.disconnect_subscriptions();
    assert_eq!(child.next()["view"]["desired_pi_name"], "Before");
    assert_eq!(f.server.methods(), ["events.subscribe", "session.snapshot"]);
    f.server.disconnect_subscriptions();
    assert_eq!(child.next()["type"], "warning");
    child.request(1, json!({"kind":"list_streams"}));
    assert_eq!(child.response(1)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["events.subscribe", "session.snapshot"]);
}

#[test]
fn failed_snapshot_after_resubscribe_warns_without_retry_and_does_not_disable_catalog() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    *f.snapshot.lock().unwrap() = json!({});
    f.server.disconnect_subscriptions();
    assert_eq!(child.next()["type"], "warning");
    child.request(1, json!({"kind":"list_streams"}));
    assert_eq!(child.response(1)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["events.subscribe", "session.snapshot"]);
}

#[test]
fn failed_resubscribe_warns_without_a_polling_read() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    f.rejected.store(true, Ordering::Release);
    f.server.disconnect_subscriptions();
    assert_eq!(child.next()["type"], "warning");
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["events.subscribe"]);
}

#[test]
fn catalog_only_ready_does_not_prepare_file_watches_or_connect_from_inherited_herdr() {
    let f = NativeFixture::new();
    let mut child = f.begin();
    assert!(
        child
            .frames
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    assert!(f.server.methods().is_empty());
    child.request(1, json!({"kind":"context"}));
    assert_eq!(child.response(1)["outcome"]["status"], "success");
    assert!(f.server.methods().is_empty());
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn running_native_cancel_stops_at_the_next_boundary_without_retrying_or_disabling_the_child() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    let gate = f.block_next_snapshot();
    child.request(1, json!({"kind":"open_card"}));
    gate.entered.recv_timeout(Duration::from_secs(15)).unwrap();
    child.send(json!({"type":"cancel","id":1}));
    // A catalog response proves the input reader consumed the cancel while native I/O was blocked.
    child.request(2, json!({"kind":"list_streams"}));
    assert_eq!(child.response(2)["outcome"]["status"], "success");
    drop(gate);
    let response = child.response(1);
    assert_eq!(response["outcome"]["status"], "error", "{response}");
    assert!(
        response["outcome"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cancel"),
        "{response}"
    );
    assert!(
        f.server
            .methods()
            .iter()
            .all(|method| method == "session.snapshot")
    );
    child.request(3, json!({"kind":"attach"}));
    assert_eq!(
        child.response(3)["outcome"]["result"],
        json!({"native":"active"})
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn accepted_save_with_native_completion_failure_retains_the_result_and_warns_once() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    child.request(2, json!({"kind":"context"}));
    let context = child.response(2);
    assert_eq!(context["outcome"]["status"], "success", "{context}");
    let context = &context["outcome"]["result"];
    let request = json!({"version":2,"operation_id":"native-warning","catalog_source":context["catalog_source"],"actor":"user",
        "kind":"session","before":context["session"],"changes":{"title":"Saved despite native failure"},
        "title_default":context["title_defaults"]["session"],"inputs":[]});
    f.fail_next_snapshot.store(true, Ordering::Release);
    child.request(1, json!({"kind":"apply","request":request}));
    let response = child.response(1);
    assert_eq!(
        response["outcome"]["status"], "saved_with_warnings",
        "{response}"
    );
    assert_eq!(
        response["outcome"]["result"]["save"]["domain"]["outcome"],
        "changes"
    );
    assert!(
        !response["outcome"]["warnings"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.fixture.ok(&["session", "list"], None)["sessions"][0]["title"],
        "Saved despite native failure"
    );
    assert!(
        child
            .frames
            .recv_timeout(Duration::from_millis(150))
            .is_err(),
        "request warning repeated as unsolicited output"
    );
    assert_eq!(child.finish(), (0, String::new()));
}
