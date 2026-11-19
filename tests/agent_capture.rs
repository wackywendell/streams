//! Diagnostic capture reads addressed conversations; only its explicit chooser inventories.
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use streams::{
    agent::{CaptureTarget, capture_inputs},
    application::{self, OpenedRecords},
    config::{Config, Strategy},
    file_store::records::{RecordIoEvent, relative_record_path},
    frontend::herdr::HerdrNativeSnapshot,
    model::{Record, Session, Stream, TitleDefaults},
    runtime::{
        BindingKey, CheckoutRegistry, DirectoryBindings, conversations::ConversationRoots,
        native_markers::stream_tokens, state::RuntimeState,
    },
    storage::records::RecordAddress,
};

struct Fixture {
    temp: tempfile::TempDir,
    config: Config,
    opened: OpenedRecords,
    state: RuntimeState,
    roots: ConversationRoots,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().canonicalize().unwrap();
        let config = Config::new(
            &BTreeMap::from([("HOME".into(), home.to_str().unwrap().into())]),
            &home,
        )
        .unwrap();
        let opened =
            application::initialize_records(&config, Strategy::None, None, "main").unwrap();
        let state = RuntimeState::new(home.join("runtime"), opened.host().as_str()).unwrap();
        for id in ["effort", "other"] {
            application::create_record(
                opened.records(),
                &Record::Stream(
                    Stream::new(id, format!("Title {id}"), None, opened.host().as_str()).unwrap(),
                ),
            )
            .unwrap();
        }
        let roots = ConversationRoots {
            pi: vec![home.join("poisoned-transcript-root")],
            codex: vec![],
            claude: vec![],
        };
        fs::write(&roots.pi[0], "transcript discovery must not run").unwrap();
        Self {
            temp,
            config,
            opened,
            state,
            roots,
        }
    }
    fn session(&self, native: &str, stream: &str) -> Session {
        let session = Session::new(
            Session::id_for(self.opened.host().as_str(), "pi", native),
            self.opened.host().as_str(),
            stream,
            "pi",
            native,
            format!("Lane {native}"),
        )
        .unwrap();
        application::create_record(self.opened.records(), &Record::Session(session.clone()))
            .unwrap();
        session
    }
    fn transcript(&self, native: &str) -> PathBuf {
        let path = self.temp.path().join(format!("{native}.jsonl"));
        fs::write(
            &path,
            format!("{}\n", json!({"type":"session","id":native})),
        )
        .unwrap();
        path.canonicalize().unwrap()
    }
    fn snapshot(&self, agents: Vec<(&str, Value)>, painted: bool) -> HerdrNativeSnapshot {
        let agents: Vec<_> = agents
            .into_iter()
            .enumerate()
            .map(|(index, (workspace, mut native))| {
                native["workspace_id"] = json!(workspace);
                native["tab_id"] = json!(format!("tab-{workspace}"));
                native["pane_id"] = json!(format!("pane-{index}"));
                native["terminal_id"] = json!(format!("terminal-{index}"));
                native
            })
            .collect();
        let mut raw = json!({
            "workspaces":[{"workspace_id":"workspace","label":"Working","number":1},{"workspace_id":"unrelated","label":"Other","number":2}],
            "tabs":[{"workspace_id":"workspace","tab_id":"tab-workspace","label":"Working","number":1},{"workspace_id":"unrelated","tab_id":"tab-unrelated","label":"Other","number":1}],
            "panes":agents,"agents":agents,
            "layouts":(["workspace","unrelated"].iter().map(|workspace| json!({"workspace_id":workspace,"tab_id":format!("tab-{workspace}"),"panes":agents.iter().filter(|agent|agent["workspace_id"] == *workspace).map(|agent|json!({"pane_id":agent["pane_id"]})).collect::<Vec<_>>()})).collect::<Vec<_>>())
        });
        if painted {
            raw["workspaces"][0]["tokens"] = json!(stream_tokens(
                self.opened.source_id(),
                self.opened.host().as_str(),
                "effort"
            ));
        }
        HerdrNativeSnapshot::parse(&raw).unwrap()
    }
    fn capture(&self, snapshot: &HerdrNativeSnapshot, target: CaptureTarget<'_>) -> Value {
        capture_inputs(
            &self.opened,
            Ok(snapshot),
            target,
            TitleDefaults::default(),
            &self.locations(),
            &self.roots,
            |_, _| json!({"checkouts":[],"document_locations":[]}),
        )
        .unwrap()
    }
    fn locations(&self) -> DirectoryBindings {
        DirectoryBindings::new(
            self.state.root(),
            CheckoutRegistry::new(
                self.config.checkout_state_dir(),
                self.opened.host().as_str(),
            )
            .unwrap(),
        )
    }
    fn retain(&self, session: &Session, locator: &std::path::Path) {
        let locations = self.locations();
        locations
            .remember(
                BindingKey::Session(&session.id),
                self.temp.path(),
                false,
                None,
                None,
            )
            .unwrap();
        locations.remember_transcript(&session.id, locator).unwrap();
    }
}
fn native(kind: &str, value: Value) -> Value {
    json!({"agent":"pi","agent_session":{"agent":"pi","kind":kind,"value":value}})
}
fn sample<'a>(value: &'a Value, id: &str) -> &'a Value {
    value["samples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sample| sample["native_session_id"] == id)
        .unwrap()
}

#[test]
fn exact_capture_reads_only_its_target_and_observed_native_keys_at_zero_or_64_unrelated_records() {
    for unrelated in [0, 64] {
        let f = Fixture::new();
        let session = f.session("owner", "effort");
        f.session("not-observed", "effort");
        let transcript = f.transcript("owner");
        let snapshot = f.snapshot(
            vec![
                ("workspace", native("path", json!(transcript))),
                (
                    "unrelated",
                    native("path", json!("/never-open-unrelated.jsonl")),
                ),
            ],
            true,
        );
        for index in 0..unrelated {
            let path = f
                .opened
                .root()
                .join(format!("hosts/foreign-{index}/conversations/bad.json"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "malformed unrelated record").unwrap();
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let opened = application::open_records_observed(&f.config, move |event| {
            sink.lock().unwrap().push(event)
        })
        .unwrap();
        events.lock().unwrap().clear();
        let result = capture_inputs(
            &opened,
            Ok(&snapshot),
            CaptureTarget::Stream("effort"),
            TitleDefaults::default(),
            &f.locations(),
            &f.roots,
            |_, _| json!({"checkouts":[],"document_locations":[]}),
        )
        .unwrap();
        assert_eq!(result["samples"].as_array().unwrap().len(), 1);
        assert_eq!(
            sample(&result, "owner")["catalog"]["session"]["id"],
            session.id
        );
        assert!(
            sample(&result, "owner")["catalog"]
                .get("authority")
                .is_none()
        );
        assert_eq!(
            sample(&result, "owner")["catalog"]["title_defaults"],
            json!(TitleDefaults::default())
        );
        assert_eq!(
            sample(&result, "owner")["transcript_file"],
            json!(transcript)
        );
        assert_eq!(sample(&result, "owner")["open"], true);
        let reads: Vec<_> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                RecordIoEvent::Read(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            reads,
            vec![
                opened.root().join(".catalog-local/records.json"),
                opened.root().join(
                    relative_record_path(
                        &RecordAddress::new(
                            opened.host().as_str(),
                            streams::model::RecordKind::Stream,
                            "effort"
                        )
                        .unwrap()
                    )
                    .unwrap()
                ),
                opened.root().join(".catalog-local/records.json"),
                opened.root().join(
                    relative_record_path(&RecordAddress::for_record(&Record::Session(session)))
                        .unwrap()
                )
            ]
        );
        assert!(
            events.lock().unwrap().iter().all(|event| !matches!(
                event,
                RecordIoEvent::List(_) | RecordIoEvent::Written(_, _)
            ))
        );
    }
}

#[test]
fn unbound_workspace_samples_exact_native_keys_and_never_attaches_them() {
    let f = Fixture::new();
    let snapshot = f.snapshot(
        vec![("workspace", native("id", json!("unenrolled")))],
        false,
    );
    let result = f.capture(&snapshot, CaptureTarget::Workspace("workspace"));
    let lane = sample(&result, "unenrolled");
    assert!(lane["catalog"]["session"].is_null());
    assert!(lane["catalog"]["stream"].is_null());
    assert!(lane["transcript_file"].is_null());
    assert!(
        lane["unavailable"]
            .as_str()
            .unwrap()
            .contains("no retained exact transcript locator")
    );
    assert!(!f.state.root().exists());
}

#[test]
fn actual_session_membership_wins_over_workspace_markers() {
    let f = Fixture::new();
    let session = f.session("moved", "other");
    let snapshot = f.snapshot(vec![("workspace", native("id", json!("moved")))], true);
    let result = f.capture(&snapshot, CaptureTarget::Workspace("workspace"));
    assert_eq!(sample(&result, "moved")["catalog"]["stream"]["id"], "other");
    assert_eq!(
        sample(&result, "moved")["catalog"]["session"]["id"],
        session.id
    );
    assert_eq!(result["streams"][0]["id"], "effort");
}

#[test]
fn observed_id_uses_its_exact_retained_locator_without_discovering_closed_sessions() {
    let f = Fixture::new();
    let session = f.session("observed", "effort");
    let closed = f.session("not-observed", "effort");
    let locator = f.transcript("observed");
    f.retain(&session, &locator);
    // This unrelated saved input must not be opened or used to infer liveness.
    fs::write(
        f.locations().path(BindingKey::Session(&closed.id)),
        "unrelated malformed binding",
    )
    .unwrap();
    let snapshot = f.snapshot(vec![("workspace", native("id", json!("observed")))], true);
    let result = f.capture(&snapshot, CaptureTarget::Stream("effort"));
    assert_eq!(result["samples"].as_array().unwrap().len(), 1);
    assert_eq!(sample(&result, "observed")["open"], true);
    assert_eq!(
        sample(&result, "observed")["transcript_file"],
        json!(locator)
    );
    let result = f.capture(&f.snapshot(vec![], true), CaptureTarget::Stream("effort"));
    assert_eq!(result["samples"], json!([]));
    let elsewhere = f.snapshot(
        vec![(
            "unrelated",
            native("path", json!("/not-a-discovery-input.jsonl")),
        )],
        true,
    );
    let result = f.capture(&elsewhere, CaptureTarget::Stream("effort"));
    assert_eq!(result["samples"], json!([]));
    assert!(result["topology_error"].is_null());
    fs::write(&locator, "{\"type\":\"session\",\"id\":\"different\"}\n").unwrap();
    let result = f.capture(&snapshot, CaptureTarget::Stream("effort"));
    assert!(sample(&result, "observed")["transcript_file"].is_null());
    assert!(sample(&result, "observed")["unavailable"].is_string());
}

#[test]
fn bad_or_duplicate_native_rows_do_not_hide_other_verified_conversations() {
    for ambiguous in [false, true] {
        let f = Fixture::new();
        let good = f.transcript("good");
        let mut agents = vec![
            ("workspace", native("path", json!(good))),
            ("workspace", native("id", json!("other"))),
        ];
        if ambiguous {
            agents.push(("workspace", native("id", json!("other"))));
        } else {
            agents.push((
                "workspace",
                native("path", json!("/unreadable-concerned.jsonl")),
            ));
        }
        let result = f.capture(&f.snapshot(agents, true), CaptureTarget::Stream("effort"));
        assert_eq!(sample(&result, "good")["transcript_file"], json!(good));
        assert!(!result["unresolved"].as_array().unwrap().is_empty());
        if ambiguous {
            assert!(
                result["samples"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|sample| sample["native_session_id"] != "other")
            );
        }
    }
}

#[test]
fn diagnostic_inventory_is_explicit_and_targets_do_not_use_title_resolution_or_legacy_state() {
    let f = Fixture::new();
    let snapshot = f.snapshot(vec![], false);
    let result = f.capture(&snapshot, CaptureTarget::Inventory);
    assert_eq!(result["streams"].as_array().unwrap().len(), 2);
    assert!(result.get("samples").is_none());
    for target in [
        CaptureTarget::Stream("Title effort"),
        CaptureTarget::Stream("eff"),
        CaptureTarget::Workspace("unobserved"),
    ] {
        assert!(
            capture_inputs(
                &f.opened,
                Ok(&snapshot),
                target,
                TitleDefaults::default(),
                &f.locations(),
                &f.roots,
                |_, _| json!({"checkouts":[],"document_locations":[]}),
            )
            .is_err()
        );
    }
    let unavailable = capture_inputs(
        &f.opened,
        Err("native unavailable".into()),
        CaptureTarget::Stream("effort"),
        TitleDefaults::default(),
        &f.locations(),
        &f.roots,
        |_, _| json!({"checkouts":[],"document_locations":[]}),
    )
    .unwrap();
    assert_eq!(unavailable["samples"], json!([]));
    assert_eq!(unavailable["topology_error"], "native unavailable");
    assert!(!f.state.root().exists());
}

#[test]
fn capture_calls_shared_hints_once_per_typed_sample_and_freezes_every_payload() {
    use std::cell::RefCell;
    let f = Fixture::new();
    let first = f.session("first", "effort");
    let second = f.session("second", "other");
    let snapshot = f.snapshot(
        vec![
            ("workspace", native("id", json!("first"))),
            ("workspace", native("id", json!("second"))),
            ("workspace", native("id", json!("missing"))),
        ],
        true,
    );
    let successful = json!({"checkouts":[{"repository_url":"repository","checkout_id":"checkout","root":"/work"}],
        "document_locations":[{"repository_url":"repository","path":"plan.md","checkout_id":"checkout","root":"/work","kind":"local","file":"/work/plan.md","error":null}]});
    let unavailable = json!({"checkouts":[],"document_locations":[{"repository_url":"other","path":"missing.md","checkout_id":null,"root":null,"kind":null,"file":null,"error":"no checkout"}]});
    let calls = RefCell::new(Vec::new());
    let result = capture_inputs(
        &f.opened,
        Ok(&snapshot),
        CaptureTarget::Workspace("workspace"),
        TitleDefaults::default(),
        &f.locations(),
        &f.roots,
        |stream, session| {
            calls
                .borrow_mut()
                .push((stream.id.clone(), session.id.clone()));
            if session.id == first.id {
                successful.clone()
            } else {
                unavailable.clone()
            }
        },
    )
    .unwrap();
    assert_eq!(
        *calls.borrow(),
        vec![("effort".into(), first.id), ("other".into(), second.id)]
    );
    for (id, expected) in [("first", &successful), ("second", &unavailable)] {
        for field in ["checkouts", "document_locations"] {
            assert_eq!(sample(&result, id)["catalog"][field], expected[field]);
        }
    }
    for field in ["checkouts", "document_locations"] {
        assert_eq!(sample(&result, "missing")["catalog"][field], json!([]));
    }
    let inventory = capture_inputs(
        &f.opened,
        Ok(&snapshot),
        CaptureTarget::Inventory,
        TitleDefaults::default(),
        &f.locations(),
        &f.roots,
        |_, _| panic!("inventory must not acquire hints"),
    )
    .unwrap();
    assert!(inventory.get("samples").is_none());
}
