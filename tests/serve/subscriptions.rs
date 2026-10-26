use super::*;
use crate::support::herdr::{FakeHerdr, Reply};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use streams_temper::{
    application,
    config::Config,
    runtime::state::{RuntimeState, herdr_scope},
};

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
    snapshot_gate: Arc<Mutex<Option<PendingSnapshot>>>,
    session: Value,
    id: String,
    association: PathBuf,
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
        let snapshot_gate = Arc::new(Mutex::new(None::<PendingSnapshot>));
        let server = FakeHerdr::start(&socket, {
            let snapshot = snapshot.clone();
            let rejected = rejected.clone();
            let snapshot_gate = snapshot_gate.clone();
            move |method, params| match method {
                "events.subscribe" if rejected.load(Ordering::Acquire) => {
                    Reply::Error(json!({"code":"denied","message":"fixture subscription denied"}))
                }
                "events.subscribe" => Reply::Result(json!({"type":"subscription_started"})),
                "session.snapshot" => {
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
        let opened = application::open(&config).unwrap();
        let state = RuntimeState::new(
            config.runtime_state_dir_for(opened.root()).unwrap(),
            opened.host().as_str(),
        )
        .unwrap();
        let [association, _] = state
            .workspace_binding_paths(&herdr_scope(&socket, None), "workspace")
            .unwrap();
        fs::create_dir_all(association.parent().unwrap()).unwrap();
        fs::write(
            &association,
            json!({"version":1,"workspace_id":"workspace","stream_id":stream["id"],
            "session_id":null,"command":null,"ready":true})
            .to_string(),
        )
        .unwrap();
        fixture.ok(&["config", "set", "frontend.mode", "herdr"], None);
        let native = home.join("state/streams/native");
        fs::create_dir_all(&native).unwrap();
        fs::set_permissions(&native, fs::Permissions::from_mode(0o700)).unwrap();
        fs::write(home.join("work-cards.json"), json!({
            "workspace_startup":"manual","workspace_naming":false,"tab_naming":false,"session_naming":true
        }).to_string()).unwrap();
        Self {
            server,
            fixture,
            snapshot,
            rejected,
            snapshot_gate,
            session,
            id,
            association,
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
        warning["message"].as_str().unwrap().contains("session"),
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
fn restarted_child_cannot_bootstrap_moved_origin_or_recover_by_returning_to_stale_id() {
    let f = NativeFixture::new();
    f.set_location("already-moved", "next-tab");
    let mut child = f.begin();
    child.request(1, json!({"kind":"attach"}));
    let refused = child.response(1);
    assert_eq!(refused["outcome"]["status"], "error");
    assert!(
        refused["outcome"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("origin")
    );
    f.server.clear();
    f.set_location("pane", "tab");
    child.request(2, json!({"kind":"open_card"}));
    assert_eq!(
        child.response(2)["outcome"]["error"]["kind"],
        "native-context-changed"
    );
    child.request(3, json!({"kind":"list_streams"}));
    assert_eq!(child.response(3)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
    assert!(f.server.methods().is_empty());
}

#[test]
fn moved_id_cannot_adopt_a_reused_terminal_or_bypass_its_refusal() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    for key in ["panes", "agents"] {
        f.snapshot.lock().unwrap()[key][0]["terminal_id"] = json!("reused");
    }
    f.move_to("pane", "moved", "next-tab");
    let warning = child.next();
    assert_eq!(warning["type"], "warning");
    assert!(
        warning["message"]
            .as_str()
            .unwrap()
            .contains("terminal identity")
    );
    assert_eq!(f.server.methods(), ["session.snapshot"]);
    f.server.clear();
    child.request(1, json!({"kind":"open_card"}));
    assert_eq!(
        child.response(1)["outcome"]["error"]["kind"],
        "native-context-changed"
    );
    child.request(2, json!({"kind":"close_card"}));
    assert_eq!(
        child.response(2)["outcome"]["error"]["kind"],
        "native-context-changed"
    );
    assert_eq!(child.finish(), (0, String::new()));
    assert!(f.server.methods().is_empty());
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
fn a_catalog_association_error_is_not_inactive_and_ready_change_can_select_a_new_read() {
    let f = NativeFixture::new();
    let mut binding: Value = serde_json::from_slice(&fs::read(&f.association).unwrap()).unwrap();
    binding["ready"] = json!(false);
    fs::write(&f.association, binding.to_string()).unwrap();
    let mut child = f.begin();
    child.request(1, json!({"kind":"attach"}));
    assert_eq!(child.response(1)["outcome"]["status"], "error");
    binding["ready"] = json!(true);
    let staging = f.association.with_extension("tmp");
    fs::write(&staging, binding.to_string()).unwrap();
    fs::rename(staging, &f.association).unwrap();
    assert_eq!(child.next()["view"]["desired_pi_name"], "Before");
    assert_eq!(child.finish(), (0, String::new()));
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
    let request = json!({"operation_id":"native-warning","catalog_source":context["catalog_source"],"actor":"user",
        "kind":"session","before":context["session"],"changes":{"title":"Saved despite native failure"},
        "ownership_token":context["authority"]["token"],"inputs":[]});
    fs::set_permissions(
        f.fixture.directory.path().join("state/streams/native"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
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
