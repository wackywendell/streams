mod artifacts;

use super::*;
use crate::{
    fake_herdr::{FakeHerdr, Reply},
    model::{Session, Stream},
    tui::DisplayContext,
};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::Mutex,
};

const NATIVE: &str = "11234567-89ab-4def-8123-456789abcdef";
fn card(owner: &HerdrInventoryPane) -> CardSnapshot {
    let stream = Stream::new("effort", "Effort", None, "host").unwrap();
    let session = Session::new(
        Session::id_for("host", "pi", NATIVE),
        "host",
        "effort",
        "pi",
        NATIVE,
        "Conversation",
    )
    .unwrap();
    CardSnapshot::new(
        stream,
        session,
        DisplayContext {
            catalog_source: "catalog".into(),
            owner: owner::location(owner),
            checkouts: vec![],
            documents: vec![],
        },
    )
    .unwrap()
}
fn pane(id: &str, terminal: &str, workspace: &str, tab: &str, resource: &str) -> Value {
    json!({"pane_id":id,"terminal_id":terminal,"workspace_id":workspace,"tab_id":tab,
        "resource":resource,"cwd":"/","label":"Effort","tokens":{}})
}
struct World {
    snapshot: Value,
    changes: Vec<String>,
    created: usize,
    lost_open_reply: bool,
    reads: usize,
    display_reads: usize,
}
impl World {
    fn new() -> Self {
        let mut owner = pane("owner", "pi-terminal", "w", "t", "Pi process");
        owner["agent"] = json!("pi");
        owner["agent_session"] = json!({"agent":"pi","kind":"id","value":NATIVE});
        let parsed = HerdrInventoryPane::parse(&owner).unwrap();
        owner["tokens"] = json!({"strm_status":card(&parsed).session().status.to_string()});
        let mut display = pane("card", "card-terminal", "w", "t", "original card");
        display["tokens"] = json!(card_tokens(card(&parsed).scope()));
        display["label"] = json!("Effort: Conversation");
        Self {
            snapshot: json!({
            "workspaces":[{"workspace_id":"w","number":1,"label":"Effort","resource":"source workspace",
                "tokens":stream_tokens("catalog","host","effort")},
                {"workspace_id":"other","number":2,"label":"Effort","resource":"destination workspace",
                "tokens":stream_tokens("catalog","host","effort")}],
            "tabs":[{"tab_id":"t","workspace_id":"w","number":1,"label":"Conversation","resource":"source tab"},
                {"tab_id":"other-tab","workspace_id":"other","number":1,"label":"User tab","resource":"destination tab"}],
            "panes":[owner,display,pane("neighbor","neighbor-terminal","other","other-tab","unrelated terminal")],
            "agents":[],"layouts":[] }),
            changes: vec![],
            created: 0,
            lost_open_reply: false,
            reads: 0,
            display_reads: 0,
        }
    }
    fn refresh(&mut self) {
        self.snapshot["agents"] = json!(
            self.snapshot["panes"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|pane| pane.get("agent_session").is_some())
                .cloned()
                .collect::<Vec<_>>()
        );
        self.snapshot["layouts"] = json!(
            self.snapshot["tabs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tab| {
                    let panes: Vec<_> = self.snapshot["panes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter(|pane| pane["tab_id"] == tab["tab_id"])
                        .map(|pane| json!({"pane_id":pane["pane_id"]}))
                        .collect();
                    json!({"workspace_id":tab["workspace_id"],"tab_id":tab["tab_id"],"panes":panes})
                })
                .collect::<Vec<_>>()
        );
    }
    fn observe(&mut self) -> HerdrNativeSnapshot {
        self.refresh();
        HerdrNativeSnapshot::parse(&self.snapshot).unwrap()
    }
    fn row(&mut self, kind: &str, id: &str) -> &mut Value {
        let collection = format!("{kind}s");
        let key = format!("{kind}_id");
        self.snapshot[collection]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|row| row[&key] == id)
            .unwrap()
    }
    fn relocate(&mut self, id: &str, new_id: &str, workspace: &str, tab: &str) {
        let index = self.snapshot["panes"]
            .as_array()
            .unwrap()
            .iter()
            .position(|pane| pane["pane_id"] == id)
            .unwrap();
        let mut row = self.snapshot["panes"].as_array_mut().unwrap().remove(index);
        row["pane_id"] = json!(new_id);
        row["workspace_id"] = json!(workspace);
        row["tab_id"] = json!(tab);
        self.snapshot["panes"].as_array_mut().unwrap().push(row);
        self.refresh();
    }
    fn remove(&mut self, id: &str) {
        self.snapshot["panes"]
            .as_array_mut()
            .unwrap()
            .retain(|pane| pane["pane_id"] != id);
        self.refresh();
    }
    fn reply(&mut self, method: &str, params: &Value, plugin: &Path) -> Reply {
        match method {
            "events.subscribe" => Reply::Result(json!({"type":"subscription_started"})),
            "plugin.list" => Reply::Result(
                json!({"type":"plugin_list","plugins":[{"plugin_id":PLUGIN_ID,
                "enabled":true,"plugin_root":plugin,"manifest_path":plugin.join("streams-rust-herdr-plugin.toml")}]}),
            ),
            "workspace.report_metadata" | "pane.report_metadata" => {
                let kind = method.split('.').next().unwrap();
                let id = params[format!("{kind}_id")].as_str().unwrap();
                let row = self.row(kind, id);
                let resource = row["resource"].as_str().unwrap().to_owned();
                for (key, value) in params["tokens"].as_object().unwrap() {
                    if value.is_null() {
                        row["tokens"].as_object_mut().unwrap().remove(key);
                    } else {
                        row["tokens"][key] = value.clone();
                    }
                }
                self.changes.push(resource);
                Reply::Result(json!({"type":"ok"}))
            }
            "workspace.rename" | "tab.rename" | "pane.rename" => {
                let kind = method.split('.').next().unwrap();
                let id = params[format!("{kind}_id")].as_str().unwrap();
                let row = self.row(kind, id);
                row["label"] = params["label"].clone();
                let resource = row["resource"].as_str().unwrap().to_owned();
                let row = row.clone();
                self.changes.push(resource);
                Reply::Result(json!({"type":format!("{kind}_info"),kind:row}))
            }
            "pane.move" => {
                let id = params["pane_id"].as_str().unwrap();
                let destination = &params["destination"];
                assert_eq!(destination["split"], "down");
                let target = self
                    .row("pane", destination["target_pane_id"].as_str().unwrap())
                    .clone();
                assert_eq!(destination["tab_id"], target["tab_id"]);
                let resource = self.row("pane", id)["resource"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                self.relocate(
                    id,
                    "moved-card",
                    target["workspace_id"].as_str().unwrap(),
                    target["tab_id"].as_str().unwrap(),
                );
                self.changes.push(resource);
                let row = self.row("pane", "moved-card").clone();
                Reply::Result(json!({"type":"pane_move","move_result":{"changed":true,"pane":row}}))
            }
            "plugin.pane.open" => {
                assert_eq!(params["entrypoint"], "card");
                let owner = self
                    .row("pane", params["target_pane_id"].as_str().unwrap())
                    .clone();
                self.created += 1;
                let id = format!("opened-{}", self.created);
                let terminal = format!("opened-terminal-{}", self.created);
                let row = pane(
                    &id,
                    &terminal,
                    owner["workspace_id"].as_str().unwrap(),
                    owner["tab_id"].as_str().unwrap(),
                    "new card",
                );
                self.snapshot["panes"]
                    .as_array_mut()
                    .unwrap()
                    .push(row.clone());
                self.refresh();
                self.changes.push("new card".into());
                if self.lost_open_reply {
                    Reply::Raw(vec![])
                } else {
                    Reply::Result(
                        json!({"type":"plugin_pane_opened","plugin_pane":{"plugin_id":PLUGIN_ID,"entrypoint":"card","pane":row}}),
                    )
                }
            }
            "plugin.pane.close" => {
                let id = params["pane_id"].as_str().unwrap();
                let resource = self.row("pane", id)["resource"]
                    .as_str()
                    .unwrap()
                    .to_owned();
                self.remove(id);
                self.changes.push(resource);
                Reply::Result(json!({"type":"plugin_pane_closed","pane_id":id}))
            }
            _ => panic!("unexpected native method {method}: {params}"),
        }
    }
}
struct Source {
    world: Arc<Mutex<World>>,
    cwd: PathBuf,
}
impl NativeCardSource for Source {
    fn read_records(&mut self, owner: &HerdrInventoryPane) -> Result<CardRecords> {
        self.world.lock().unwrap().reads += 1;
        Ok(card(owner).into_records())
    }
    fn read(&mut self, owner: &HerdrInventoryPane) -> Result<CardSnapshot> {
        self.world.lock().unwrap().display_reads += 1;
        Ok(card(owner))
    }
    fn resolve_document(&mut self, _: &DocumentTarget) -> Result<ResolvedEditor> {
        Err(invalid("no document"))
    }
    fn launch_context(&self) -> SourceLaunch {
        SourceLaunch {
            environment: BTreeMap::new(),
            cwd: self.cwd.clone(),
        }
    }
}
struct Fixture {
    server: FakeHerdr,
    world: Arc<Mutex<World>>,
    directory: tempfile::TempDir,
    client: HerdrClient,
    config: NativeCardsConfig,
    environment: BTreeMap<String, String>,
    owner: PiOwner,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("direct-card-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = directory.path().canonicalize().unwrap();
        let world = Arc::new(Mutex::new(World::new()));
        let shared = world.clone();
        let plugin = root.clone();
        let server = FakeHerdr::start(&root.join("herdr.sock"), move |method, params| {
            shared.lock().unwrap().reply(method, params, &plugin)
        });
        let environment = BTreeMap::from([
            ("HOME".into(), root.to_str().unwrap().into()),
            (
                "HERDR_SOCKET_PATH".into(),
                server.socket.to_str().unwrap().into(),
            ),
            ("HERDR_PANE_ID".into(), "owner".into()),
            (
                "STREAMS_CARDS_LOCK_DIR".into(),
                root.join("locks").to_str().unwrap().into(),
            ),
        ]);
        std::fs::write(
            root.join("work-cards.json"),
            r#"{"pane_height_fraction":0.5}"#,
        )
        .unwrap();
        let mut environment = environment;
        environment.insert(
            "WORK_CARDS_CONFIG_PATH".into(),
            root.join("work-cards.json").to_str().unwrap().into(),
        );
        let config = NativeCardsConfig::load(&environment, &root, &root).unwrap();
        let client = HerdrClient::connect(environment.clone(), 1.0).unwrap();
        let owner =
            PiOwner::from_snapshot(&world.lock().unwrap().observe(), "owner", NATIVE).unwrap();
        Self {
            server,
            world,
            directory,
            client,
            config,
            environment,
            owner,
        }
    }
    fn engine(&self) -> Result<NativeCardEngine> {
        self.engine_with(self.client.clone())
    }
    fn engine_with(&self, client: HerdrClient) -> Result<NativeCardEngine> {
        let snapshot = self.world.lock().unwrap().observe();
        NativeCardEngine::new(
            client,
            Box::new(Source {
                world: self.world.clone(),
                cwd: self.directory.path().canonicalize().unwrap(),
            }),
            snapshot,
            self.owner.clone(),
            self.config.clone(),
            self.environment.clone(),
            Arc::new(AtomicBool::new(false)),
        )
    }
}
#[test]
fn new_cards_get_the_combined_caption_without_a_later_projection() {
    for mode in [OpenMode::Automatic, OpenMode::Explicit] {
        let f = Fixture::new();
        f.world.lock().unwrap().remove("card");
        let mut engine = f.engine().unwrap();
        engine.project().unwrap();
        assert_eq!(engine.open_display(mode).unwrap(), DisplayOutcome::Opened);
        {
            let mut world = f.world.lock().unwrap();
            assert_eq!(
                world.row("pane", "opened-1")["label"],
                "Effort: Conversation"
            );
            assert_eq!(world.created, 1);
        }
        f.server.clear();
        assert_eq!(engine.open_display(mode).unwrap(), DisplayOutcome::Existing);
        assert!(f.server.methods().is_empty());

        let snapshot = f.world.lock().unwrap().observe();
        publish_card_marker(&f.client, &snapshot, "opened-1", engine.card.scope()).unwrap();
        f.server.clear();
        f.engine().unwrap().project().unwrap();
        assert!(f.server.methods().is_empty());
        assert_eq!(f.world.lock().unwrap().created, 1);
    }
}

#[test]
fn an_invalid_caption_is_rejected_before_opening_a_card() {
    let f = Fixture::new();
    f.world.lock().unwrap().remove("card");
    let mut engine = f.engine().unwrap();
    engine.config.tab_naming = false;
    let mut session = engine.card.session().clone();
    session.title = "Hidden\u{200b}format".into();
    engine.card = CardRecords::new(
        engine.card.stream().clone(),
        session,
        "catalog".into(),
        engine.card.owner().clone(),
    )
    .unwrap();
    engine.project().unwrap();
    f.server.clear();
    assert!(engine.open_display(OpenMode::Automatic).is_err());
    assert!(f.server.methods().is_empty());
    assert_eq!(f.world.lock().unwrap().created, 0);
}

#[test]
fn projection_owns_combined_caption_and_all_three_pi_short_forms() {
    let f = Fixture::new();
    f.world.lock().unwrap().row("pane", "card")["label"] = json!("Old caption");
    let mut engine = f.engine().unwrap();
    let set_short = |engine: &mut NativeCardEngine, recap, state, remaining| {
        let mut session = engine.card.session().clone();
        session.recap_short = recap;
        session.state_short = state;
        session.remaining_short = remaining;
        engine.card = CardRecords::new(
            engine.card.stream().clone(),
            session,
            "catalog".into(),
            engine.card.owner().clone(),
        )
        .unwrap();
    };
    set_short(
        &mut engine,
        Some("Card text".into()),
        Some("Review".into()),
        Some("Land".into()),
    );
    engine.project().unwrap();
    {
        let mut world = f.world.lock().unwrap();
        assert_eq!(world.row("pane", "card")["label"], "Effort: Conversation");
        let tokens = &world.row("pane", "owner")["tokens"];
        assert_eq!(tokens["strm_recap_short"], "Card text");
        assert_eq!(tokens["strm_state_short"], "Review");
        assert_eq!(tokens["strm_remaining_short"], "Land");
        assert_eq!(world.row("workspace", "w")["label"], "Effort");
        assert_eq!(world.row("tab", "t")["label"], "Conversation");
    }
    let mut engine = f.engine().unwrap();
    set_short(
        &mut engine,
        Some("x".repeat(25)),
        Some("x".repeat(24)),
        None,
    );
    engine.project().unwrap();
    let mut world = f.world.lock().unwrap();
    let tokens = &world.row("pane", "owner")["tokens"];
    assert!(tokens.get("strm_recap_short").is_none());
    assert_eq!(tokens["strm_state_short"], "x".repeat(24));
    assert!(tokens.get("strm_remaining_short").is_none());
}

#[test]
fn opening_a_document_rereads_display_inputs_and_refuses_a_removed_reference() {
    let f = Fixture::new();
    let mut engine = f.engine().unwrap();
    let target = DocumentTarget {
        scope: engine.card.scope().clone(),
        owner: engine.card.owner().clone(),
        document: crate::model::Document::new("repo", "main", "removed.md", "", None).unwrap(),
        repository_url: "repository".into(),
        location: crate::tui::ResolvedDocument::Unavailable {
            selection: None,
            problem: crate::tui::DocumentProblem::MissingCheckout,
        },
    };
    f.server.clear();
    assert!(
        engine
            .open_document(&target)
            .unwrap_err()
            .to_string()
            .contains("document viewing context changed")
    );
    let world = f.world.lock().unwrap();
    assert_eq!(world.reads, 1);
    assert_eq!(world.display_reads, 1);
    assert!(f.server.methods().is_empty());
}

#[test]
fn a_moved_pi_is_found_by_terminal_and_its_existing_card_follows_without_a_baseline() {
    let f = Fixture::new();
    f.world
        .lock()
        .unwrap()
        .relocate("owner", "moved-owner", "other", "other-tab");
    f.engine().unwrap().project().unwrap();
    let mut world = f.world.lock().unwrap();
    let display = world.row("pane", "moved-card");
    assert_eq!(display["resource"], "original card");
    assert_eq!(display["tab_id"], "other-tab");
    assert_eq!(
        world.row("pane", "neighbor")["resource"],
        "unrelated terminal"
    );
    assert_eq!(world.row("tab", "other-tab")["label"], "User tab");
    assert_eq!(world.created, 0);
    assert_eq!(world.reads, 1);
    assert_eq!(
        world.display_reads, 0,
        "projection must not resolve display inputs"
    );
    assert_eq!(world.changes, ["original card"]);
    drop(world);
    f.server.clear();
    f.engine().unwrap().project().unwrap();
    assert!(
        f.server.methods().is_empty(),
        "settled pass must issue no RPC"
    );
}
#[test]
fn a_user_moved_card_returns_on_the_next_pass_including_after_child_restart() {
    let f = Fixture::new();
    f.world
        .lock()
        .unwrap()
        .relocate("card", "separated-card", "other", "other-tab");
    f.engine().unwrap().project().unwrap();
    assert_eq!(
        f.world.lock().unwrap().row("pane", "moved-card")["tab_id"],
        "t"
    );
    assert_eq!(f.world.lock().unwrap().created, 0);
}
#[test]
fn a_closed_card_stays_closed_during_projection_and_pi_replacement_ends_its_lifetime() {
    let f = Fixture::new();
    let frame = f.engine().unwrap().verify_display().unwrap();
    f.world.lock().unwrap().remove("card");
    f.engine().unwrap().project().unwrap();
    assert_eq!(f.world.lock().unwrap().created, 0);
    assert!(f.world.lock().unwrap().changes.is_empty());
    let context = LaunchedCardContext {
        scope: frame.scope().clone(),
        owner: f.owner.clone(),
        reader: SourceLaunch {
            environment: BTreeMap::new(),
            cwd: f.directory.path().into(),
        },
        lock_directory: f.directory.path().join("locks"),
        plugin_root: f.directory.path().into(),
        control_socket: None,
    };
    let mut lifetime = DisplayLifetime::new(context);
    f.world.lock().unwrap().row("pane", "owner")["agent_session"]["value"] =
        json!(uuid::Uuid::new_v4().to_string());
    let snapshot = f.world.lock().unwrap().observe();
    assert!(
        lifetime
            .observe_native(&f.client, &snapshot)
            .unwrap()
            .is_none()
    );
    assert!(f.engine().is_err());
    assert!(f.world.lock().unwrap().changes.is_empty());
}
#[test]
fn a_user_tab_rename_is_left_alone_when_pi_is_no_longer_first() {
    let f = Fixture::new();
    f.world
        .lock()
        .unwrap()
        .relocate("owner", "moved-owner", "other", "other-tab");
    {
        let mut world = f.world.lock().unwrap();
        world.row("tab", "other-tab")["label"] = json!("User rename");
        world.row("workspace", "other")["label"] = json!("Outside workspace name");
    }
    f.engine().unwrap().project().unwrap();
    let mut world = f.world.lock().unwrap();
    assert_eq!(world.row("tab", "other-tab")["label"], "User rename");
    assert_eq!(world.row("workspace", "other")["label"], "Effort");
    assert!(
        !world
            .changes
            .iter()
            .any(|resource| resource == "destination tab")
    );
}
#[test]
fn a_lost_open_reply_leaves_the_actual_card_and_ends_once_without_a_journal_or_retry() {
    let f = Fixture::new();
    {
        let mut world = f.world.lock().unwrap();
        world.remove("card");
        world.lost_open_reply = true;
    }
    let mut engine = f.engine().unwrap();
    assert!(engine.open_display(OpenMode::Explicit).is_err());
    assert_eq!(f.world.lock().unwrap().created, 1);
    assert_eq!(
        f.world.lock().unwrap().row("pane", "opened-1")["resource"],
        "new card"
    );
    assert_eq!(f.world.lock().unwrap().changes, ["new card"]);
    let snapshot = f.world.lock().unwrap().observe();
    let scope = engine.card.scope().clone();
    publish_card_marker(&f.client, &snapshot, "opened-1", &scope).unwrap();
    f.server.clear();
    assert_eq!(
        f.engine()
            .unwrap()
            .open_display(OpenMode::Explicit)
            .unwrap(),
        DisplayOutcome::Existing
    );
    assert_eq!(f.world.lock().unwrap().created, 1);
    assert!(f.server.methods().is_empty());
    for key in ["nonce", "pending", "workspace_id", "pane_id", "tab_id"] {
        let context = LaunchedCardContext {
            scope: scope.clone(),
            owner: f.owner.clone(),
            reader: SourceLaunch {
                environment: BTreeMap::new(),
                cwd: f.directory.path().into(),
            },
            lock_directory: f.directory.path().join("locks"),
            plugin_root: f.directory.path().into(),
            control_socket: None,
        };
        assert!(!serde_json::to_string(&context).unwrap().contains(key));
    }
}
#[test]
fn an_observed_disconnect_blocks_the_whole_engine_from_reused_source_and_destination_resources() {
    let f = Fixture::new();
    let mut subscription = f
        .client
        .socket()
        .subscribe(crate::frontend::herdr::events::EventFilter {
            kinds: [crate::frontend::herdr::events::EventKind::PaneMoved].into(),
            ..Default::default()
        })
        .unwrap();
    let cancellation = subscription.cancellation_handle();
    f.world
        .lock()
        .unwrap()
        .relocate("owner", "moved-owner", "other", "other-tab");
    let mut engine = f
        .engine_with(f.client.with_subscription(&cancellation))
        .unwrap();
    {
        let mut world = f.world.lock().unwrap();
        for collection in ["panes", "tabs", "workspaces"] {
            for row in world.snapshot[collection].as_array_mut().unwrap() {
                row["resource"] = json!("unrelated replacement");
            }
        }
    }
    let before = f.world.lock().unwrap().snapshot.clone();
    f.server.disconnect_subscriptions();
    assert_eq!(
        subscription.receive().unwrap(),
        crate::frontend::herdr::events::SubscriptionUpdate::Resubscribed
    );
    assert!(
        engine
            .close_display()
            .unwrap_err()
            .to_string()
            .contains("subscription ended")
    );
    assert!(
        engine
            .follow_display()
            .unwrap_err()
            .to_string()
            .contains("subscription ended")
    );
    assert!(
        engine
            .project()
            .unwrap_err()
            .to_string()
            .contains("subscription ended")
    );
    let world = f.world.lock().unwrap();
    assert_eq!(world.snapshot, before);
    assert!(world.changes.is_empty());
}

#[test]
fn lock_helper() {
    let Ok(directory) = std::env::var("STREAMS_TEST_CARD_LOCK") else {
        return;
    };
    let owner = HerdrInventoryPane::parse(&pane("owner", "pi-terminal", "w", "t", "Pi")).unwrap();
    let Some(_lock) = CardLock::acquire(Path::new(&directory), card(&owner).scope()).unwrap()
    else {
        println!("duplicate");
        return;
    };
    println!("active");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
}
fn lock_process(directory: &Path) -> std::process::Child {
    Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "runtime::native_cards::tests::lock_helper",
            "--nocapture",
        ])
        .env("STREAMS_TEST_CARD_LOCK", directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap()
}
fn wait_active(child: &mut std::process::Child) -> BufReader<std::process::ChildStdout> {
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert_ne!(output.read_line(&mut line).unwrap(), 0);
        if line.trim() == "active" {
            return output;
        }
    }
}
#[test]
fn duplicate_card_process_exits_without_waiting_and_does_not_steal_the_lock() {
    let directory = tempfile::tempdir().unwrap();
    let mut active = lock_process(directory.path());
    let _output = wait_active(&mut active);
    let duplicate = lock_process(directory.path()).wait_with_output().unwrap();
    assert!(duplicate.status.success());
    assert!(
        String::from_utf8(duplicate.stdout)
            .unwrap()
            .contains("duplicate")
    );
    let owner = HerdrInventoryPane::parse(&pane("owner", "pi-terminal", "w", "t", "Pi")).unwrap();
    assert!(
        CardLock::acquire(directory.path(), card(&owner).scope())
            .unwrap()
            .is_none()
    );
    active.stdin.take().unwrap().write_all(b"exit\n").unwrap();
    assert!(active.wait().unwrap().success());
    assert!(
        CardLock::acquire(directory.path(), card(&owner).scope())
            .unwrap()
            .is_some()
    );
}
#[test]
fn a_crashed_card_process_releases_the_os_lock_without_recovery_state() {
    let directory = tempfile::tempdir().unwrap();
    let mut active = lock_process(directory.path());
    let _output = wait_active(&mut active);
    active.kill().unwrap();
    active.wait().unwrap();
    let owner = HerdrInventoryPane::parse(&pane("owner", "pi-terminal", "w", "t", "Pi")).unwrap();
    assert!(
        CardLock::acquire(directory.path(), card(&owner).scope())
            .unwrap()
            .is_some()
    );
    for entry in std::fs::read_dir(directory.path()).unwrap() {
        assert!(std::fs::read(entry.unwrap().path()).unwrap().is_empty());
    }
}
