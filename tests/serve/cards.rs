//! Real child/TUI runs against observed native resources, not saved placements.
use super::subscriptions::NativeFixture;
use crate::support::pty::Pty;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf, process::Command, time::Duration};
use streams_temper::{
    application,
    config::Config,
    model::Record,
    runtime::native_cards::{LaunchedCardContext, PiOwner, SourceLaunch},
    storage::records::RecordAddress,
    tui::Scope,
    yaml_store::records::relative_record_path,
};

struct CardFixture {
    native: NativeFixture,
    launch: LaunchedCardContext,
}
impl CardFixture {
    fn new() -> Self {
        let native = NativeFixture::new();
        let home = native.fixture.directory.path();
        {
            let mut snapshot = native.snapshot.lock().unwrap();
            let mut card = snapshot["panes"][0].clone();
            card["pane_id"] = json!("card");
            card["terminal_id"] = json!("card-terminal");
            card["agent"] = Value::Null;
            card["agent_session"] = Value::Null;
            card["tokens"] = json!({});
            snapshot["panes"].as_array_mut().unwrap().push(card);
            snapshot["layouts"][0]["panes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"pane_id":"card"}));
        }
        let environment = BTreeMap::from([
            ("HOME".into(), home.to_str().unwrap().into()),
            (
                "XDG_CONFIG_HOME".into(),
                home.join("config").to_str().unwrap().into(),
            ),
            (
                "XDG_DATA_HOME".into(),
                home.join("data").to_str().unwrap().into(),
            ),
            (
                "XDG_STATE_HOME".into(),
                home.join("state").to_str().unwrap().into(),
            ),
            (
                "HERDR_SOCKET_PATH".into(),
                native.server.socket.to_str().unwrap().into(),
            ),
        ]);
        let opened = application::open_records(&Config::new(&environment, home).unwrap()).unwrap();
        let launch = LaunchedCardContext {
            scope: Scope {
                catalog_source: opened.source_id().into(),
                host_id: opened.host().as_str().into(),
                stream_id: native.session["stream_id"].as_str().unwrap().into(),
                session_id: native.session["id"].as_str().unwrap().into(),
                harness: "pi".into(),
                native_session_id: native.id.clone(),
            },
            owner: PiOwner {
                terminal_id: "terminal".into(),
                native_session_id: native.id.clone(),
            },
            reader: SourceLaunch {
                environment,
                cwd: home.into(),
            },
            lock_directory: home.join("card-locks"),
            plugin_root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("integrations/herdr-work-cards"),
        };
        let fixture = Self { native, launch };
        fixture.save_recap("Before");
        fixture
    }
    fn command(&self) -> Command {
        let mut command = self.native.fixture.command();
        command.args(["native", "pane"]);
        for (key, value) in &self.launch.reader.environment {
            command.env(key, value);
        }
        command
            .env("HERDR_ENV", "1")
            .env("HERDR_PANE_ID", "card")
            .env(
                "WORK_CARDS_CONFIG_PATH",
                self.native.fixture.directory.path().join("work-cards.json"),
            )
            .env(
                "STREAMS_CARDS_CONTEXT",
                serde_json::to_string(&self.launch).unwrap(),
            );
        command
    }
    fn spawn(&self) -> CardProcess {
        let paths = self
            .native
            .fixture
            .directory
            .path()
            .join(format!("card-process-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&paths).unwrap();
        let native = self.command();
        let mut helper = Command::new(std::env::current_exe().unwrap());
        helper.env_clear();
        for (key, value) in native.get_envs() {
            if let Some(value) = value {
                helper.env(key, value);
            }
        }
        helper
            .current_dir(self.native.fixture.directory.path())
            .args([
                "--exact",
                "serve::subscriptions::cards::real_card_process",
                "--nocapture",
            ])
            .env("STREAMS_TEST_CARD_PROCESS", &paths)
            .env("STREAMS_TEST_CARD_BINARY", native.get_program());
        CardProcess {
            pty: Pty::spawn(helper, false),
            paths,
        }
    }
    fn save_recap(&self, recap: &str) {
        let mut session: streams_temper::model::Session =
            serde_json::from_value(self.native.session.clone()).unwrap();
        session.recap = recap.into();
        let value = serde_json::to_value(&session).unwrap();
        let record = Record::Session(session);
        let path = self
            .native
            .fixture
            .root()
            .join(relative_record_path(&RecordAddress::for_record(&record)).unwrap());
        let staging = path.with_extension("staging");
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope["record"] = value;
        envelope["revision"] = json!(uuid::Uuid::new_v4().to_string());
        fs::write(&staging, serde_json::to_vec(&envelope).unwrap()).unwrap();
        fs::rename(staging, path).unwrap();
    }
    fn move_owner(&self, pane: &str) {
        let mut snapshot = self.native.snapshot.lock().unwrap();
        let previous = snapshot["panes"][0]["pane_id"].clone();
        for key in ["panes", "agents"] {
            snapshot[key][0]["pane_id"] = json!(pane);
            snapshot[key][0]["tab_id"] = json!("moved-tab");
        }
        snapshot["tabs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"workspace_id":"workspace","tab_id":"moved-tab","label":"2","number":2}));
        snapshot["layouts"][0]["panes"] = json!([{"pane_id":"card"}]);
        snapshot["layouts"].as_array_mut().unwrap().push(
            json!({"workspace_id":"workspace","tab_id":"moved-tab","panes":[{"pane_id":pane}]}),
        );
        drop(snapshot);
        self.native.server.emit(json!({"event":"pane_moved","data":{"type":"pane_moved",
            "previous_pane_id":previous,"previous_tab_id":"tab","previous_workspace_id":"workspace",
            "pane":{"pane_id":pane,"terminal_id":"terminal","tab_id":"moved-tab","workspace_id":"workspace"}}}));
    }

    fn assert_only_empty_locks(&self) {
        assert!(
            !self
                .native
                .fixture
                .directory
                .path()
                .join("state/streams/native")
                .exists()
        );
        for entry in fs::read_dir(&self.launch.lock_directory).unwrap() {
            let entry = entry.unwrap();
            assert!(entry.file_type().unwrap().is_file());
            assert!(fs::read(entry.path()).unwrap().is_empty());
        }
    }
}
struct CardProcess {
    pty: Pty,
    paths: PathBuf,
}
impl std::ops::Deref for CardProcess {
    type Target = Pty;
    fn deref(&self) -> &Pty {
        &self.pty
    }
}
impl std::ops::DerefMut for CardProcess {
    fn deref_mut(&mut self) -> &mut Pty {
        &mut self.pty
    }
}
impl CardProcess {
    fn finish(&mut self, used_tui: bool) {
        self.pty.until(|_| self.paths.join("status").exists());
        assert_eq!(
            fs::read_to_string(self.paths.join("status")).unwrap(),
            "0",
            "{}",
            self.pty.screen()
        );
        if used_tui {
            self.pty.assert_cleanup();
        }
        fs::write(self.paths.join("release"), b"").unwrap();
        self.pty.finish();
    }
}

#[test]
fn real_card_process() {
    let Some(paths) = std::env::var_os("STREAMS_TEST_CARD_PROCESS") else {
        return;
    };
    let paths = PathBuf::from(paths);
    let status = Command::new(std::env::var_os("STREAMS_TEST_CARD_BINARY").unwrap())
        .args(["native", "pane"])
        .status()
        .unwrap();
    // Keep the controlling session alive while the parent checks restored termios.
    fs::write(
        paths.join("status.tmp"),
        status.code().unwrap_or(-1).to_string(),
    )
    .unwrap();
    fs::rename(paths.join("status.tmp"), paths.join("status")).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !paths.join("release").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "card parent did not release the helper"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(status.success());
}

fn painted(pty: &mut Pty, title: &str) {
    pty.until(|p| {
        let text = p.screen();
        text.contains("[q] close") && text.contains(title)
    });
}

#[test]
fn real_card_observes_its_exact_session_after_a_pi_move_and_never_reopens() {
    let f = CardFixture::new();
    let mut pty = f.spawn();
    painted(&mut pty, "Before");
    f.move_owner("owner-moved");
    f.save_recap("After external save");
    painted(&mut pty, "After external save");
    pty.send(b"q");
    pty.finish(true);
    let snapshot = f.native.snapshot.lock().unwrap();
    let pi = snapshot["panes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["terminal_id"] == "terminal")
        .unwrap();
    assert_eq!(pi["pane_id"], "owner-moved");
    assert_eq!(snapshot["panes"].as_array().unwrap().len(), 2);
    assert!(
        !f.native
            .server
            .methods()
            .iter()
            .any(|m| m == "plugin.open" || m == "pane.close")
    );
    drop(snapshot);
    f.assert_only_empty_locks();
}

#[test]
fn a_real_card_recovers_routing_from_a_new_observation_without_an_owner_baseline() {
    let f = CardFixture::new();
    let mut pty = f.spawn();
    painted(&mut pty, "Before");
    let gate = f.native.block_next_snapshot();
    f.native.server.disconnect_subscriptions();
    gate.entered.recv_timeout(Duration::from_secs(3)).unwrap();
    f.move_owner("owner-moved-during-gap");
    drop(gate);
    f.save_recap("After recovered observation");
    painted(&mut pty, "After recovered observation");
    for key in ["panes", "agents"] {
        f.native.snapshot.lock().unwrap()[key][0]["agent"] = "other".into();
    }
    f.native.server.emit(json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{
        "pane_id":"owner-moved-during-gap","tab_id":"moved-tab","workspace_id":"workspace","terminal_id":"terminal"}}}));
    pty.finish(true);
    f.assert_only_empty_locks();
}

#[test]
fn failed_move_refresh_is_reobserved_before_file_refresh_and_replacement() {
    let f = CardFixture::new();
    let mut pty = f.spawn();
    painted(&mut pty, "Before");
    f.native
        .fail_next_snapshot
        .store(true, std::sync::atomic::Ordering::Release);
    f.move_owner("owner-moved-after-failure");
    pty.until(|p| p.screen().contains("Card unavailable"));
    f.native.server.clear();
    let before = f.native.snapshot.lock().unwrap().clone();
    f.save_recap("Recovered through file event");
    painted(&mut pty, "Recovered through file event");
    assert_eq!(
        f.native
            .server
            .methods()
            .iter()
            .filter(|m| *m == "session.snapshot")
            .count(),
        1
    );
    assert_eq!(*f.native.snapshot.lock().unwrap(), before);
    for key in ["panes", "agents"] {
        f.native.snapshot.lock().unwrap()[key][0]["agent"] = json!("other");
    }
    f.native.server.emit(
        json!({"event":"pane_updated","data":{"type":"pane_updated","pane":{
        "pane_id":"owner-moved-after-failure","tab_id":"moved-tab","workspace_id":"workspace"}}}),
    );
    pty.finish(true);
    let snapshot = f.native.snapshot.lock().unwrap();
    assert_eq!(snapshot["panes"].as_array().unwrap().len(), 2);
    assert_eq!(snapshot["panes"][0]["agent"], "other");
    assert_eq!(snapshot["panes"][1]["terminal_id"], "card-terminal");
    assert!(
        !f.native
            .server
            .methods()
            .iter()
            .any(|m| m == "plugin.open" || m == "pane.close")
    );
    drop(snapshot);
    f.assert_only_empty_locks();
}

#[test]
fn quit_cancels_an_unavailable_initial_snapshot_and_leaves_resources_untouched() {
    let f = CardFixture::new();
    let before = f.native.snapshot.lock().unwrap().clone();
    let gate = f.native.block_next_snapshot();
    let mut pty = f.spawn();
    gate.entered.recv_timeout(Duration::from_secs(3)).unwrap();
    pty.send(b"q");
    pty.finish(true);
    drop(gate);
    assert_eq!(*f.native.snapshot.lock().unwrap(), before);
    f.assert_only_empty_locks();
}

#[test]
fn a_real_card_closes_only_after_its_pi_is_observed_absent() {
    let f = CardFixture::new();
    let mut pty = f.spawn();
    painted(&mut pty, "Before");
    {
        let mut snapshot = f.native.snapshot.lock().unwrap();
        snapshot["agents"] = json!([]);
        snapshot["panes"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["terminal_id"] != "terminal");
        snapshot["layouts"][0]["panes"] = json!([{"pane_id":"card"}]);
    }
    f.native.server.emit(json!({"event":"pane_exited","data":{"type":"pane_exited","pane_id":"pane","workspace_id":"workspace"}}));
    pty.finish(true);
    let snapshot = f.native.snapshot.lock().unwrap();
    assert_eq!(snapshot["panes"].as_array().unwrap().len(), 1);
    assert_eq!(snapshot["panes"][0]["terminal_id"], "card-terminal");
    assert!(
        !f.native
            .server
            .methods()
            .iter()
            .any(|m| m == "plugin.open" || m == "pane.close")
    );
    drop(snapshot);
    f.assert_only_empty_locks();
}

#[test]
fn a_second_real_card_exits_before_host_io_and_leaves_the_active_card_alone() {
    let f = CardFixture::new();
    let mut active = f.spawn();
    painted(&mut active, "Before");
    f.native.server.clear();
    let before = f.native.snapshot.lock().unwrap().clone();
    let mut duplicate = f.spawn();
    duplicate.finish(false);
    assert!(f.native.server.methods().is_empty());
    assert_eq!(*f.native.snapshot.lock().unwrap(), before);
    f.save_recap("Active card kept running");
    painted(&mut active, "Active card kept running");
    active.send(b"q");
    active.finish(true);
    f.assert_only_empty_locks();
}

#[test]
fn a_real_card_can_refresh_after_its_initial_session_read_failed() {
    let f = CardFixture::new();
    let path = f
        .native
        .fixture
        .root()
        .join("hosts")
        .join(&f.launch.scope.host_id)
        .join("conversations")
        .join(format!("{}.yaml", f.native.session["id"].as_str().unwrap()));
    let original = fs::read(&path).unwrap();
    fs::write(&path, b"not a record").unwrap();
    let mut pty = f.spawn();
    pty.until(|p| p.screen().contains("Card unavailable"));
    f.move_owner("owner-moved-before-initial-card");
    let staging = path.with_extension("tmp");
    fs::write(&staging, original).unwrap();
    fs::rename(staging, path).unwrap();
    painted(&mut pty, "Before");
    f.save_recap("Recovered after initial failure");
    painted(&mut pty, "Recovered after initial failure");
    pty.send(b"q");
    pty.finish(true);
    f.assert_only_empty_locks();
}

#[test]
fn a_real_card_refuses_a_rekey_instead_of_discovering_a_neighbor_session() {
    let f = CardFixture::new();
    let mut pty = f.spawn();
    painted(&mut pty, "Before");
    let old = f.native.session["id"].as_str().unwrap();
    let directory = f
        .native
        .fixture
        .root()
        .join("hosts")
        .join(&f.launch.scope.host_id)
        .join("conversations");
    let mut value: Value =
        serde_json::from_slice(&fs::read(directory.join(format!("{old}.yaml"))).unwrap()).unwrap();
    let new = "22222222-2222-4222-8222-222222222222";
    value["record"]["id"] = new.into();
    value["record"]["title"] = "Must not be discovered".into();
    fs::write(directory.join(format!("{new}.yaml")), value.to_string()).unwrap();
    fs::remove_file(directory.join(format!("{old}.yaml"))).unwrap();
    pty.until(|p| p.screen().contains("Card unavailable"));
    assert!(!pty.screen().contains("Must not be discovered"));
    pty.send(b"q");
    pty.finish(true);
    f.assert_only_empty_locks();
}
