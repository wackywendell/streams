#![cfg(test)]

use super::*;
#[path = "display_tests.rs"]
mod display_tests;
use crate::model::{Document, Repository, Session, Stream};
use crate::tui::{
    CardView, CheckoutBinding, CheckoutSelection, DisplayContext, DocumentLocation,
    NativeOwnerObservation, Rect, ResolvedDocument,
};
use serde_json::json;
use std::os::unix::fs::DirBuilderExt;
use std::sync::Mutex;

fn card(native: &str, hints: Option<&str>) -> CardSnapshot {
    let mut stream = Stream::new("effort", "Effort", None, "host").unwrap();
    stream.goal = Some("private goal narrative".into());
    stream.goal_short = hints.map(str::to_owned);
    let mut session = Session::new("card", "host", "effort", "pi", native, "Conversation").unwrap();
    session.state = "private state narrative".into();
    session.remaining = "private remaining narrative".into();
    session.state_short = hints.map(str::to_owned);
    CardSnapshot::new(
        stream,
        session,
        DisplayContext {
            catalog_source: "catalog".into(),
            owner: NativeOwnerObservation {
                pane_id: "owner".into(),
                workspace_id: "w".into(),
                tab_id: "t".into(),
            },
            checkouts: vec![],
            documents: vec![],
        },
    )
    .unwrap()
}

#[test]
fn conversation_key_excludes_membership_and_catalog_session_id() {
    let mut scope = card("native", None).scope().clone();
    let key = conversation_key(&scope).unwrap();
    assert_eq!(
        key,
        "74e4e8706a77d276b78161d6c928a8c08bcd5c336531bdb5f5d00ee83ddae55a"
    );
    scope.stream_id = "destination".into();
    scope.session_id = "migrated-catalog-id".into();
    assert_eq!(conversation_key(&scope).unwrap(), key);
}

#[test]
fn conversation_key_pins_principal_and_uses_compact_utf8_wire() {
    let base = card("native", None).scope().clone();
    let changes: [fn(&mut Scope); 4] = [
        |s| s.catalog_source.push('x'),
        |s| s.host_id.push('x'),
        |s| s.harness.push('x'),
        |s| s.native_session_id.push('x'),
    ];
    for change in changes {
        let mut changed = base.clone();
        change(&mut changed);
        assert_ne!(
            conversation_key(&changed).unwrap(),
            conversation_key(&base).unwrap()
        );
    }
    let mut unicode = base.clone();
    unicode.catalog_source = "café".into();
    unicode.host_id = "hôte".into();
    unicode.native_session_id = "会話".into();
    assert_eq!(
        conversation_key(&unicode).unwrap(),
        "5309df7f0ce5e0359a87657021bd967bf3b5d43c2b331fd49775d9ad4587f8c6"
    );
    for invalid in ["", " ", "bad\nsource"] {
        unicode.catalog_source = invalid.into();
        assert!(conversation_key(&unicode).is_err());
    }
}

struct World {
    snapshot: Value,
    current: String,
    observation: CardObservation,
    effects: Vec<(String, Value)>,
    malformed_open: bool,
    ambiguous_close: bool,
    fail_after_open: bool,
    deny_read: bool,
    inventory_reads: usize,
    observation_reads: usize,
    clear_binding_after_caption_rename: bool,
}
struct Source {
    world: Arc<Mutex<World>>,
    launch: SourceLaunch,
}
impl NativeCardSource for Source {
    fn read(
        &mut self,
        _client: &HerdrClient,
        _snapshot: &HerdrNativeSnapshot,
        inventory: bool,
    ) -> Result<CardObservation> {
        let mut world = self.world.lock().unwrap();
        world.observation_reads += 1;
        if inventory {
            world.inventory_reads += 1;
        }
        if world.deny_read || (inventory && world.observation.handover.is_none()) {
            return Err(invalid(
                "fake application authority/creation context unavailable",
            ));
        }
        let mut observation = world.observation.clone();
        if !inventory {
            observation.handover = None;
        }
        Ok(observation)
    }
    fn resolve_document(
        &mut self,
        _client: &HerdrClient,
        _snapshot: &HerdrNativeSnapshot,
        target: &DocumentTarget,
    ) -> Result<ResolvedEditor> {
        let world = self.world.lock().unwrap();
        if world.deny_read
            || world
                .observation
                .card
                .document_target(&target.document.repository, &target.document.path)
                .as_ref()
                != Some(target)
        {
            return Err(invalid("painted document changed"));
        }
        let ResolvedDocument::File { selection, .. } = &target.location else {
            return Err(invalid("document unavailable"));
        };
        Ok(ResolvedEditor {
            root: selection.root.clone(),
            identity: target
                .editor_identity()
                .map_err(|e| invalid(e.to_string()))?,
        })
    }
    fn launch_context(&self) -> SourceLaunch {
        self.launch.clone()
    }
}
struct FakeHerdr {
    world: Arc<Mutex<World>>,
    state: Arc<NativeState>,
    socket: PathBuf,
    plugin: Value,
    key: String,
}
fn pane(id: &str, terminal: &str) -> Value {
    json!({"pane_id":id,"terminal_id":terminal,"workspace_id":"w","tab_id":"t",
        "cwd":"/observed","label":"Streams work card","tokens":{}})
}
fn add_card(world: &mut World, id: &str, terminal: &str) {
    world.snapshot["panes"]
        .as_array_mut()
        .unwrap()
        .push(pane(id, terminal));
    world.snapshot["layouts"][0]["panes"]
        .as_array_mut()
        .unwrap()
        .push(json!({"pane_id":id}));
}
fn document_keys(socket: &Path, target: &DocumentTarget) -> (String, String) {
    let identity = target.editor_identity().unwrap();
    let key = json_digest(&(
        "streams-native-editor",
        2,
        &identity.catalog_source,
        &identity.host_id,
        &identity.checkout_id,
        &identity.file,
    ))
    .unwrap();
    let evidence = json_digest(&(socket, &target.owner.workspace_id, "editor", &key)).unwrap();
    (key, evidence)
}
fn remove_card(world: &mut World) {
    world.snapshot["panes"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["pane_id"] != "display");
    world.snapshot["layouts"][0]["panes"]
        .as_array_mut()
        .unwrap()
        .retain(|p| p["pane_id"] != "display");
}
impl FakeHerdr {
    fn handle(&self, method: &str, p: &Value) -> Value {
        let mut w = self.world.lock().unwrap();
        let call = (method.to_owned(), p.clone());
        match method {
            "pane.current" => {
                assert_eq!(p["caller_pane_id"], w.current);
                let current = w.snapshot["panes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| p["pane_id"] == w.current)
                    .unwrap();
                json!({"type":"pane_current","pane":current})
            }
            "session.snapshot" => json!({"type":"session_snapshot","snapshot":w.snapshot}),
            "plugin.list" => {
                assert_eq!(p["plugin_id"], PLUGIN_ID);
                json!({"type":"plugin_list","plugins":[self.plugin]})
            }
            "plugin.pane.open" => {
                assert_eq!(p["plugin_id"], PLUGIN_ID);
                let entrypoint = p["entrypoint"].as_str().unwrap();
                assert_eq!(p["focus"], false);
                let evidence_key = if entrypoint == "document" {
                    let target = w
                        .observation
                        .card
                        .document_target("lib", "docs/design.md")
                        .unwrap();
                    let ResolvedDocument::File { selection, file } = &target.location else {
                        panic!("not a file")
                    };
                    assert_eq!(p["workspace_id"], "w");
                    assert_eq!(p["placement"], "tab");
                    assert!(p.get("target_pane_id").is_none());
                    assert_eq!(p["env"]["WORK_CARDS_DOCUMENT_COMMAND"], "less");
                    assert_eq!(
                        p["env"]["WORK_CARDS_DOCUMENT_CWD"],
                        selection.root.to_str().unwrap()
                    );
                    assert_eq!(p["env"]["WORK_CARDS_DOCUMENT_PATH"], file.to_str().unwrap());
                    document_keys(&self.socket, &target).1
                } else {
                    assert_eq!(entrypoint, "card");
                    assert_eq!(p["target_pane_id"], "owner");
                    self.key.clone()
                };
                // Resource creation must never precede durable pending evidence.
                let pending = self
                    .state
                    .read(&evidence_key)
                    .unwrap()
                    .expect("pending before native launch");
                assert!(matches!(pending.phase(), NativePhase::Unlaunched));
                if entrypoint == "card" {
                    assert_eq!(p["env"]["STREAMS_CARDS_NONCE"], pending.nonce());
                }
                let launch = w
                    .effects
                    .iter()
                    .filter(|(m, e)| m == "plugin.pane.open" && e["entrypoint"] == entrypoint)
                    .count()
                    + 1;
                w.effects.push(call);
                let mut created = if entrypoint == "card" {
                    let suffix = if launch == 1 {
                        String::new()
                    } else {
                        format!("-{launch}")
                    };
                    let id = format!("display{suffix}");
                    let terminal = format!("display-terminal{suffix}");
                    add_card(&mut w, &id, &terminal);
                    pane(&id, &terminal)
                } else {
                    let tab = format!("editor-tab-{launch}");
                    let mut editor = pane(
                        &format!("editor-{launch}"),
                        &format!("editor-terminal-{launch}"),
                    );
                    editor["tab_id"] = json!(tab);
                    w.snapshot["panes"]
                        .as_array_mut()
                        .unwrap()
                        .push(editor.clone());
                    w.snapshot["tabs"].as_array_mut().unwrap().push(json!({
                        "workspace_id":"w","tab_id":tab,"label":(launch+1).to_string(),"number":launch + 1,
                    }));
                    w.snapshot["layouts"].as_array_mut().unwrap().push(json!({
                        "workspace_id":"w","tab_id":tab,"panes":[{"pane_id":editor["pane_id"]}],
                    }));
                    editor
                };
                if w.fail_after_open {
                    w.deny_read = true;
                }
                if w.malformed_open {
                    created = json!({"pane_id":created["pane_id"]});
                }
                json!({"type":"plugin_pane_opened","plugin_pane":{
                    "plugin_id":PLUGIN_ID,"entrypoint":entrypoint,"pane":created}})
            }
            "workspace.rename" | "tab.rename" | "pane.rename" => {
                w.effects.push(call);
                let (kind, collection, key, result) = match method {
                    "workspace.rename" => {
                        ("workspace", "workspaces", "workspace_id", "workspace_info")
                    }
                    "tab.rename" => ("tab", "tabs", "tab_id", "tab_info"),
                    _ => ("pane", "panes", "pane_id", "pane_info"),
                };
                let target = w.snapshot[collection]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|t| t[key] == p[key])
                    .unwrap();
                target["label"] = p["label"].clone();
                let target = target.clone();
                if kind == "pane" && w.clear_binding_after_caption_rename {
                    w.snapshot["workspaces"][0]["tokens"] = json!({});
                }
                json!({"type":result,kind:target})
            }
            "tab.focus" => {
                let target = w
                    .observation
                    .card
                    .document_target("lib", "docs/design.md")
                    .unwrap();
                let (key, evidence) = document_keys(&self.socket, &target);
                let ready = self.state.read(&evidence).unwrap().unwrap();
                assert!(matches!(ready.phase(), NativePhase::Ready(_)));
                assert_eq!(ready.resources().unwrap().tab_id(), p["tab_id"]);
                let editor = w.snapshot["panes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|e| e["tab_id"] == p["tab_id"])
                    .unwrap();
                assert_eq!(editor["tokens"]["strm_doc"], key);
                assert_eq!(editor["tokens"]["strm_editor"], ready.nonce());
                w.effects.push(call);
                json!({"type":"ok"})
            }
            "workspace.report_metadata" | "pane.report_metadata" => {
                assert_eq!(p["source"], PLUGIN_ID);
                w.effects.push(call);
                let (collection, id_key) = if method == "workspace.report_metadata" {
                    ("workspaces", "workspace_id")
                } else {
                    ("panes", "pane_id")
                };
                let target = w.snapshot[collection]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|t| t[id_key] == p[id_key])
                    .expect("exact metadata target");
                let tokens = target["tokens"].as_object_mut().unwrap();
                for (key, value) in p["tokens"].as_object().unwrap() {
                    match value {
                        Value::String(_) => {
                            tokens.insert(key.clone(), value.clone());
                        }
                        Value::Null => {
                            tokens.remove(key);
                        }
                        _ => panic!("unexpected metadata token: {p}"),
                    }
                }
                json!({"type":"ok"})
            }
            "pane.resize" => {
                assert!(matches!(
                    self.state.read(&self.key).unwrap().unwrap().phase(),
                    NativePhase::Ready(_)
                ));
                assert!(p["amount"].is_f64());
                w.effects.push(call);
                json!({"type":"pane_resize","resize":{"pane_id":p["pane_id"]}})
            }
            "plugin.pane.close" => {
                assert_eq!(p["pane_id"], "display");
                assert!(
                    matches!(
                        self.state.read(&self.key).unwrap().unwrap().phase(),
                        NativePhase::Closed(_)
                    ),
                    "closed intent must precede native close"
                );
                w.effects.push(call);
                remove_card(&mut w);
                json!({"type":"plugin_pane_closed","pane_id":
                    if w.ambiguous_close { "another-pane" } else { "display" }})
            }
            _ => panic!("unsupported fake Herdr request: {method} {p}"),
        }
    }
}
struct Fixture {
    // Declared before the directory holding its socket.
    server: fake_herdr::FakeHerdr,
    _temp: tempfile::TempDir,
    runner: Arc<FakeHerdr>,
    config: NativeCardsConfig,
    environment: BTreeMap<String, String>,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let plugin_root = root.join("plugin");
        std::fs::create_dir(&plugin_root).unwrap();
        let config_path = root.join("config.json");
        std::fs::write(&config_path, r#"{"pane_height_fraction":0.2}"#).unwrap();
        let environment = BTreeMap::from([
            ("HOME".into(), root.to_str().unwrap().into()),
            (
                "WORK_CARDS_CONFIG_PATH".into(),
                config_path.to_str().unwrap().into(),
            ),
            ("HERDR_ENV".into(), "1".into()),
            ("HERDR_PANE_ID".into(), "owner".into()),
            (
                "HERDR_SOCKET_PATH".into(),
                root.join("herdr.sock").to_str().unwrap().into(),
            ),
        ]);
        let config = NativeCardsConfig::load(&environment, &root, &plugin_root).unwrap();
        let state = Arc::new(NativeState::new(root.join("state")).unwrap());
        let socket = root.join("herdr.sock");
        let observation = CardObservation {
            card: card("native", None),
            handover: None,
        };
        let key = conversation_key(observation.card.scope()).unwrap();
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(state.directory())
            .unwrap();
        crate::file_io::write_json(
            &state.directory().join("format.json"),
            &json!({"version":2}),
        )
        .unwrap();
        let world = Arc::new(Mutex::new(World {
            snapshot: json!({
                "workspaces":[{"workspace_id":"w","label":"Workspace","number":1,"tokens":{}}],
                "tabs":[{"workspace_id":"w","tab_id":"t","label":"1","number":1}],
                "panes":[pane("owner", "owner-terminal")],
                "layouts":[{"workspace_id":"w","tab_id":"t","panes":[{"pane_id":"owner"}]}],
                "agents":[],
            }),
            current: "owner".into(),
            observation,
            effects: vec![],
            malformed_open: false,
            ambiguous_close: false,
            fail_after_open: false,
            deny_read: false,
            inventory_reads: 0,
            observation_reads: 0,
            clear_binding_after_caption_rename: false,
        }));
        let plugin = json!({"plugin_id":PLUGIN_ID,"enabled":true,"plugin_root":plugin_root,
        "manifest_path":plugin_root.join("streams-rust-herdr-plugin.toml"),
        "source":{"kind":"local"},"version":"0.1.0","min_herdr_version":"0.9.3",
        "platforms":["macos","linux"],"panes":[
            {"id":"card","title":"Streams work card","placement":"split","command":["/bin/sh","bin/open-rust-streams-pane.sh"]},
            {"id":"document","title":"Document","placement":"tab","command":["/bin/sh","bin/open-document.sh"]},
        ]});
        let runner = Arc::new(FakeHerdr {
            world,
            state,
            socket,
            plugin,
            key,
        });
        let server = fake_herdr::FakeHerdr::start(&runner.socket, {
            let runner = runner.clone();
            move |method, params| {
                fake_herdr::Reply::Result(if method == "events.subscribe" {
                    json!({"type":"subscription_started"})
                } else {
                    runner.handle(method, params)
                })
            }
        });
        Self {
            server,
            _temp: temp,
            runner,
            config,
            environment,
        }
    }
    fn engine(&self) -> NativeCardEngine {
        self.engine_with(self.environment.clone())
    }
    fn engine_with(&self, environment: BTreeMap<String, String>) -> NativeCardEngine {
        self.try_engine_with(environment).unwrap()
    }
    fn try_engine_with(&self, environment: BTreeMap<String, String>) -> Result<NativeCardEngine> {
        self.runner.world.lock().unwrap().current = environment["HERDR_PANE_ID"].clone();
        NativeCardEngine::new(
            HerdrClient::connect(environment.clone(), 1.0).unwrap(),
            Box::new(Source {
                world: self.runner.world.clone(),
                launch: SourceLaunch {
                    environment: self.environment.clone(),
                    cwd: self._temp.path().canonicalize().unwrap(),
                },
            }),
            self.runner.state.clone(),
            self.config.clone(),
            environment,
            Arc::new(AtomicBool::new(false)),
        )
    }
    fn launched_environment(&self) -> BTreeMap<String, String> {
        let mut environment = self.environment.clone();
        let scope = self
            .runner
            .world
            .lock()
            .unwrap()
            .observation
            .card
            .scope()
            .clone();
        environment.extend(BTreeMap::from([
            ("HERDR_PANE_ID".into(), "display".into()),
            (
                "STREAMS_CARDS_SCOPE".into(),
                serde_json::to_string(&ScopeWire::from(&scope)).unwrap(),
            ),
            (
                "STREAMS_CARDS_OWNER".into(),
                json!({"pane_id":"owner","workspace_id":"w","tab_id":"t"}).to_string(),
            ),
            ("STREAMS_CARDS_NONCE".into(), self.record().nonce().into()),
            (
                "STREAMS_CARDS_OWNER_TERMINAL".into(),
                "owner-terminal".into(),
            ),
            ("STREAMS_CARDS_READ_ENV".into(), "{}".into()),
            (
                "STREAMS_CARDS_READ_CWD".into(),
                self._temp.path().to_str().unwrap().into(),
            ),
            (
                "STREAMS_CARDS_STATE_DIR".into(),
                self.runner.state.directory().to_str().unwrap().into(),
            ),
            (
                "STREAMS_CARDS_PLUGIN_ROOT".into(),
                self.config.plugin_root.to_str().unwrap().into(),
            ),
        ]));
        environment
    }
    fn document(&self) -> DocumentTarget {
        let root = self
            ._temp
            .path()
            .canonicalize()
            .unwrap()
            .join("viewing-checkout");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        let file = root.join("docs/design.md");
        std::fs::write(&file, "current viewing checkout document").unwrap();
        let base = card("native", None);
        let mut stream = base.stream().clone();
        stream.repositories.insert(
            "lib".into(),
            Repository::new("https://example.com/lib").unwrap(),
        );
        stream
            .documents
            .push(Document::new("lib", "topic", "docs/design.md", "Design", None).unwrap());
        let selection = CheckoutSelection {
            checkout_id: "viewing-checkout".into(),
            root,
        };
        let snapshot = CardSnapshot::new(
            stream,
            base.session().clone(),
            DisplayContext {
                catalog_source: base.scope().catalog_source.clone(),
                owner: base.owner().clone(),
                checkouts: vec![CheckoutBinding {
                    repository: "lib".into(),
                    selection: selection.clone(),
                }],
                documents: vec![DocumentLocation {
                    repository: "lib".into(),
                    path: "docs/design.md".into(),
                    location: ResolvedDocument::File { selection, file },
                }],
            },
        )
        .unwrap();
        self.runner.world.lock().unwrap().observation.card = snapshot.clone();
        let mut view = CardView::new();
        let refresh = view.begin_refresh();
        assert!(view.complete_refresh(refresh, Ok(snapshot)));
        let super::super::super::tui::Target::Document(document) =
            view.paint(Rect::new(0, 0, 100, 20)).documents[&'a']
                .target()
                .clone()
        else {
            panic!("document shortcut")
        };
        *document
    }
    fn record(&self) -> NativeAttempt {
        self.runner.state.read(&self.runner.key).unwrap().unwrap()
    }
    fn write(&self, old: Option<&NativeAttempt>, next: &NativeAttempt) {
        let cancelled = AtomicBool::new(false);
        self.runner
            .state
            .lock(1.0, &cancelled)
            .unwrap()
            .write(old, next)
            .unwrap();
    }
    fn effects(&self) -> Vec<(String, Value)> {
        self.runner.world.lock().unwrap().effects.clone()
    }
    fn launches(&self) -> usize {
        self.effects()
            .iter()
            .filter(|(method, _)| method == "plugin.pane.open")
            .count()
    }
    fn bind(&self) {
        let mut w = self.runner.world.lock().unwrap();
        w.snapshot["workspaces"][0]["tokens"] = json!({
            "strm_catalog":json_digest(&"catalog").unwrap(),"strm_host":"host",
            "strm_stream":"effort","strm_binding":"binding-lifetime",
        });
    }
    fn prepare_predecessor(&self) {
        self.engine().open_display(OpenMode::Explicit).unwrap();
        let snapshot = {
            let mut world = self.runner.world.lock().unwrap();
            remove_card(&mut world);
            let native = "11111111-1111-4111-8111-111111111111";
            world.observation.card = card(native, None);
            world.snapshot["panes"][0]["agent"] = json!("pi");
            world.snapshot["panes"][0]["agent_session"] =
                json!({"agent":"pi","kind":"id","value":native});
            world.snapshot.clone()
        };
        let client = HerdrClient::connect(self.environment.clone(), 1.0).unwrap();
        let native =
            super::super::native::native_inventory_from_snapshot(&client, &snapshot).unwrap();
        let mut world = self.runner.world.lock().unwrap();
        world.observation.handover = Some(HandoverInventory {
            scopes: vec![world.observation.card.scope().clone()],
            native,
        });
    }
    fn reserve(&self, partial: bool) {
        self.bind();
        let pending = NativeAttempt::new(
            &self.runner.key,
            "reserved-nonce",
            &self.runner.socket,
            NativePurpose::Card,
        )
        .unwrap();
        self.write(None, &pending);
        let mut attempt = pending.clone();
        attempt
            .reserve(Resources::new("display", "display-terminal", "t").unwrap())
            .unwrap();
        self.write(Some(&pending), &attempt);
        let mut w = self.runner.world.lock().unwrap();
        add_card(&mut w, "display", "display-terminal");
        if partial {
            let identity = conversation_key(w.observation.card.scope()).unwrap();
            w.snapshot["panes"][1]["tokens"] = json!({"strm_agent":identity,
                "strm_nonce":attempt.nonce(),"strm_pi":"owner-terminal","strm_card":"display-terminal",
                "strm_native":"native","strm_role":"card"});
        }
    }
}

#[test]
fn owner_observation_is_readonly_and_lifetime_expectations_fail_closed() {
    let f = Fixture::new();
    let mut engine = f.engine();
    let observed = engine.observe_owner().unwrap();
    assert_eq!(observed.context.pane_id, "owner");
    assert!(f.runner.state.read(&f.runner.key).unwrap().is_none());
    assert!(f.effects().is_empty());
    engine
        .pin_owner(observed.card.scope(), &observed.context)
        .unwrap();
    for field in 0..6 {
        let mut changed = observed.context.clone();
        match field {
            0 => changed.socket = PathBuf::from("/different/socket"),
            1 => changed.pane_id = "different".into(),
            2 => changed.terminal_id = "different".into(),
            3 => changed.workspace_id = "different".into(),
            4 => changed.tab_id = "different".into(),
            _ => changed.binding = Some("different".into()),
        }
        let mut candidate = f.engine();
        assert!(
            candidate
                .pin_owner(observed.card.scope(), &changed)
                .and_then(|()| candidate.observe_owner())
                .is_err()
        );
    }
    for field in 0..6 {
        let mut changed = observed.card.scope().clone();
        match field {
            0 => changed.catalog_source = "different".into(),
            1 => changed.host_id = "different".into(),
            2 => changed.stream_id = "different".into(),
            3 => changed.session_id = "different".into(),
            4 => changed.native_session_id = "different".into(),
            _ => changed.harness = "different".into(),
        }
        let mut candidate = f.engine();
        assert!(
            candidate
                .pin_owner(&changed, &observed.context)
                .and_then(|()| candidate.observe_owner())
                .is_err()
        );
    }
    assert!(f.effects().is_empty());
    assert!(f.runner.state.read(&f.runner.key).unwrap().is_none());
    engine.open_display(OpenMode::Explicit).unwrap();
    let current = f.engine().observe_owner().unwrap();
    assert!(current.context.binding.is_some());
    let mut candidate = f.engine();
    candidate
        .pin_owner(observed.card.scope(), &observed.context)
        .and_then(|()| candidate.observe_owner())
        .unwrap();
    let mut candidate = f.engine();
    candidate
        .pin_owner(current.card.scope(), &current.context)
        .and_then(|()| candidate.observe_owner())
        .unwrap();
    f.runner.world.lock().unwrap().snapshot["workspaces"][0]["tokens"]["strm_binding"] =
        json!("replacement");
    let before = f.effects().len();
    let mut candidate = f.engine();
    assert!(
        candidate
            .pin_owner(current.card.scope(), &current.context)
            .and_then(|()| candidate.observe_owner())
            .is_err()
    );
    assert_eq!(before, f.effects().len());
}
#[test]
fn native_preparation_registers_restrictions_without_observing() {
    let f = Fixture::new();
    let observed = f.engine().observe_owner().unwrap();
    f.server.clear();
    f.runner.world.lock().unwrap().observation_reads = 0;
    let mut engine = f.engine();
    engine
        .require_caller_terminal(&observed.context.terminal_id)
        .unwrap();
    engine
        .pin_owner(observed.card.scope(), &observed.context)
        .unwrap();
    assert_eq!(f.server.methods(), ["ping"]);
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 0);
    assert!(f.effects().is_empty());
    engine.observe_owner().unwrap();
    assert_eq!(f.server.methods(), ["ping", "session.snapshot"]);
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 1);
}

#[test]
fn caller_restriction_is_verified_before_catalog_reads_or_effects() {
    let f = Fixture::new();
    let mut engine = f.engine();
    engine.require_caller_terminal("another-terminal").unwrap();
    assert!(engine.open_display(OpenMode::Explicit).is_err());
    assert_eq!(f.runner.world.lock().unwrap().observation_reads, 0);
    assert!(f.effects().is_empty());
    assert!(f.runner.state.read(&f.runner.key).unwrap().is_none());
}

#[test]
fn card_pane_cannot_use_the_owner_bridge() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let mut engine = f.engine_with(f.launched_environment());
    let before = f.effects().len();
    assert!(engine.observe_owner().is_err());
    assert_eq!(before, f.effects().len());
}
#[test]
fn operation_request_budgets() {
    for operation in ["observe", "check", "open", "close", "project"] {
        let f = Fixture::new();
        f.server.clear();
        let mut engine = f.engine();
        // Close exercises an owned, ready card, not the no-op absence path.
        if operation == "close" {
            engine.open_display(OpenMode::Explicit).unwrap();
            f.server.clear();
        }
        match operation {
            "observe" => {
                engine.observe_owner().unwrap();
            }
            "check" => {
                engine.status().unwrap();
            }
            "open" => {
                engine.open_display(OpenMode::Explicit).unwrap();
            }
            "close" => {
                engine.close_display().unwrap();
            }
            "project" => {
                engine.project_summaries().unwrap();
                engine.project_names().unwrap();
                engine.project_caption().unwrap();
            }
            _ => unreachable!(),
        }
        let mut counts = BTreeMap::new();
        for method in f.server.methods() {
            *counts.entry(method).or_insert(0_usize) += 1;
        }
        // One ping per process (close reuses the ready engine), then one socket
        // request per observation or effect. Every effect invalidates the
        // observation for its confirmation; taking the workspace lock also
        // invalidates the pre-lock display snapshot.
        let expected: &[(&str, usize)] = match operation {
            "observe" | "check" => &[("ping", 1), ("session.snapshot", 1)],
            "open" => &[
                ("pane.report_metadata", 2),
                ("pane.resize", 1),
                ("ping", 1),
                ("plugin.list", 1),
                ("plugin.pane.open", 1),
                ("session.snapshot", 7),
                ("workspace.report_metadata", 1),
            ],
            "close" => &[("plugin.pane.close", 1), ("session.snapshot", 3)],
            "project" => &[
                ("pane.report_metadata", 1),
                ("ping", 1),
                ("session.snapshot", 9),
                ("tab.rename", 1),
                ("workspace.rename", 1),
                ("workspace.report_metadata", 1),
            ],
            _ => unreachable!(),
        };
        let expected: BTreeMap<String, usize> =
            expected.iter().map(|(m, n)| ((*m).into(), *n)).collect();
        assert_eq!(counts, expected, "{operation}");
    }
}

#[test]
fn native_open_baseline_source_evidence_and_wait_counts() {
    use super::super::native_state::accounting::{self, Io};
    let f = Fixture::new();
    f.server.clear();
    accounting::take();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    let io = accounting::take();
    let source_reads = f.runner.world.lock().unwrap().observation_reads;
    let probes = io
        .iter()
        .filter(|event| matches!(event, Io::Probe(_)))
        .count();
    let reads = io
        .iter()
        .filter(|event| matches!(event, Io::Read(_)))
        .count();
    let waits = io
        .iter()
        .filter(|event| matches!(event, Io::Wait(_)))
        .count();
    // The fake source counts context requests, not real catalog file reads.
    // The CLI/catalog integration budget must count those separately.
    assert_eq!((source_reads, probes, reads, waits), (16, 38, 31, 0));
    let evidence = f
        .runner
        .state
        .directory()
        .join(format!("{}.json", f.runner.key));
    let format = f.runner.state.directory().join("format.json");
    assert_eq!(
        io.iter()
            .filter(|event| matches!(event, Io::Read(path) if path == &format))
            .count(),
        19
    );
    assert_eq!(
        io.iter()
            .filter(|event| matches!(event, Io::Read(path) if path == &evidence))
            .count(),
        12
    );
    for event in io {
        if let Io::Probe(path) | Io::Read(path) = event {
            assert!(path == evidence || path == format);
        }
    }
}

#[test]
fn bootstrap_and_verify_are_read_only_and_require_pair_and_durable_evidence() {
    let f = Fixture::new();
    let mut engine = f.engine();
    assert!(engine.verify_display().is_err());
    assert!(f.runner.state.read(&f.runner.key).unwrap().is_none());
    let lock = f.runner.state.directory().join(".native.lock");
    assert!(!lock.exists());
    assert!(f.effects().is_empty());
    f.reserve(true);
    let old = f.record();
    assert!(matches!(
        engine.open_display(OpenMode::Explicit).unwrap(),
        DisplayOutcome::Existing(_)
    ));
    assert!(engine.verify_display().is_ok());
    // A complete painted/native pair cannot replace its separate durable witness.
    let absent = Arc::new(NativeState::new(f._temp.path().join("absent-state")).unwrap());
    engine.state = absent.clone();
    assert!(engine.verify_display().is_err());
    assert!(!absent.directory().exists());
    assert_eq!(old.nonce(), f.record().nonce());
}

#[test]
fn missing_completed_format_refuses_launch_without_legacy_lookup_or_repair() {
    let f = Fixture::new();
    std::fs::remove_file(f.runner.state.directory().join("format.json")).unwrap();
    let error = f.engine().open_display(OpenMode::Explicit).unwrap_err();
    assert!(matches!(
        error,
        super::super::RuntimeError::NativeEvidenceResetRequired
    ));
    assert!(f.effects().is_empty());
    assert!(!f.runner.state.directory().join("format.json").exists());
}

#[test]
fn conversation_key_cannot_adopt_evidence_from_another_endpoint() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let original = f.record();
    let mut wire = serde_json::to_value(&original).unwrap();
    wire["endpoint"] = "/another/herdr.sock".into();
    crate::file_io::write_json(
        &f.runner
            .state
            .directory()
            .join(format!("{}.json", f.runner.key)),
        &wire,
    )
    .unwrap();
    let before = f.effects();
    assert!(
        f.engine()
            .open_display(OpenMode::Explicit)
            .unwrap_err()
            .to_string()
            .contains("another Herdr endpoint")
    );
    assert!(f.engine().status().is_err());
    assert_eq!(f.effects(), before);
    assert_eq!(f.record().nonce(), original.nonce());
    assert_eq!(f.record().phase(), original.phase());
}

#[test]
fn fresh_open_persists_pending_then_exact_pair_and_ready_and_launches_once() {
    let f = Fixture::new();
    let mut engine = f.engine();
    let DisplayOutcome::Opened(resources) = engine.open_display(OpenMode::Automatic).unwrap()
    else {
        panic!("not opened")
    };
    assert_eq!(
        resources,
        Resources::new("display", "display-terminal", "t").unwrap()
    );
    let ready = f.record();
    assert!(matches!(ready.phase(), NativePhase::Ready(_)));
    let w = f.runner.world.lock().unwrap();
    let snapshot = HerdrNativeSnapshot::parse(&w.snapshot).unwrap();
    let identity = conversation_key(w.observation.card.scope()).unwrap();
    assert!(
        matches!(snapshot.classify_pair(&identity, "owner-terminal").unwrap(), PairState::Paired { nonce, .. } if nonce == ready.nonce())
    );
    for (index, role) in [(0, "agent"), (1, "card")] {
        let tokens = &w.snapshot["panes"][index]["tokens"];
        assert_eq!(tokens["strm_role"], role);
        assert_eq!(tokens["strm_pi"], "owner-terminal");
        assert_eq!(tokens["strm_card"], "display-terminal");
        assert_eq!(tokens["strm_nonce"], ready.nonce());
        assert_eq!(tokens["strm_native"], "native");
    }
    drop(w);
    let effects = f.effects();
    assert!(matches!(
        engine.open_display(OpenMode::Explicit).unwrap(),
        DisplayOutcome::Existing(_)
    ));
    assert_eq!(f.effects(), effects);
    assert_eq!(f.launches(), 1);
    assert!(engine.verify_display().is_ok());
}

#[test]
fn malformed_open_ack_retains_unlaunched_and_never_duplicates_resource() {
    let f = Fixture::new();
    f.runner.world.lock().unwrap().malformed_open = true;
    let mut engine = f.engine();
    assert!(engine.open_display(OpenMode::Explicit).is_err());
    assert!(matches!(f.record().phase(), NativePhase::Unlaunched));
    let effects = f.effects();
    for mode in [OpenMode::Explicit, OpenMode::Automatic] {
        assert!(engine.open_display(mode).is_err());
        assert!(f.engine().open_display(mode).is_err());
    }
    assert_eq!(f.effects(), effects);
    assert_eq!(f.launches(), 1);
}

#[test]
fn authority_loss_after_creation_stops_stamping_and_retains_pending() {
    let f = Fixture::new();
    f.runner.world.lock().unwrap().fail_after_open = true;
    assert!(f.engine().open_display(OpenMode::Explicit).is_err());
    assert!(matches!(f.record().phase(), NativePhase::Reserved(_)));
    assert_eq!(f.record().resources().unwrap().pane_id(), "display");
    let effects = f.effects();
    assert_eq!(effects.last().unwrap().0, "plugin.pane.open");
    let w = f.runner.world.lock().unwrap();
    assert_eq!(w.snapshot["panes"][0]["tokens"], json!({}));
    assert_eq!(w.snapshot["panes"][1]["tokens"], json!({}));
    drop(w);
    assert!(f.engine().open_display(OpenMode::Automatic).is_err());
    assert_eq!(f.effects(), effects);
}

#[test]
fn ready_and_closed_absent_resources_allow_automatic_open_in_a_new_pi_lifetime() {
    for close in [false, true] {
        let f = Fixture::new();
        let mut engine = f.engine();
        engine.open_display(OpenMode::Explicit).unwrap();
        if close {
            engine.close_display().unwrap();
        } else {
            remove_card(&mut f.runner.world.lock().unwrap());
        }
        // Pi admits automatic opening only at session start/resume. The native
        // layer still proves absence, but earlier close evidence is not a veto.
        assert!(matches!(
            f.engine().open_display(OpenMode::Automatic).unwrap(),
            DisplayOutcome::Opened(_)
        ));
        assert!(matches!(
            f.engine().open_display(OpenMode::Explicit).unwrap(),
            DisplayOutcome::Existing(_)
        ));
        assert_eq!(f.launches(), 2);
        assert_eq!(
            f.record().resources().unwrap(),
            &Resources::new("display-2", "display-terminal-2", "t").unwrap()
        );
        assert!(engine.verify_display().is_ok());
    }
}

#[test]
fn reserved_exact_tuple_and_partial_markers_recover_without_launch() {
    for partial in [false, true] {
        let f = Fixture::new();
        f.reserve(partial);
        let mut engine = f.engine();
        assert!(matches!(
            engine.open_display(OpenMode::Explicit).unwrap(),
            DisplayOutcome::Existing(_)
        ));
        assert!(matches!(f.record().phase(), NativePhase::Ready(_)));
        assert!(engine.verify_display().is_ok());
        assert_eq!(f.launches(), 0);
    }
}

#[test]
fn reserved_reused_or_moved_tuple_remains_fenced() {
    for moved in [false, true] {
        let f = Fixture::new();
        f.reserve(false);
        {
            let mut w = f.runner.world.lock().unwrap();
            if moved {
                w.snapshot["tabs"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"workspace_id":"w","tab_id":"other-tab","label":"2","number":2}));
                w.snapshot["panes"][1]["tab_id"] = json!("other-tab");
                w.snapshot["layouts"][0]["panes"] = json!([{"pane_id":"owner"}]);
                w.snapshot["layouts"].as_array_mut().unwrap().push(json!({"workspace_id":"w","tab_id":"other-tab","panes":[{"pane_id":"display"}]}));
            } else {
                w.snapshot["panes"][1]["terminal_id"] = json!("reused-terminal");
            }
        }
        let before = f.record();
        assert!(f.engine().open_display(OpenMode::Explicit).is_err());
        assert_eq!(f.record(), before);
        assert!(f.effects().is_empty());
    }
}

#[test]
fn close_writes_closed_first_and_ambiguous_ack_preserves_markers_and_other_pane() {
    for ambiguous in [false, true] {
        let f = Fixture::new();
        let mut engine = f.engine();
        engine.open_display(OpenMode::Explicit).unwrap();
        let markers = f.runner.world.lock().unwrap().snapshot["panes"][0]["tokens"].clone();
        {
            let mut w = f.runner.world.lock().unwrap();
            w.ambiguous_close = ambiguous;
            let mut other = pane("unrelated", "unrelated-terminal");
            other["tokens"] = json!({"private":"leave untouched"});
            w.snapshot["panes"].as_array_mut().unwrap().push(other);
            w.snapshot["layouts"][0]["panes"]
                .as_array_mut()
                .unwrap()
                .push(json!({"pane_id":"unrelated"}));
        }
        assert_eq!(engine.close_display().is_err(), ambiguous);
        assert!(matches!(f.record().phase(), NativePhase::Closed(_)));
        let w = f.runner.world.lock().unwrap();
        assert_eq!(w.snapshot["panes"][0]["tokens"], markers);
        assert_eq!(w.snapshot["panes"][1]["pane_id"], "unrelated");
        assert_eq!(
            w.snapshot["panes"][1]["tokens"],
            json!({"private":"leave untouched"})
        );
    }
}

#[test]
fn changed_owned_tuple_is_never_closed_or_replaced() {
    let f = Fixture::new();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    f.runner.world.lock().unwrap().snapshot["panes"][1]["terminal_id"] = json!("reused-terminal");
    let effects = f.effects();
    assert!(engine.close_display().is_err());
    assert!(engine.open_display(OpenMode::Explicit).is_err());
    assert_eq!(f.effects(), effects);
    assert!(matches!(f.record().phase(), NativePhase::Ready(_)));
}

#[test]
fn caller_terminal_workspace_or_tab_change_never_effects() {
    for field in ["terminal_id", "workspace_id", "tab_id"] {
        let f = Fixture::new();
        let mut engine = f.engine();
        engine.observe_owner().unwrap();
        {
            let mut w = f.runner.world.lock().unwrap();
            w.snapshot["panes"][0][field] = json!("moved");
            if field == "workspace_id" {
                w.snapshot["workspaces"][0][field] = json!("moved");
                w.snapshot["tabs"][0][field] = json!("moved");
                w.snapshot["layouts"][0][field] = json!("moved");
            } else if field == "tab_id" {
                w.snapshot["tabs"][0][field] = json!("moved");
                w.snapshot["layouts"][0][field] = json!("moved");
            }
        }
        assert!(engine.open_display(OpenMode::Explicit).is_err());
        assert!(f.effects().is_empty());
    }
}

#[test]
fn forged_card_nonce_or_owner_terminal_cannot_focus() {
    for field in ["STREAMS_CARDS_NONCE", "STREAMS_CARDS_OWNER_TERMINAL"] {
        let f = Fixture::new();
        f.engine().open_display(OpenMode::Explicit).unwrap();
        let mut environment = f.launched_environment();
        environment.insert(field.into(), "forged".into());
        let mut engine = f.engine_with(environment);
        let effects = f.effects();
        assert!(engine.verify_display().is_err());
        assert!(engine.focus_display().is_err());
        assert_eq!(f.effects(), effects);
    }
}

#[test]
fn manual_startup_projects_without_requiring_or_creating_a_display() {
    let f = Fixture::new();
    let mut engine = f.engine();
    engine.project_summaries().unwrap();
    let before = f.effects().len();
    engine.project_caption().unwrap();
    assert_eq!(before, f.effects().len());
    assert_eq!(f.launches(), 0);
    assert!(engine.observe_owner().unwrap().context.binding.is_some());
}
#[test]
fn summary_projection_only_publishes_bounded_hints_and_is_idempotent() {
    for hint in [
        "界".repeat(80),
        "x".repeat(81),
        " padded ".into(),
        "line\nbreak".into(),
    ] {
        let f = Fixture::new();
        f.runner.world.lock().unwrap().observation.card = card("native", Some(&hint));
        let mut engine = f.engine();
        engine.project_summaries().unwrap();
        let effects = f.effects();
        engine.project_summaries().unwrap();
        assert_eq!(f.effects(), effects);
        let w = f.runner.world.lock().unwrap();
        let expected = if hint.chars().count() <= 80
            && hint.trim() == hint
            && !hint.chars().any(char::is_control)
        {
            json!(hint)
        } else {
            Value::Null
        };
        assert_eq!(
            w.snapshot["workspaces"][0]["tokens"]["strm_goal_short"],
            expected
        );
        assert_eq!(
            w.snapshot["panes"][0]["tokens"]["strm_state_short"],
            expected
        );
        assert_eq!(w.snapshot["panes"][0]["tokens"]["strm_status"], "active");
        let wire = serde_json::to_string(&effects).unwrap();
        for secret in [
            "private goal narrative",
            "private state narrative",
            "private remaining narrative",
        ] {
            assert!(!wire.contains(secret));
        }
    }
}

#[test]
fn document_launch_persists_pending_and_exact_editor_then_reuses_by_focus() {
    let f = Fixture::new();
    let target = f.document();
    let (key, evidence) = document_keys(&f.runner.socket, &target);
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    engine.open_document(&target).unwrap();
    let ready = f.runner.state.read(&evidence).unwrap().unwrap();
    assert!(matches!(ready.phase(), NativePhase::Ready(_)));
    assert_eq!(
        ready.resources().unwrap(),
        &Resources::new("editor-1", "editor-terminal-1", "editor-tab-1").unwrap()
    );
    {
        let w = f.runner.world.lock().unwrap();
        let editor = w.snapshot["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["pane_id"] == "editor-1")
            .unwrap();
        assert_eq!(
            editor["tokens"],
            json!({
                "strm_doc":key,"strm_editor":ready.nonce(),"strm_doc_pane":"editor-1",
                "strm_doc_terminal":"editor-terminal-1","strm_doc_tab":"editor-tab-1",
            })
        );
    }
    {
        let mut w = f.runner.world.lock().unwrap();
        assert_eq!(w.snapshot["tabs"][1]["label"], "docs/design.md");
        w.snapshot["tabs"][1]["label"] = json!("My document");
    }
    let effects = f.effects();
    engine.open_document(&target).unwrap();
    assert_eq!(
        f.runner.world.lock().unwrap().snapshot["tabs"][1]["label"],
        "docs/design.md"
    );
    assert_eq!(f.launches(), 2); // One display and one document, never a duplicate editor.
    let after = f.effects();
    assert_eq!(&after[..effects.len()], effects.as_slice());
    assert_eq!(
        &after[effects.len()..],
        &[
            (
                "tab.rename".into(),
                json!({"tab_id":"editor-tab-1","label":"docs/design.md"})
            ),
            ("tab.focus".into(), json!({"tab_id":"editor-tab-1"})),
        ]
    );
    assert_eq!(f.runner.state.read(&evidence).unwrap().unwrap(), ready);
    let effects = f.effects();
    engine.open_document(&target).unwrap();
    assert_eq!(
        &f.effects()[effects.len()..],
        &[("tab.focus".into(), json!({"tab_id":"editor-tab-1"}))]
    );
}

#[test]
fn disabled_tab_naming_leaves_new_and_reused_document_tabs_independent() {
    let mut f = Fixture::new();
    f.config.tab_naming = false;
    let target = f.document();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    engine.open_document(&target).unwrap();
    assert_eq!(
        f.runner.world.lock().unwrap().snapshot["tabs"][1]["label"],
        "2"
    );
    f.runner.world.lock().unwrap().snapshot["tabs"][1]["label"] = json!("My document");
    let effects = f.effects();
    engine.open_document(&target).unwrap();
    assert_eq!(
        f.runner.world.lock().unwrap().snapshot["tabs"][1]["label"],
        "My document"
    );
    assert_eq!(
        &f.effects()[effects.len()..],
        &[("tab.focus".into(), json!({"tab_id":"editor-tab-1"}))]
    );
    assert!(!f.effects().iter().any(|(method, _)| method == "tab.rename"));
}

#[test]
fn editor_left_by_reset_is_not_adopted_or_closed() {
    let f = Fixture::new();
    let target = f.document();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    engine.open_document(&target).unwrap();
    let (_, evidence) = document_keys(&f.runner.socket, &target);
    std::fs::remove_file(f.runner.state.directory().join(format!("{evidence}.json"))).unwrap();
    let identity = target.editor_identity().unwrap();
    let legacy_key = json_digest(&(
        &identity.catalog_source,
        &identity.host_id,
        &identity.checkout_id,
        &identity.file,
    ))
    .unwrap();
    let old = {
        let mut world = f.runner.world.lock().unwrap();
        let editor = world.snapshot["panes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|p| p["pane_id"] == "editor-1")
            .unwrap();
        editor["tokens"]["strm_doc"] = legacy_key.into();
        editor.clone()
    };
    let before = f.effects().len();
    engine.open_document(&target).unwrap();
    let world = f.runner.world.lock().unwrap();
    assert_eq!(
        world.snapshot["panes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["pane_id"] == "editor-1")
            .unwrap(),
        &old
    );
    assert!(
        world.effects[before..]
            .iter()
            .all(|(method, _)| method != "plugin.pane.close")
    );
    assert_eq!(
        world
            .effects
            .iter()
            .filter(|(method, p)| method == "plugin.pane.open" && p["entrypoint"] == "document")
            .count(),
        2
    );
}

#[test]
fn malformed_document_ack_retains_pending_and_fences_duplicate_creation() {
    let f = Fixture::new();
    let target = f.document();
    let (_, evidence) = document_keys(&f.runner.socket, &target);
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    f.runner.world.lock().unwrap().malformed_open = true;
    assert!(engine.open_document(&target).is_err());
    let pending = f.runner.state.read(&evidence).unwrap().unwrap();
    assert!(matches!(pending.phase(), NativePhase::Unlaunched));
    let effects = f.effects();
    assert!(engine.open_document(&target).is_err());
    assert!(f.engine().open_document(&target).is_err());
    assert_eq!(f.effects(), effects);
    assert_eq!(f.launches(), 2);
    assert_eq!(f.runner.state.read(&evidence).unwrap().unwrap(), pending);
}

#[test]
fn launch_carries_custom_plugin_root_editor_and_named_namespace() {
    let mut f = Fixture::new();
    f.environment.insert("EDITOR".into(), "nano -w".into());
    f.environment.insert("HERDR_SESSION".into(), "fake".into());
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let effects = f.effects();
    let (_, launch) = effects
        .iter()
        .find(|(method, _)| method == "plugin.pane.open")
        .unwrap();
    let env = &launch["env"];
    assert_eq!(
        env["STREAMS_CARDS_PLUGIN_ROOT"],
        f.config.plugin_root.to_str().unwrap()
    );
    assert_eq!(env["EDITOR"], "nano -w");
    let captured: Value =
        serde_json::from_str(env["STREAMS_CARDS_READ_ENV"].as_str().unwrap()).unwrap();
    assert_eq!(captured["EDITOR"], "nano -w");
    assert_eq!(captured["HERDR_SESSION"], "fake");
    let environment = serde_json::from_value(env.clone()).unwrap();
    let context = LaunchedCardContext::from_environment(&environment)
        .unwrap()
        .unwrap();
    assert_eq!(context.reader().environment["HERDR_SESSION"], "fake");
}
#[test]
fn fully_paired_reserved_card_completes_without_restamping() {
    let f = Fixture::new();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    let ready = f.record();
    let mut crash = serde_json::to_value(&ready).unwrap();
    crash["phase"] = json!("pending");
    std::fs::write(
        f.runner
            .state
            .directory()
            .join(format!("{}.json", ready.key())),
        serde_json::to_vec(&crash).unwrap(),
    )
    .unwrap();
    let effects = f.effects();
    engine.open_display(OpenMode::Explicit).unwrap();
    assert_eq!(f.record(), ready);
    assert_eq!(f.effects(), effects);
}
#[test]
fn exact_reserved_editor_promotes_and_restores_its_document_title_without_relaunch() {
    let f = Fixture::new();
    let target = f.document();
    let (_, key) = document_keys(&f.runner.socket, &target);
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    engine.open_document(&target).unwrap();
    let ready = f.runner.state.read(&key).unwrap().unwrap();
    // Simulate the crash boundary after exact editor markers but before Ready persisted.
    let mut crash = serde_json::to_value(&ready).unwrap();
    crash["phase"] = json!("pending");
    std::fs::write(
        f.runner.state.directory().join(format!("{key}.json")),
        serde_json::to_vec(&crash).unwrap(),
    )
    .unwrap();
    f.runner.world.lock().unwrap().snapshot["tabs"][1]["label"] = json!("Manual title");
    let effects = f.effects();
    engine.open_document(&target).unwrap();
    assert_eq!(f.runner.state.read(&key).unwrap().unwrap(), ready);
    assert_eq!(f.launches(), 2);
    assert_eq!(
        &f.effects()[effects.len()..],
        &[
            (
                "tab.rename".into(),
                json!({"tab_id":"editor-tab-1", "label":"docs/design.md"})
            ),
            ("tab.focus".into(), json!({"tab_id":"editor-tab-1"})),
        ]
    );
    assert_eq!(
        f.runner.world.lock().unwrap().snapshot["tabs"][1]["label"],
        "docs/design.md"
    );
}
#[test]
fn launched_context_requires_every_protected_field_and_never_becomes_an_owner() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    let valid = f.launched_environment();
    f.engine_with(valid.clone()).verify_display().unwrap();
    let effects = f.effects();
    for key in [
        "STREAMS_CARDS_SCOPE",
        "STREAMS_CARDS_OWNER",
        "STREAMS_CARDS_NONCE",
        "STREAMS_CARDS_OWNER_TERMINAL",
        "STREAMS_CARDS_READ_ENV",
        "STREAMS_CARDS_READ_CWD",
        "STREAMS_CARDS_STATE_DIR",
        "STREAMS_CARDS_PLUGIN_ROOT",
    ] {
        let mut missing = valid.clone();
        missing.remove(key);
        assert!(f.try_engine_with(missing).is_err(), "{key}");
    }
    for (key, value) in [
        ("HERDR_PANE_ID", "owner"),
        ("STREAMS_CARDS_SCOPE", "{}"),
        (
            "STREAMS_CARDS_READ_ENV",
            r#"{"TOKEN":"not a reader capability"}"#,
        ),
    ] {
        let mut forged = valid.clone();
        forged.insert(key.into(), value.into());
        assert!(f.try_engine_with(forged).is_err(), "{key}");
    }
    assert_eq!(f.effects(), effects);
}
#[test]
fn focus_and_close_do_not_even_request_predecessor_retirement() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    remove_card(&mut f.runner.world.lock().unwrap());
    f.runner.world.lock().unwrap().observation.card = card("next-native", None);
    let effects = f.effects();
    assert!(f.engine().focus_display().is_err());
    assert!(f.engine().close_display().is_err());
    assert_eq!(f.runner.world.lock().unwrap().inventory_reads, 0);
    assert_eq!(f.effects(), effects);
}
#[test]
fn status_and_caption_fail_closed_when_workspace_binding_disappears() {
    let f = Fixture::new();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    f.runner
        .world
        .lock()
        .unwrap()
        .clear_binding_after_caption_rename = true;
    assert!(engine.project_caption().is_err());
    assert!(engine.status().is_err());
    let w = f.runner.world.lock().unwrap();
    assert_eq!(w.snapshot["panes"][1]["label"], "Effort");
    assert!(
        w.snapshot["panes"][1]["tokens"]
            .get("strm_caption")
            .is_none()
    );
}
#[test]
fn enabled_names_restore_saved_titles_and_do_not_write_unchanged_names() {
    let f = Fixture::new();
    let mut engine = f.engine();
    engine.project_summaries().unwrap();
    assert_eq!(
        engine.project_names().unwrap(),
        NameReport {
            workspace: NameOutcome::Renamed,
            tab: NameOutcome::Renamed,
        }
    );
    let effects = f.effects();
    assert_eq!(
        engine.project_names().unwrap(),
        NameReport {
            workspace: NameOutcome::Unchanged,
            tab: NameOutcome::Unchanged,
        }
    );
    assert_eq!(f.effects(), effects);
    {
        let mut world = f.runner.world.lock().unwrap();
        world.snapshot["workspaces"][0]["label"] = json!("External workspace name");
        world.snapshot["tabs"][0]["label"] = json!("External tab name");
    }
    assert_eq!(
        engine.project_names().unwrap(),
        NameReport {
            workspace: NameOutcome::Renamed,
            tab: NameOutcome::Renamed,
        }
    );
    let world = f.runner.world.lock().unwrap();
    assert_eq!(world.snapshot["workspaces"][0]["label"], "Effort");
    assert_eq!(world.snapshot["tabs"][0]["label"], "Conversation");
}

#[test]
fn disabled_name_surfaces_are_not_renamed_and_tab_naming_requires_first_pane() {
    let mut f = Fixture::new();
    f.config.workspace_naming = false;
    f.config.tab_naming = false;
    let mut engine = f.engine();
    engine.project_summaries().unwrap();
    let effects = f.effects();
    assert_eq!(
        engine.project_names().unwrap(),
        NameReport {
            workspace: NameOutcome::Disabled,
            tab: NameOutcome::Disabled,
        }
    );
    assert_eq!(f.effects(), effects);

    let f = Fixture::new();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    f.runner.world.lock().unwrap().snapshot["layouts"][0]["panes"] =
        json!([{"pane_id":"display"},{"pane_id":"owner"}]);
    assert_eq!(engine.project_names().unwrap().tab, NameOutcome::NotFirst);
    assert!(!f.effects().iter().any(|(method, _)| method == "tab.rename"));
}

#[test]
fn caption_ignores_old_manual_tokens_and_restores_its_saved_title() {
    let f = Fixture::new();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    {
        let mut world = f.runner.world.lock().unwrap();
        world.snapshot["panes"][1]["label"] = json!("External caption");
        world.snapshot["panes"][1]["tokens"]["strm_caption_manual"] = json!("1");
    }
    let effects = f.effects().len();
    engine.project_caption().unwrap();
    assert_eq!(
        f.runner.world.lock().unwrap().snapshot["panes"][1]["label"],
        "Effort"
    );
    assert_eq!(
        &f.effects()[effects..],
        &[(
            "pane.rename".into(),
            json!({"pane_id":"display","label":"Effort"})
        )]
    );
}

#[test]
fn caption_publication_is_idempotent() {
    let f = Fixture::new();
    let mut engine = f.engine();
    engine.open_display(OpenMode::Explicit).unwrap();
    engine.project_caption().unwrap();
    let effects = f.effects();
    engine.project_caption().unwrap();
    assert_eq!(f.effects(), effects);
}
#[test]
fn document_engine_rechecks_symlinks_and_nonregular_files_without_launch() {
    use std::os::unix::fs::symlink;
    for kind in ["symlink", "directory", "fifo"] {
        let f = Fixture::new();
        let target = f.document();
        let file = target.editor_identity().unwrap().file;
        f.engine().open_display(OpenMode::Explicit).unwrap();
        std::fs::remove_file(&file).unwrap();
        match kind {
            "symlink" => {
                let outside = f._temp.path().join("outside.md");
                std::fs::write(&outside, "outside").unwrap();
                symlink(outside, &file).unwrap();
            }
            "directory" => std::fs::create_dir(&file).unwrap(),
            _ => {
                let path = std::ffi::CString::new(file.to_str().unwrap()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
        }
        let effects = f.effects();
        assert!(f.engine().open_document(&target).is_err(), "{kind}");
        assert_eq!(f.effects(), effects);
    }
}
#[test]
fn document_acknowledgement_survives_authority_loss() {
    let f = Fixture::new();
    let target = f.document();
    let (_, key) = document_keys(&f.runner.socket, &target);
    f.engine().open_display(OpenMode::Explicit).unwrap();
    f.runner.world.lock().unwrap().fail_after_open = true;
    assert!(f.engine().open_document(&target).is_err());
    let reserved = f.runner.state.read(&key).unwrap().unwrap();
    assert!(matches!(reserved.phase(), NativePhase::Reserved(_)));
    assert_eq!(reserved.resources().unwrap().pane_id(), "editor-1");
    f.runner.world.lock().unwrap().deny_read = false;
    assert!(f.engine().open_document(&target).is_err());
    assert_eq!(f.launches(), 2);
}
#[test]
fn predecessor_retirement_uses_private_conversation_evidence_not_a_catalog_roster() {
    let f = Fixture::new();
    f.prepare_predecessor();
    let old = f.record();
    let before = f.effects().len();
    let observed = f.engine().guard_recovering().unwrap();
    assert_eq!(
        observed.saved.card.scope().native_session_id,
        "11111111-1111-4111-8111-111111111111"
    );
    assert!(matches!(observed.pair().unwrap(), PairState::Empty));
    assert_eq!(f.record(), old);
    assert!(f.runner.state.read(&observed.key).unwrap().is_none());
    let effects = f.effects();
    assert_eq!(effects.len(), before + 1);
    assert_eq!(effects.last().unwrap().0, "pane.report_metadata");
}

#[test]
fn predecessor_retirement_rejects_another_catalogs_principal() {
    let f = Fixture::new();
    f.prepare_predecessor();
    f.runner.world.lock().unwrap().snapshot["panes"][0]["tokens"]["strm_agent"] =
        json!(conversation_key_for("another-catalog", "host", "pi", "native").unwrap());
    let before = f.effects();
    assert!(f.engine().guard_recovering().is_err());
    assert_eq!(f.effects(), before);
}

#[test]
fn predecessor_retirement_retains_old_and_current_pending_launches() {
    for predecessor in [false, true] {
        let f = Fixture::new();
        f.prepare_predecessor();
        let old = f.record();
        let key = if predecessor {
            old.key().into()
        } else {
            conversation_key(f.runner.world.lock().unwrap().observation.card.scope()).unwrap()
        };
        let pending = NativeAttempt::new(
            key,
            if predecessor {
                old.nonce()
            } else {
                "current-pending"
            },
            &f.runner.socket,
            NativePurpose::Card,
        )
        .unwrap();
        // An interrupted-launch fixture, not a permitted rollback transition.
        crate::file_io::write_json(&f.runner.state.path(pending.key()).unwrap(), &pending).unwrap();
        let before = f.effects();
        assert!(f.engine().guard_recovering().is_err());
        assert_eq!(f.effects(), before);
        assert_eq!(f.runner.state.read(pending.key()).unwrap(), Some(pending));
    }
}

#[test]
fn predecessor_without_fresh_creation_context_has_no_native_effects() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    // A fresh engine represents the next attested conversation in the same owner.
    f.runner.world.lock().unwrap().observation.card = card("next-native", None);
    let effects = f.effects();
    assert!(f.engine().open_display(OpenMode::Explicit).is_err());
    assert_eq!(f.effects(), effects);
}
