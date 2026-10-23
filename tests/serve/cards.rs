use super::*;
use crate::support::pty::Pty;
use streams_temper::{
    model::Record,
    runtime::{
        native_cards::SOURCE_ENV_KEYS,
        native_state::{NativeAttempt, NativeState, Resources},
    },
    yaml_store::YamlStore,
};

struct CardFixture {
    native: NativeFixture,
    state: NativeState,
    evidence: NativeAttempt,
    environment: BTreeMap<String, String>,
}
impl CardFixture {
    fn new() -> Self {
        let native = NativeFixture::new();
        let output = native
            .command()
            .args(["native", "check", "--session-id", &native.id, "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let status: Value = serde_json::from_slice(&output.stdout).unwrap();
        let key = status["native"]["key"].as_str().unwrap();
        let base = native.command();
        let read_environment: BTreeMap<String, String> = base
            .get_envs()
            .filter_map(|(key, value)| {
                let key = key.to_str()?;
                SOURCE_ENV_KEYS
                    .contains(&key)
                    .then(|| (key.to_owned(), value.unwrap().to_str().unwrap().to_owned()))
            })
            .collect();
        let home = native.fixture.directory.path();
        let config = Config::new(&read_environment, home).unwrap();
        let opened = application::open(&config).unwrap();
        let scope = json!({"catalogSource":opened.source_id().unwrap(), "hostId":opened.host().as_str(),
            "streamId":native.session["stream_id"], "sessionId":native.session["id"], "nativeSessionId":native.id});
        let state = NativeState::new(
            native
                .server
                .socket
                .parent()
                .unwrap()
                .join("streams-native"),
        )
        .unwrap();
        let pending = NativeAttempt::new(key, "fixture-launch").unwrap();
        let mut evidence = pending.clone();
        evidence
            .reserve(Resources::new("card", "card-terminal", "tab").unwrap())
            .unwrap();
        {
            let lock = state
                .lock(
                    &native.server.socket,
                    "workspace",
                    1.0,
                    &AtomicBool::new(false),
                )
                .unwrap();
            lock.write(None, &pending).unwrap();
            lock.write(Some(&pending), &evidence).unwrap();
        }
        {
            let mut snapshot = native.snapshot.lock().unwrap();
            snapshot["panes"].as_array_mut().unwrap().push(json!({"pane_id":"card", "terminal_id":"card-terminal",
                "workspace_id":"workspace","tab_id":"tab","cwd":home,"label":"Streams work card","tokens":{}}));
            snapshot["layouts"][0]["panes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"pane_id":"card"}));
        }
        let environment = BTreeMap::from([
            ("HERDR_PANE_ID".into(), "card".into()),
            ("TERM".into(), "xterm-256color".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("STREAMS_CARDS_SCOPE".into(), scope.to_string()),
            (
                "STREAMS_CARDS_OWNER".into(),
                json!({"pane_id":"pane","tab_id":"tab","workspace_id":"workspace"}).to_string(),
            ),
            ("STREAMS_CARDS_NONCE".into(), evidence.nonce().into()),
            ("STREAMS_CARDS_OWNER_TERMINAL".into(), "terminal".into()),
            (
                "STREAMS_CARDS_READ_ENV".into(),
                serde_json::to_string(&read_environment).unwrap(),
            ),
            (
                "STREAMS_CARDS_READ_CWD".into(),
                home.to_str().unwrap().into(),
            ),
            (
                "STREAMS_CARDS_STATE_DIR".into(),
                state.directory().to_str().unwrap().into(),
            ),
            (
                "STREAMS_CARDS_PLUGIN_ROOT".into(),
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("integrations/herdr-work-cards")
                    .to_str()
                    .unwrap()
                    .into(),
            ),
        ]);
        Self {
            native,
            state,
            evidence,
            environment,
        }
    }
    fn start(&self) -> Pty {
        let mut command = self.native.command();
        command
            .args(["native", "pane", "--session-id", &self.native.id])
            .envs(&self.environment);
        // Exactly one card process. Native reads cannot dispatch a Herdr CLI or
        // another Streams process; the fake server accepts only socket reads.
        Pty::spawn(command, false)
    }
    fn save_title(&self, title: &str) {
        let mut session: streams_temper::model::Session =
            serde_json::from_value(self.native.session.clone()).unwrap();
        session.title = title.into();
        let value = serde_json::to_value(&session).unwrap();
        let record = Record::Session(session);
        let store = YamlStore::new(self.native.fixture.root());
        let path = self.native.fixture.root().join(store.record_path(&record));
        let staging = path.with_extension("staging");
        let mut envelope: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        envelope["record"] = value;
        envelope["revision"] = json!(uuid::Uuid::new_v4().to_string());
        fs::write(&staging, serde_json::to_vec(&envelope).unwrap()).unwrap();
        fs::rename(staging, path).unwrap();
    }
    fn move_owner(&self, pane: &str) {
        let mut snapshot = self.native.snapshot.lock().unwrap();
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
    }
    fn assert_evidence_unchanged(&self) {
        assert_eq!(
            self.state.read(self.evidence.key()).unwrap(),
            Some(self.evidence.clone())
        );
    }
}
fn painted(pty: &mut Pty, title: &str) {
    pty.until(|p| p.screen().contains(title) && p.screen().contains("[q] close"));
}
#[test]
fn real_card_first_paint_and_file_update_use_one_process_three_startup_requests_and_zero_file_triggered_requests()
 {
    let f = CardFixture::new();
    f.native.server.clear();
    let mut card = f.start();
    painted(&mut card, "Before");
    assert_eq!(
        f.native.server.methods(),
        ["ping", "events.subscribe", "session.snapshot"]
    );
    f.assert_evidence_unchanged();
    f.native.server.clear();
    f.save_title("After an atomic Session save");
    painted(&mut card, "After an atomic Session save");
    assert!(f.native.server.methods().is_empty());
    card.send(b"q");
    card.finish();
    assert!(
        f.native.server.methods().is_empty(),
        "self-exit never calls Herdr"
    );
    f.assert_evidence_unchanged();
}
#[test]
fn real_card_retargets_record_watches_after_a_catalog_session_id_change() {
    let f = CardFixture::new();
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    let session: streams_temper::model::Session =
        serde_json::from_value(f.native.session.clone()).unwrap();
    let store = YamlStore::new(f.native.fixture.root());
    let old = f
        .native
        .fixture
        .root()
        .join(store.record_path(&Record::Session(session)));
    let next_id = uuid::Uuid::new_v4().to_string();
    let next = old.with_file_name(format!("{next_id}.yaml"));
    let mut envelope: Value = serde_json::from_slice(&fs::read(&old).unwrap()).unwrap();
    envelope["record"]["id"] = json!(next_id);
    envelope["record"]["title"] = json!("After catalog id change");
    envelope["revision"] = json!(uuid::Uuid::new_v4().to_string());
    fs::write(&next, serde_json::to_vec(&envelope).unwrap()).unwrap();
    fs::remove_file(old).unwrap();
    painted(&mut card, "After catalog id change");
    envelope["record"]["title"] = json!("The new id is watched");
    envelope["revision"] = json!(uuid::Uuid::new_v4().to_string());
    let staging = next.with_extension("staging");
    fs::write(&staging, serde_json::to_vec(&envelope).unwrap()).unwrap();
    fs::rename(staging, next).unwrap();
    painted(&mut card, "The new id is watched");
    card.send(b"q");
    card.finish();
    assert!(f.native.server.methods().is_empty());
    f.assert_evidence_unchanged();
}
#[test]
fn real_card_quit_cancels_initial_snapshot_without_waiting_for_server_or_native_lock() {
    let f = CardFixture::new();
    let gate = f.native.block_next_snapshot();
    let _lock = f
        .state
        .lock(
            &f.native.server.socket,
            "workspace",
            1.0,
            &AtomicBool::new(false),
        )
        .unwrap();
    f.native.server.clear();
    let mut card = f.start();
    card.until(|_| gate.entered.try_recv().is_ok());
    card.send(b"aq");
    card.finish();
    assert_eq!(
        f.native.server.methods(),
        ["ping", "events.subscribe", "session.snapshot"]
    );
    f.assert_evidence_unchanged();
    drop(gate); // Release the fake only AFTER the card has cancelled and exited.
}
#[test]
fn real_card_follows_a_moved_owner_and_keeps_reading_its_session_while_separated() {
    let f = CardFixture::new();
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    f.move_owner("moved-owner");
    f.native.server.emit(
        json!({"event":"pane_moved","data":{"type":"pane_moved","previous_pane_id":"pane",
        "previous_tab_id":"tab","previous_workspace_id":"workspace",
        "pane":{"pane_id":"moved-owner","tab_id":"moved-tab","workspace_id":"workspace","terminal_id":"terminal"}}}),
    );
    f.save_title("After owner moved");
    painted(&mut card, "After owner moved");
    assert_eq!(f.native.server.methods(), ["session.snapshot"]);
    card.send(b"q");
    card.finish();
    assert_eq!(f.native.server.methods(), ["session.snapshot"]);
    f.assert_evidence_unchanged();
}
#[test]
fn real_card_recovers_once_and_follows_ids_observed_during_the_subscription_gap() {
    let f = CardFixture::new();
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    let recovery = f.native.block_next_snapshot();
    f.native.server.disconnect_subscriptions();
    card.until(|_| recovery.entered.try_recv().is_ok());
    f.move_owner("gap-owner");
    drop(recovery);
    let followup = f.native.block_next_snapshot();
    f.native
        .server
        .emit(json!({"event":"pane_updated","data":{"type":"pane_updated",
        "pane":{"pane_id":"gap-owner","tab_id":"moved-tab","workspace_id":"workspace","terminal_id":"terminal"}}}));
    card.until(|_| followup.entered.try_recv().is_ok());
    card.send(b"q");
    card.finish();
    assert_eq!(
        f.native.server.methods(),
        ["events.subscribe", "session.snapshot", "session.snapshot"]
    );
    f.assert_evidence_unchanged();
    drop(followup);
}
#[test]
fn real_card_exits_when_its_evidence_is_removed_without_any_native_event() {
    let f = CardFixture::new();
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    fs::remove_file(f.state.path(f.evidence.key()).unwrap()).unwrap();
    card.finish();
    assert!(f.native.server.methods().is_empty());
    assert!(f.state.read(f.evidence.key()).unwrap().is_none());
    let mut orphan = f.start();
    orphan.finish();
    assert!(f.native.server.methods().is_empty());
    assert!(f.state.read(f.evidence.key()).unwrap().is_none());
}
#[test]
fn real_card_exits_on_replaced_evidence_and_does_not_touch_the_new_nonce() {
    let f = CardFixture::new();
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    let next = NativeAttempt::new(f.evidence.key(), "next-launch").unwrap();
    {
        let lock = f
            .state
            .lock(
                &f.native.server.socket,
                "workspace",
                1.0,
                &AtomicBool::new(false),
            )
            .unwrap();
        let mut ready = f.evidence.clone();
        ready.mark_ready().unwrap();
        lock.write(Some(&f.evidence), &ready).unwrap();
        lock.write(Some(&ready), &next).unwrap();
    }
    card.finish();
    assert!(f.native.server.methods().is_empty());
    assert_eq!(f.state.read(next.key()).unwrap(), Some(next));
}
#[test]
fn a_failed_first_catalog_read_does_not_pin_old_routing_or_prevent_repair() {
    let f = CardFixture::new();
    f.move_owner("already-moved");
    let session: streams_temper::model::Session =
        serde_json::from_value(f.native.session.clone()).unwrap();
    let store = YamlStore::new(f.native.fixture.root());
    let path = f
        .native
        .fixture
        .root()
        .join(store.record_path(&Record::Session(session)));
    let saved = fs::read(&path).unwrap();
    fs::write(&path, "broken: [").unwrap();
    let mut card = f.start();
    card.until(|p| p.screen().contains("Card unavailable"));
    let staging = path.with_extension("repair");
    fs::write(&staging, saved).unwrap();
    fs::rename(staging, path).unwrap();
    painted(&mut card, "Before");
    f.native.server.clear();
    f.native.server.emit(json!({"event":"pane_exited","data":{"type":"pane_exited","pane_id":"already-moved","workspace_id":"workspace"}}));
    card.finish();
    assert!(f.native.server.methods().is_empty());
    f.assert_evidence_unchanged();
}
#[test]
fn real_card_observes_owner_workspace_metadata_but_ignores_foreign_workspace_metadata() {
    let f = CardFixture::new();
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    for _ in 0..32 {
        f.native.server.emit(
            json!({"event":"workspace_metadata_updated","data":{"type":"workspace_metadata_updated",
            "workspace":{"workspace_id":"elsewhere"}}}),
        );
    }
    let gate = f.native.block_next_snapshot();
    f.native.server.emit(
        json!({"event":"workspace_metadata_updated","data":{"type":"workspace_metadata_updated",
        "workspace":{"workspace_id":"workspace"}}}),
    );
    card.until(|_| gate.entered.try_recv().is_ok());
    card.send(b"q");
    card.finish();
    assert_eq!(f.native.server.methods(), ["session.snapshot"]);
    f.assert_evidence_unchanged();
    drop(gate);
}
#[test]
fn real_card_follows_ids_discovered_at_start_and_owner_exit_cancels_a_later_read() {
    let f = CardFixture::new();
    f.move_owner("before-subscribe");
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    let gate = f.native.block_next_snapshot();
    f.native.server.emit(json!({"event":"pane_updated","data":{"type":"pane_updated",
        "pane":{"pane_id":"before-subscribe","tab_id":"moved-tab","workspace_id":"workspace","terminal_id":"terminal"}}}));
    card.until(|_| gate.entered.try_recv().is_ok());
    f.native.server.emit(json!({"event":"pane_exited","data":{"type":"pane_exited","pane_id":"before-subscribe","workspace_id":"workspace"}}));
    card.finish();
    assert_eq!(f.native.server.methods(), ["session.snapshot"]);
    f.assert_evidence_unchanged();
    drop(gate);
}
#[test]
fn real_card_explicit_refresh_is_full_even_when_evidence_notifications_are_queued() {
    let f = CardFixture::new();
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    let gate = f.native.block_next_snapshot();
    let mut ready = f.evidence.clone();
    ready.mark_ready().unwrap();
    {
        let lock = f
            .state
            .lock(
                &f.native.server.socket,
                "workspace",
                1.0,
                &AtomicBool::new(false),
            )
            .unwrap();
        lock.write(Some(&f.evidence), &ready).unwrap();
    }
    card.send(b"\x12");
    card.until(|_| gate.entered.try_recv().is_ok());
    card.send(b"q");
    card.finish();
    assert_eq!(f.native.server.methods(), ["session.snapshot"]);
    assert_eq!(f.state.read(ready.key()).unwrap(), Some(ready));
    drop(gate);
}
#[test]
fn real_card_owner_exit_cancels_the_initial_snapshot_before_the_server_replies() {
    let f = CardFixture::new();
    let gate = f.native.block_next_snapshot();
    f.native.server.clear();
    let mut card = f.start();
    card.until(|_| gate.entered.try_recv().is_ok());
    f.native.server.emit(json!({"event":"pane_exited","data":{"type":"pane_exited","pane_id":"pane","workspace_id":"workspace"}}));
    card.finish();
    assert_eq!(
        f.native.server.methods(),
        ["ping", "events.subscribe", "session.snapshot"]
    );
    f.assert_evidence_unchanged();
    drop(gate);
}
#[test]
fn real_card_owner_exit_does_no_read_or_write_and_orphaned_start_does_not_connect() {
    let mut f = CardFixture::new();
    let mut card = f.start();
    painted(&mut card, "Before");
    f.native.server.clear();
    f.native.server.emit(json!({"event":"pane_exited","data":{"type":"pane_exited","pane_id":"pane","workspace_id":"workspace"}}));
    card.finish();
    assert!(f.native.server.methods().is_empty());
    f.assert_evidence_unchanged();
    f.environment
        .insert("STREAMS_CARDS_NONCE".into(), "orphaned-launch".into());
    let mut orphan = f.start();
    orphan.finish();
    assert!(f.native.server.methods().is_empty());
    f.assert_evidence_unchanged();
}
