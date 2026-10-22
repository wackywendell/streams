#![cfg(test)]

use super::*;
use crate::model::{Document, Repository, Session, Stream};
use crate::process::{ProcessError, ProcessOutput, ProcessRequest, ProcessRunner};
use crate::tui::{
    CardView, CheckoutBinding, CheckoutSelection, DisplayContext, DocumentLocation,
    NativeOwnerObservation, Rect, ResolvedDocument,
};
use serde_json::json;
use std::os::unix::process::ExitStatusExt;
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

struct World {
    snapshot: Value,
    current: String,
    observation: CardObservation,
    effects: Vec<Vec<String>>,
    calls: Vec<Vec<String>>,
    malformed_open: bool,
    ambiguous_close: bool,
    fail_after_open: bool,
    deny_read: bool,
    inventory_reads: usize,
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
        if inventory {
            world.inventory_reads += 1;
        }
        if world.deny_read || inventory {
            return Err(invalid(
                "fake application authority/creation context unavailable",
            ));
        }
        Ok(world.observation.clone())
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
impl ProcessRunner for FakeHerdr {
    fn run(&self, request: &ProcessRequest) -> std::result::Result<ProcessOutput, ProcessError> {
        assert_eq!(request.program, Path::new("/never/execute/herdr"));
        assert!(request.input.is_none());
        let args: Vec<String> = request
            .args
            .iter()
            .map(|a| a.to_str().unwrap().into())
            .collect();
        let a: Vec<&str> = args.iter().map(String::as_str).collect();
        let mut w = self.world.lock().unwrap();
        w.calls.push(args.clone());
        let result = match a.as_slice() {
            ["status", "--json"] => json!({"server":{"running":true,"compatible":true,
                "endpoint_compatible":true,"version":"0.9.5","socket":self.socket,"session":"fake"}}),
            ["pane", "current", "--current"] => {
                let current = w.snapshot["panes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| p["pane_id"] == w.current)
                    .unwrap();
                json!({"result":{"type":"pane_current","pane":current}})
            }
            ["api", "snapshot"] => {
                json!({"result":{"type":"session_snapshot","snapshot":w.snapshot}})
            }
            ["plugin", "list", "--plugin", PLUGIN_ID, "--json"] => {
                json!({"result":{"type":"plugin_list","plugins":[self.plugin]}})
            }
            ["plugin", "pane", "open", ..] => {
                let entrypoint = a.windows(2).find(|p| p[0] == "--entrypoint").unwrap()[1];
                assert!(a.contains(&"--no-focus"));
                let evidence_key = if entrypoint == "document" {
                    let target = w
                        .observation
                        .card
                        .document_target("lib", "docs/design.md")
                        .unwrap();
                    let ResolvedDocument::File { selection, file } = &target.location else {
                        panic!("not a file")
                    };
                    assert!(a.windows(2).any(|p| p == ["--workspace", "w"]));
                    assert!(a.windows(2).any(|p| p == ["--placement", "tab"]));
                    assert!(!a.contains(&"--target-pane"));
                    for value in [
                        "WORK_CARDS_DOCUMENT_COMMAND=less".into(),
                        format!("WORK_CARDS_DOCUMENT_CWD={}", selection.root.display()),
                        format!("WORK_CARDS_DOCUMENT_PATH={}", file.display()),
                    ] {
                        assert!(a.windows(2).any(|p| p[0] == "--env" && p[1] == value));
                    }
                    document_keys(&self.socket, &target).1
                } else {
                    assert_eq!(entrypoint, "card");
                    assert!(a.windows(2).any(|p| p == ["--target-pane", "owner"]));
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
                    assert!(
                        a.contains(&format!("STREAMS_CARDS_NONCE={}", pending.nonce()).as_str())
                    );
                }
                let launch = w
                    .effects
                    .iter()
                    .filter(|command| {
                        command
                            .windows(2)
                            .any(|p| p[0] == "--entrypoint" && p[1] == entrypoint)
                    })
                    .count()
                    + 1;
                w.effects.push(args.clone());
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
                json!({"result":{"type":"plugin_pane_opened","plugin_pane":{
                    "plugin_id":PLUGIN_ID,"entrypoint":entrypoint,"pane":created}}})
            }
            [kind @ ("tab" | "pane"), "rename", id, label] => {
                w.effects.push(args.clone());
                let (collection, key, result) = if *kind == "tab" {
                    ("tabs", "tab_id", "tab_info")
                } else {
                    ("panes", "pane_id", "pane_info")
                };
                let target = w.snapshot[collection]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|p| p[key] == *id)
                    .unwrap();
                target["label"] = json!(label);
                let target = target.clone();
                if *kind == "pane" && w.clear_binding_after_caption_rename {
                    w.snapshot["workspaces"][0]["tokens"] = json!({});
                }
                json!({"result":{"type":result,*kind:target}})
            }
            ["tab", "focus", id] => {
                let target = w
                    .observation
                    .card
                    .document_target("lib", "docs/design.md")
                    .unwrap();
                let (key, evidence) = document_keys(&self.socket, &target);
                let ready = self.state.read(&evidence).unwrap().unwrap();
                assert!(matches!(ready.phase(), NativePhase::Ready(_)));
                assert_eq!(ready.resources().unwrap().tab_id(), *id);
                let editor = w.snapshot["panes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| p["tab_id"] == *id)
                    .unwrap();
                assert_eq!(editor["tokens"]["strm_doc"], key);
                assert_eq!(editor["tokens"]["strm_editor"], ready.nonce());
                w.effects.push(args.clone());
                json!({"result":{"type":"ok"}})
            }
            [
                kind @ ("workspace" | "pane"),
                "report-metadata",
                id,
                "--source",
                PLUGIN_ID,
                rest @ ..,
            ] => {
                w.effects.push(args.clone());
                let collection = if *kind == "workspace" {
                    "workspaces"
                } else {
                    "panes"
                };
                let id_key = if *kind == "workspace" {
                    "workspace_id"
                } else {
                    "pane_id"
                };
                let target = w.snapshot[collection]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|p| p[id_key] == *id)
                    .expect("exact metadata target");
                let tokens = target["tokens"].as_object_mut().unwrap();
                for pair in rest.chunks_exact(2) {
                    match pair[0] {
                        "--token" => {
                            let (key, value) = pair[1].split_once('=').unwrap();
                            tokens.insert(key.into(), json!(value));
                        }
                        "--clear-token" => {
                            tokens.remove(pair[1]);
                        }
                        _ => panic!("unexpected metadata arguments: {args:?}"),
                    }
                }
                json!({"result":{"type":"ok"}})
            }
            [
                "pane",
                "resize",
                "--pane",
                id,
                "--direction",
                _,
                "--amount",
                _,
            ] => {
                assert!(matches!(
                    self.state.read(&self.key).unwrap().unwrap().phase(),
                    NativePhase::Ready(_)
                ));
                w.effects.push(args.clone());
                json!({"result":{"type":"pane_resize","resize":{"pane_id":id}}})
            }
            ["plugin", "pane", "close", "display"] => {
                assert!(
                    matches!(
                        self.state.read(&self.key).unwrap().unwrap().phase(),
                        NativePhase::Closed(_)
                    ),
                    "closed intent must precede native close"
                );
                w.effects.push(args.clone());
                remove_card(&mut w);
                json!({"result":{"type":"plugin_pane_closed","pane_id":
                    if w.ambiguous_close { "another-pane" } else { "display" }}})
            }
            _ => panic!("unsupported fake command (never executed): {args:?}"),
        };
        Ok(ProcessOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout: serde_json::to_vec(&result).unwrap(),
            stderr: vec![],
        })
    }
}
struct Fixture {
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
            ("HERDR_BIN_PATH".into(), "/never/execute/herdr".into()),
        ]);
        let config = NativeCardsConfig::load(&environment, &root, &plugin_root).unwrap();
        let state = Arc::new(NativeState::new(root.join("state")).unwrap());
        let socket = root.join("herdr.sock");
        let observation = CardObservation {
            card: card("native", None),
            handover: None,
        };
        let owner = NativeTuple {
            pane_id: "owner".into(),
            terminal_id: "owner-terminal".into(),
            workspace_id: "w".into(),
            tab_id: "t".into(),
        };
        let key = attempt_key(&socket, observation.card.scope(), &owner).unwrap();
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
            calls: vec![],
            malformed_open: false,
            ambiguous_close: false,
            fail_after_open: false,
            deny_read: false,
            inventory_reads: 0,
            clear_binding_after_caption_rename: false,
        }));
        let plugin = json!({"plugin_id":PLUGIN_ID,"enabled":true,"plugin_root":plugin_root,
        "manifest_path":plugin_root.join("streams-rust-herdr-plugin.toml"),
        "source":{"kind":"local"},"version":"0.1.0","min_herdr_version":"0.9.3",
        "platforms":["macos","linux"],"panes":[
            {"id":"card","title":"Streams work card","placement":"split","command":["/bin/sh","bin/open-rust-streams-pane.sh"]},
            {"id":"document","title":"Document","placement":"tab","command":["/bin/sh","bin/open-document.sh"]},
        ]});
        Self {
            _temp: temp,
            runner: Arc::new(FakeHerdr {
                world,
                state,
                socket,
                plugin,
                key,
            }),
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
            HerdrClient::connect(self.runner.clone(), environment.clone(), 1.0).unwrap(),
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
            .lock(&self.runner.socket, "w", 1.0, &cancelled)
            .unwrap()
            .write(old, next)
            .unwrap();
    }
    fn effects(&self) -> Vec<Vec<String>> {
        self.runner.world.lock().unwrap().effects.clone()
    }
    fn launches(&self) -> usize {
        self.effects()
            .iter()
            .filter(|a| a.starts_with(&argv(&["plugin", "pane", "open"])))
            .count()
    }
    fn bind(&self) {
        let mut w = self.runner.world.lock().unwrap();
        w.snapshot["workspaces"][0]["tokens"] = json!({
            "strm_catalog":json_digest(&"catalog").unwrap(),"strm_host":"host",
            "strm_stream":"effort","strm_binding":"binding-lifetime",
        });
    }
    fn reserve(&self, partial: bool) {
        self.bind();
        let pending = NativeAttempt::new(&self.runner.key, "reserved-nonce").unwrap();
        self.write(None, &pending);
        let mut attempt = pending.clone();
        attempt
            .reserve(Resources::new("display", "display-terminal", "t").unwrap())
            .unwrap();
        self.write(Some(&pending), &attempt);
        let mut w = self.runner.world.lock().unwrap();
        add_card(&mut w, "display", "display-terminal");
        if partial {
            let identity = json_digest(&ScopeWire::from(w.observation.card.scope())).unwrap();
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
    assert!(!f.runner.state.directory().exists());
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
        assert!(
            f.engine()
                .pin_owner(observed.card.scope(), &changed)
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
        assert!(f.engine().pin_owner(&changed, &observed.context).is_err());
    }
    assert!(f.effects().is_empty());
    assert!(!f.runner.state.directory().exists());
    engine.open_display(OpenMode::Explicit).unwrap();
    let current = f.engine().observe_owner().unwrap();
    assert!(current.context.binding.is_some());
    f.engine()
        .pin_owner(observed.card.scope(), &observed.context)
        .unwrap();
    f.engine()
        .pin_owner(current.card.scope(), &current.context)
        .unwrap();
    f.runner.world.lock().unwrap().snapshot["workspaces"][0]["tokens"]["strm_binding"] =
        json!("replacement");
    let before = f.effects().len();
    assert!(
        f.engine()
            .pin_owner(current.card.scope(), &current.context)
            .is_err()
    );
    assert_eq!(before, f.effects().len());
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
        f.runner.world.lock().unwrap().calls.clear();
        let mut engine = f.engine();
        // Close exercises an owned, ready card, not the no-op absence path.
        if operation == "close" {
            engine.open_display(OpenMode::Explicit).unwrap();
            f.runner.world.lock().unwrap().calls.clear();
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
                engine
                    .project_names(NameProjectionIntent::Automatic)
                    .unwrap();
                engine.project_caption().unwrap();
            }
            _ => unreachable!(),
        }
        let world = f.runner.world.lock().unwrap();
        let mut counts = BTreeMap::new();
        for call in &world.calls {
            *counts.entry(call[..2].join(" ")).or_insert(0_usize) += 1;
        }
        // Includes bootstrap except close, which starts with a ready engine.
        // Every effect invalidates the observation for its confirmation; taking
        // the workspace lock also invalidates the pre-lock display snapshot.
        let (snapshots, starts) = match operation {
            "observe" | "check" => (1, 3),
            "open" => (7, 27),
            "close" => (3, 8),
            "project" => (8, 23),
            _ => unreachable!(),
        };
        assert_eq!(counts["api snapshot"], snapshots, "{operation}: {counts:?}");
        assert_eq!(world.calls.len(), starts, "{operation}: {counts:?}");
    }
}

#[test]
fn bootstrap_and_verify_are_read_only_and_require_pair_and_durable_evidence() {
    let f = Fixture::new();
    let mut engine = f.engine();
    assert!(engine.verify_display().is_err());
    assert!(!f.runner.state.directory().exists());
    let lock = f.runner.socket.parent().unwrap().join(format!(
        ".work-cards-{}.lock",
        digest(format!("{}\0w", f.runner.socket.display()).as_bytes())
    ));
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
    let identity = json_digest(&ScopeWire::from(w.observation.card.scope())).unwrap();
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
    assert_eq!(&effects.last().unwrap()[..3], &["plugin", "pane", "open"]);
    let w = f.runner.world.lock().unwrap();
    assert_eq!(w.snapshot["panes"][0]["tokens"], json!({}));
    assert_eq!(w.snapshot["panes"][1]["tokens"], json!({}));
    drop(w);
    assert!(f.engine().open_display(OpenMode::Automatic).is_err());
    assert_eq!(f.effects(), effects);
}

#[test]
fn ready_and_closed_absent_resources_suppress_automatic_but_allow_explicit_reopen() {
    for close in [false, true] {
        let f = Fixture::new();
        let mut engine = f.engine();
        engine.open_display(OpenMode::Explicit).unwrap();
        if close {
            engine.close_display().unwrap();
        } else {
            remove_card(&mut f.runner.world.lock().unwrap());
        }
        let effects = f.effects();
        assert_eq!(
            engine.open_display(OpenMode::Automatic).unwrap(),
            DisplayOutcome::Suppressed
        );
        assert_eq!(f.effects(), effects);
        assert!(matches!(
            engine.open_display(OpenMode::Explicit).unwrap(),
            DisplayOutcome::Opened(_)
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
        "My document"
    );
    assert_eq!(f.launches(), 2); // One display and one document, never a duplicate editor.
    let after = f.effects();
    assert_eq!(&after[..effects.len()], effects.as_slice());
    assert_eq!(
        &after[effects.len()..],
        &[argv(&["tab", "focus", "editor-tab-1"])]
    );
    assert_eq!(f.runner.state.read(&evidence).unwrap().unwrap(), ready);
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
    let launch = effects
        .iter()
        .find(|args| args.starts_with(&argv(&["plugin", "pane", "open"])))
        .unwrap();
    assert!(launch.contains(&format!(
        "STREAMS_CARDS_PLUGIN_ROOT={}",
        f.config.plugin_root.display()
    )));
    assert!(launch.contains(&"EDITOR=nano -w".to_owned()));
    let reader = launch
        .iter()
        .find_map(|arg| arg.strip_prefix("STREAMS_CARDS_READ_ENV="))
        .unwrap();
    let captured: Value = serde_json::from_str(reader).unwrap();
    assert_eq!(captured["EDITOR"], "nano -w");
    assert_eq!(captured["HERDR_SESSION"], "fake");
    let environment = launch
        .windows(2)
        .filter(|pair| pair[0] == "--env")
        .map(|pair| {
            let (key, value) = pair[1].split_once('=').unwrap();
            (key.to_owned(), value.to_owned())
        })
        .collect();
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
fn exact_reserved_editor_promotes_without_relaunch_or_renaming() {
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
        &[argv(&["tab", "focus", "editor-tab-1"])]
    );
    assert_eq!(
        f.runner.world.lock().unwrap().snapshot["tabs"][1]["label"],
        "Manual title"
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
fn predecessor_without_fresh_creation_context_has_no_native_effects() {
    let f = Fixture::new();
    f.engine().open_display(OpenMode::Explicit).unwrap();
    // A fresh engine represents the next attested conversation in the same owner.
    f.runner.world.lock().unwrap().observation.card = card("next-native", None);
    let effects = f.effects();
    assert!(f.engine().open_display(OpenMode::Explicit).is_err());
    assert_eq!(f.effects(), effects);
}
