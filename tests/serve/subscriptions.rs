use super::*;
use crate::support::herdr::{FakeHerdr, Reply};
use std::{
    collections::BTreeMap,
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
            move |method, _| match method {
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
            .env("HERDR_SOCKET_PATH", &self.server.socket)
            .env("HERDR_PANE_ID", "pane")
            .env("HERDR_TAB_ID", "tab")
            .env("HERDR_WORKSPACE_ID", "workspace");
        command
    }
    fn start(&self) -> (Transport, Value) {
        let child = Transport::start(self.command(), &self.id);
        assert_eq!(child.next(), json!({"type":"ready","protocol":1}));
        let scope = child.next();
        assert_eq!(scope["event"], "scope", "{scope}");
        assert_eq!(
            self.server.methods(),
            ["ping", "events.subscribe", "session.snapshot"]
        );
        (child, scope)
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
fn idle_move_emits_one_scope_and_irrelevant_native_events_spend_zero_observations() {
    let f = NativeFixture::new();
    let (mut child, initial) = f.start();
    assert!(!initial["value"].is_null(), "{initial}");
    f.server.clear();
    for _ in 0..32 {
        // Incomplete unused fields prove filtering happens before full event decoding.
        f.server.emit(json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"unrelated"}}}));
    }
    f.move_to("pane", "moved", "next-tab");
    let moved = child.next();
    assert_eq!(moved["event"], "scope");
    assert_eq!(moved["value"]["context"]["paneId"], "moved");
    assert_eq!(moved["value"]["context"]["terminalId"], "terminal");
    assert_eq!(f.server.methods(), ["session.snapshot"]);
    // Old IDs must stop invalidating, while the adopted new ID keeps working.
    f.server.emit(
        json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{"pane_id":"pane"}}}),
    );
    f.move_to("moved", "again", "last-tab");
    assert_eq!(child.next()["value"]["context"]["paneId"], "again");
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["session.snapshot", "session.snapshot"]);
    assert!(
        child.frames.try_recv().is_err(),
        "unchanged/irrelevant event emitted scope"
    );
}

#[test]
fn slow_unsolicited_observation_does_not_block_independent_request_dispatch() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    let gate = f.block_next_snapshot();
    f.server.emit(
        json!({"event":"pane_updated", "data":{"type":"pane_updated",
        "pane":{"pane_id":"pane","tab_id":"tab","workspace_id":"workspace"}}}),
    );
    gate.entered
        .recv_timeout(Duration::from_secs(15))
        .expect("unsolicited read entered");
    child.request(1, &["agent", "preview"], Some(Value::Null));
    child.request(2, &["list"], None);
    let mut replies = [child.next(), child.next()];
    replies.sort_by_key(|reply| reply["id"].as_u64().unwrap());
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[0]["code"], 1);
    assert_eq!(replies[1]["id"], 2);
    assert_eq!(replies[1]["code"], 0);
    // Neither response depended on releasing the native I/O gate.
    drop(gate);
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["session.snapshot"]);
}

#[test]
fn child_started_after_a_move_cannot_bootstrap_origin_from_the_pi_uuid() {
    let f = NativeFixture::new();
    f.set_location("already-moved", "next-tab");
    let (mut child, scope) = f.start();
    assert_eq!(scope["value"], Value::Null);
    assert!(
        scope["reason"].as_str().unwrap().contains("origin"),
        "{scope}"
    );
    assert_eq!(
        f.server.methods(),
        ["ping", "events.subscribe", "session.snapshot"]
    );
    f.server.clear();
    // Origin refusal survives even a later return to the stale inherited ID.
    f.set_location("pane", "tab");
    child.request(1, &["native", "observe"], None);
    assert_eq!(
        child.response(1)["value"]["error"]["kind"],
        "native-context-changed"
    );
    child.request(2, &["native", "project"], None);
    assert_eq!(
        child.response(2)["value"]["error"]["kind"],
        "native-context-changed"
    );
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), Vec::<String>::new());
}

#[test]
fn moved_id_cannot_adopt_a_reused_terminal() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    for key in ["panes", "agents"] {
        f.snapshot.lock().unwrap()[key][0]["terminal_id"] = json!("reused");
    }
    f.move_to("pane", "moved", "next-tab");
    let refused = child.next();
    assert_eq!(refused["value"], Value::Null);
    assert!(
        refused["reason"]
            .as_str()
            .unwrap()
            .contains("terminal identity")
    );
    assert_eq!(f.server.methods(), ["session.snapshot"]);
    f.server.clear();
    child.request(1, &["native", "observe"], None);
    let response = child.response(1);
    assert_eq!(response["value"]["error"]["kind"], "native-context-changed");
    assert_eq!(response["value"]["error"]["message"], refused["reason"]);
    child.request(2, &["native", "project"], None);
    assert_eq!(
        child.response(2)["value"]["error"]["kind"],
        "native-context-changed"
    );
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), Vec::<String>::new());
}

#[test]
fn unfenced_observe_and_effect_requests_cannot_bypass_the_bound_terminal() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    for key in ["panes", "agents"] {
        f.snapshot.lock().unwrap()[key][0]["terminal_id"] = json!("reused");
    }
    child.request(1, &["native", "observe"], None);
    let scope = child.next();
    assert_eq!(scope["event"], "scope");
    assert_eq!(scope["value"], Value::Null);
    assert!(
        scope["reason"]
            .as_str()
            .unwrap()
            .contains("terminal identity")
    );
    assert_eq!(
        child.response(1)["value"]["error"]["kind"],
        "native-context-changed"
    );
    child.request(2, &["native", "project"], None);
    assert_eq!(
        child.response(2)["value"]["error"]["kind"],
        "native-context-changed"
    );
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["session.snapshot"]);
}

#[test]
fn post_move_watch_parent_failure_warns_and_exits_instead_of_emitting_null_scope() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    let gate = f.block_next_snapshot();
    f.set_location("moved", "moved-tab");
    {
        let mut snapshot = f.snapshot.lock().unwrap();
        for key in ["panes", "agents", "tabs", "layouts", "workspaces"] {
            snapshot[key][0]["workspace_id"] = json!("next");
        }
    }
    f.server
        .emit(json!({"event":"pane_moved", "data":{"type":"pane_moved",
        "previous_pane_id":"pane", "previous_tab_id":"tab", "previous_workspace_id":"workspace",
        "pane":{"pane_id":"moved", "tab_id":"moved-tab", "workspace_id":"next"}}}));
    gate.entered
        .recv_timeout(Duration::from_secs(15))
        .expect("move read entered");
    let state = f.fixture.directory.path().join("state");
    fs::rename(&state, f.fixture.directory.path().join("old-state")).unwrap();
    fs::write(&state, "obstructed fixture state parent").unwrap();
    drop(gate);
    assert_eq!(child.next()["event"], "warning");
    assert_ne!(child.finish().0, 0);
    assert_eq!(f.server.methods(), ["session.snapshot"]);
}

#[test]
fn external_session_save_updates_scope_once_and_unrelated_catalog_files_do_not_observe() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    f.fixture.ok(&["create", "Unrelated"], None);
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
    let changed = child.next();
    assert_eq!(changed["event"], "scope");
    assert_eq!(changed["value"]["session"]["title"], "After");
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(
        f.server.methods(),
        ["session.snapshot"],
        "one atomic save must cause at most one observation"
    );
    assert!(child.frames.try_recv().is_err());
}

#[test]
fn external_ready_binding_releases_null_scope_without_a_request() {
    let f = NativeFixture::new();
    let mut binding: Value = serde_json::from_slice(&fs::read(&f.association).unwrap()).unwrap();
    binding["ready"] = json!(false);
    fs::write(&f.association, binding.to_string()).unwrap();
    let (mut child, unready) = f.start();
    assert!(unready["value"].is_null());
    f.server.clear();
    binding["ready"] = json!(true);
    let staging = f.association.with_extension("tmp");
    fs::write(&staging, binding.to_string()).unwrap();
    fs::rename(staging, &f.association).unwrap();
    assert_eq!(child.next()["value"]["scope"]["nativeSessionId"], f.id);
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(f.server.methods(), ["session.snapshot"]);
}

#[test]
fn required_subscription_failure_warns_and_exits_before_observing() {
    let f = NativeFixture::new();
    f.rejected.store(true, Ordering::Release);
    let mut child = Transport::start(f.command(), &f.id);
    assert_eq!(child.next(), json!({"type":"ready","protocol":1}));
    let warning = child.next();
    assert_eq!(warning["event"], "warning");
    assert!(
        warning["message"]
            .as_str()
            .unwrap()
            .contains("subscription")
    );
    assert_ne!(child.finish().0, 0);
    assert_eq!(f.server.methods(), ["ping", "events.subscribe"]);
}

#[test]
fn resubscribe_reads_once_recovers_an_idle_move_and_a_second_drop_is_fatal() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    {
        let mut snapshot = f.snapshot.lock().unwrap();
        for key in ["panes", "agents"] {
            snapshot[key][0]["pane_id"] = json!("during-gap");
        }
        snapshot["layouts"][0]["panes"][0]["pane_id"] = json!("during-gap");
    }
    f.server.disconnect_subscriptions();
    assert_eq!(child.next()["value"]["context"]["paneId"], "during-gap");
    assert_eq!(f.server.methods(), ["events.subscribe", "session.snapshot"]);
    f.server.disconnect_subscriptions();
    assert_eq!(child.next()["event"], "warning");
    assert_ne!(child.finish().0, 0);
    assert_eq!(f.server.methods(), ["events.subscribe", "session.snapshot"]);
}

#[test]
fn failed_full_read_after_resubscribe_warns_and_exits_without_retry() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    *f.snapshot.lock().unwrap() = json!({});
    f.server.disconnect_subscriptions();
    assert_eq!(child.next()["event"], "warning");
    assert_ne!(child.finish().0, 0);
    assert_eq!(f.server.methods(), ["events.subscribe", "session.snapshot"]);
}

#[test]
fn failed_resubscribe_warns_without_a_polling_read() {
    let f = NativeFixture::new();
    let (mut child, _) = f.start();
    f.server.clear();
    f.rejected.store(true, Ordering::Release);
    f.server.disconnect_subscriptions();
    assert_eq!(child.next()["event"], "warning");
    assert_ne!(child.finish().0, 0);
    assert_eq!(f.server.methods(), ["events.subscribe"]);
}

#[test]
fn file_subscription_setup_failure_warns_and_exits_outside_herdr() {
    let f = Fixture::new();
    let blocked = f.directory.path().join("not-a-directory");
    fs::write(&blocked, b"blocked").unwrap();
    f.ok(
        &["config", "set", "catalog.root", blocked.to_str().unwrap()],
        None,
    );
    let mut child = Transport::start(f.command(), "pi-session");
    assert_eq!(child.next(), json!({"type":"ready","protocol":1}));
    assert_eq!(child.next()["event"], "warning");
    assert_ne!(child.finish().0, 0);
}
