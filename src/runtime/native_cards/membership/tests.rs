use super::*;
use crate::{
    fake_herdr,
    model::{Session, TitleDefaults},
    operations::membership::apply_move,
    runtime::{conversations::IDENTITY_FILE_OPENS, state::herdr_scope},
};
use serde_json::json;

struct Fixture {
    directory: tempfile::TempDir,
    _server: fake_herdr::FakeHerdr,
    client: HerdrClient,
    snapshot: HerdrNativeSnapshot,
    runtime: RuntimeState,
    transcript: PathBuf,
}

impl Fixture {
    fn new(tokens: Value) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("native-membership-")
            .tempdir_in("/tmp")
            .unwrap();
        let server =
            fake_herdr::FakeHerdr::start(&directory.path().join("herdr.sock"), |method, _| {
                panic!("unexpected native request: {method}");
            });
        let client = HerdrClient::connect(
            BTreeMap::from([
                ("HERDR_ENV".into(), "1".into()),
                ("HERDR_PANE_ID".into(), "owner".into()),
                ("HERDR_SESSION".into(), "test".into()),
                (
                    "HERDR_SOCKET_PATH".into(),
                    server.socket.to_str().unwrap().into(),
                ),
            ]),
            1.0,
        )
        .unwrap();
        let transcript = directory.path().join("owner.jsonl");
        std::fs::write(&transcript, "{\"type\":\"session\",\"id\":\"native\"}\n").unwrap();
        let panes = json!([
            {"pane_id":"owner","terminal_id":"owner-terminal","workspace_id":"w","tab_id":"t",
             "agent":"pi","agent_session":{"agent":"pi","kind":"path","value":transcript}},
            {"pane_id":"neighbor","terminal_id":"neighbor-terminal","workspace_id":"w","tab_id":"t",
             "agent":"pi","agent_session":{"agent":"pi","kind":"path","value":directory.path().join("unreadable-neighbor")}}
        ]);
        let snapshot = HerdrNativeSnapshot::parse(&json!({
            "workspaces":[{"workspace_id":"w","label":"Destination","number":1,"tokens":tokens}],
            "tabs":[{"workspace_id":"w","tab_id":"t","label":"Work","number":1}],
            "panes":panes,"agents":panes,
            "layouts":[{"workspace_id":"w","tab_id":"t","panes":[{"pane_id":"owner"},{"pane_id":"neighbor"}]}]
        })).unwrap();
        let runtime = RuntimeState::new(directory.path().join("runtime"), "host").unwrap();
        Self {
            directory,
            _server: server,
            client,
            snapshot,
            runtime,
            transcript,
        }
    }

    fn caller(&self) -> PiCallerObservation {
        PiCallerObservation::from_snapshot(&self.client, self.snapshot.clone(), "native").unwrap()
    }

    fn row(&self, stream: &str, workspace: &str) -> Value {
        json!({"version":2,"stream_id":stream,
            "scope":herdr_scope(&self._server.socket, Some("test")),
            "binding":{"frontend":"herdr","workspace_id":workspace,"nonce":"binding"},
            "pending":null})
    }

    fn put(&self, stream: &str, value: &Value) {
        let path = self.runtime.placement_path(stream).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }
}

fn records(stream: &str) -> RecordContext {
    RecordContext {
        catalog_source: "catalog".into(),
        host_id: "host".into(),
        title_defaults: TitleDefaults::default(),
        stream: Some(Stream::new(stream, "Work", None, "host").unwrap()),
        session: Some(
            Session::new(
                Session::id_for("host", "pi", "native"),
                "host",
                stream,
                "pi",
                "native",
                "Conversation",
            )
            .unwrap(),
        ),
        sessions: vec![],
    }
}

fn binding() -> Value {
    json!({"strm_catalog":json_digest(&"catalog").unwrap(),"strm_host":"host",
        "strm_stream":"B","strm_binding":"binding"})
}

#[test]
fn ready_intent_reuses_caller_and_selected_placement_without_other_records_or_rereads() {
    let f = Fixture::new(binding());
    f.put("B", &f.row("B", "w"));
    f.put(
        "A",
        &json!({"corrupt":"current placement must not be read"}),
    );
    f.put("unrelated", &json!({"corrupt":true}));
    let records = records("A");
    IDENTITY_FILE_OPENS.with(|files| files.borrow_mut().clear());
    let caller = f.caller();
    let observed = NativeMembershipObservation::observe(&records, &f.runtime, &caller).unwrap();
    assert!(std::ptr::eq(observed.caller(), &caller));
    std::fs::rename(&f.transcript, f.directory.path().join("old-transcript")).unwrap();
    std::fs::rename(f.runtime.root(), f.directory.path().join("old-runtime")).unwrap();
    for _ in 0..2 {
        let NativeMembershipPlan::Ready(intent) = observed.plan().unwrap() else {
            panic!("ready")
        };
        assert_eq!(intent.host_id(), "host");
        assert_eq!(intent.session_id(), Session::id_for("host", "pi", "native"));
        assert_eq!(intent.destination_stream_id(), "B");
        let source = records.stream.as_ref().unwrap();
        let destination = Stream::new("B", "Destination", None, "host").unwrap();
        let moved = apply_move(
            records.session.as_ref().unwrap(),
            source,
            &destination,
            &intent,
        )
        .unwrap()
        .unwrap();
        assert_eq!(moved.stream_id, "B");
        assert_eq!(moved.previous_stream_ids, ["A"]);
        let foreign = Stream::new("B", "Foreign", None, "another-host").unwrap();
        assert!(apply_move(records.session.as_ref().unwrap(), source, &foreign, &intent).is_err());
        let accepted = RecordContext {
            session: Some(moved),
            stream: Some(destination),
            ..records.clone()
        };
        assert_eq!(observed.admit_context(&accepted).unwrap().stream_id, "B");
        assert!(observed.admit_context(&records).is_err());
    }
    IDENTITY_FILE_OPENS.with(|files| assert_eq!(files.borrow().len(), 1));
}

#[test]
fn agreeing_missing_metadata_requires_projection_never_a_move_or_new_stream() {
    let expected = binding().as_object().unwrap().clone();
    for mask in 0..16 {
        let partial = expected
            .iter()
            .enumerate()
            .filter(|(index, _)| mask & (1 << index) != 0)
            .map(|(_, (key, value))| (key.clone(), value.clone()))
            .collect();
        let f = Fixture::new(Value::Object(partial));
        f.put("B", &f.row("B", "w"));
        let records = records("B");
        let caller = f.caller();
        let observed = NativeMembershipObservation::observe(&records, &f.runtime, &caller).unwrap();
        let plan = observed.plan().unwrap();
        if mask == 15 {
            assert!(matches!(plan, NativeMembershipPlan::Ready(_)));
        } else {
            let NativeMembershipPlan::NeedsProjection {
                stream_id,
                workspace_id,
                missing,
            } = plan
            else {
                panic!("missing agreeing paint must not be ready or unassociated: {mask}");
            };
            assert_eq!(stream_id, "B");
            assert_eq!(workspace_id, "w");
            let expected_missing: Tokens = expected
                .iter()
                .enumerate()
                .filter(|(index, _)| mask & (1 << index) == 0)
                .map(|(_, (key, value))| (key.clone(), value.as_str().unwrap().to_owned()))
                .collect();
            assert_eq!(missing, expected_missing);
            assert!(observed.admit_context(&records).is_err());
        }
    }
}

#[test]
fn conflicting_or_orphan_binding_is_not_adopted_including_another_nonce() {
    for key in ["strm_catalog", "strm_host", "strm_stream", "strm_binding"] {
        for value in ["foreign", ""] {
            let mut tokens = binding();
            tokens[key] = json!(value);
            let f = Fixture::new(tokens);
            f.put("B", &f.row("B", "w"));
            let records = records("B");
            let caller = f.caller();
            assert!(
                NativeMembershipObservation::observe(&records, &f.runtime, &caller)
                    .and_then(|observed| observed.plan())
                    .is_err(),
                "{key}={value}"
            );
        }
    }
    for tokens in [
        binding(),
        json!({"strm_host":"host"}),
        json!({"strm_binding":"binding"}),
    ] {
        let f = Fixture::new(tokens);
        let caller = f.caller();
        let records = records("A");
        assert!(
            NativeMembershipObservation::observe(&records, &f.runtime, &caller)
                .unwrap()
                .plan()
                .is_err()
        );
    }
}

#[test]
fn unassociated_is_not_a_hint_selected_placement_or_legacy_record_fallback() {
    let f = Fixture::new(json!({"strm_goal_short":"presentation only","other_plugin":"value"}));
    let caller = f.caller();
    let current = records("A");
    assert_eq!(
        NativeMembershipObservation::observe(&current, &f.runtime, &caller)
            .unwrap()
            .plan()
            .unwrap(),
        NativeMembershipPlan::Unassociated
    );
    assert!(!f.runtime.root().exists());
    for collection in ["associations", "enrollments", "attempts", "arrangements"] {
        let path = f.runtime.root().join(collection).join("corrupt.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "unrelated legacy bytes must not be read").unwrap();
    }
    f.put("B", &json!({"corrupt":true}));
    let hinted = RecordContext {
        session: None,
        ..records("B")
    };
    assert_eq!(
        NativeMembershipObservation::observe(&hinted, &f.runtime, &caller)
            .unwrap()
            .plan()
            .unwrap(),
        NativeMembershipPlan::Unassociated
    );
    assert_eq!(
        NativeMembershipObservation::observe(&current, &f.runtime, &caller)
            .unwrap()
            .plan()
            .unwrap(),
        NativeMembershipPlan::Unassociated
    );
    f.put("A", &json!({"corrupt":true}));
    assert!(NativeMembershipObservation::observe(&current, &f.runtime, &caller).is_err());
}

#[test]
fn old_workspace_is_not_examined_or_retargeted_but_painted_wrong_workspace_refuses() {
    for painted in [false, true] {
        let f = Fixture::new(if painted { binding() } else { json!({}) });
        let current = records("B");
        f.put("B", &f.row("B", "other-absent-workspace"));
        let caller = f.caller();
        let observed = NativeMembershipObservation::observe(&current, &f.runtime, &caller).unwrap();
        if painted {
            assert!(observed.plan().is_err());
        } else {
            assert_eq!(observed.plan().unwrap(), NativeMembershipPlan::Unassociated);
        }
        assert_eq!(
            f.runtime
                .placement(caller.calling_workspace().scope(), "B")
                .unwrap()
                .unwrap()
                .binding()
                .unwrap()
                .workspace_id(),
            "other-absent-workspace"
        );
    }
}

#[test]
fn pending_known_or_unknown_workspace_and_cmux_are_not_herdr_ready() {
    for (kind, painted) in [
        ("known", true),
        ("unknown", true),
        ("other", false),
        ("cmux", true),
    ] {
        let f = Fixture::new(if painted { binding() } else { json!({}) });
        let mut row = f.row("B", if kind == "other" { "elsewhere" } else { "w" });
        if kind == "cmux" {
            row["binding"] =
                json!({"frontend":"cmux","workspace_id":"w","session_id":null,"command":null});
        } else {
            row["pending"] = json!({"session_id":null,"command":null,"cwd":"/chosen",
                "effects":[{"kind":"workspace_create"}],"launches":[]});
            if kind == "unknown" {
                row["binding"] = Value::Null;
            }
        }
        f.put("B", &row);
        let path = f.runtime.placement_path("B").unwrap();
        let before = std::fs::read(&path).unwrap();
        let current = records("B");
        let caller = f.caller();
        let observed = NativeMembershipObservation::observe(&current, &f.runtime, &caller).unwrap();
        assert!(observed.plan().is_err(), "{kind}");
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[test]
fn stream_global_endpoint_fence_is_not_an_absent_destination() {
    let f = Fixture::new(binding());
    let mut row = f.row("B", "w");
    row["scope"] = json!(herdr_scope(
        &f.directory.path().join("other.sock"),
        Some("test")
    ));
    f.put("B", &row);
    let caller = f.caller();
    let records = records("A");
    let before = std::fs::read(f.runtime.placement_path("B").unwrap()).unwrap();
    assert!(NativeMembershipObservation::observe(&records, &f.runtime, &caller).is_err());
    assert_eq!(
        std::fs::read(f.runtime.placement_path("B").unwrap()).unwrap(),
        before
    );
}

#[test]
fn wrong_actual_local_context_refuses_before_corrupt_placement_read() {
    let f = Fixture::new(binding());
    f.put("B", &json!({"corrupt":true}));
    let caller = f.caller();
    let changes: [fn(&mut RecordContext); 5] = [
        |records| records.host_id = "foreign".into(),
        |records| records.stream.as_mut().unwrap().owner_host_id = "foreign".into(),
        |records| records.session.as_mut().unwrap().stream_id = "other".into(),
        |records| records.stream = None,
        |records| {
            let session = records.session.as_mut().unwrap();
            session.harness_session_id = "another".into();
            session.id = Session::id_for("host", "pi", "another");
        },
    ];
    for change in changes {
        let mut current = records("A");
        change(&mut current);
        let error = NativeMembershipObservation::observe(&current, &f.runtime, &caller)
            .err()
            .unwrap()
            .to_string();
        assert!(
            error.contains("fixed local")
                || error.contains("exact local")
                || error.contains("no current Stream"),
            "{error}"
        );
    }
}
