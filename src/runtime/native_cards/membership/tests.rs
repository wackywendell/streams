use super::*;
use crate::{
    fake_herdr,
    model::{Session, Stream},
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
    fn new(binding: Option<(&str, bool)>, claim: Option<&str>, tokens: Value) -> Self {
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
        let workspace = herdr_scope(&server.socket, Some("test"));
        let [association, enrollment] = runtime.workspace_binding_paths(&workspace, "w").unwrap();
        if let Some((stream, ready)) = binding {
            std::fs::create_dir_all(association.parent().unwrap()).unwrap();
            std::fs::write(
                association,
                json!({"version":1,"stream_id":stream,"workspace_id":"w",
                "session_id":null,"command":null,"ready":ready})
                .to_string(),
            )
            .unwrap();
        }
        if let Some(stream) = claim {
            std::fs::create_dir_all(enrollment.parent().unwrap()).unwrap();
            std::fs::write(
                enrollment,
                json!({"version":1,"stream_id":stream,"workspace_id":"w"}).to_string(),
            )
            .unwrap();
        }
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
}

fn binding() -> Value {
    json!({"strm_catalog":json_digest(&"catalog").unwrap(),"strm_host":"host",
        "strm_stream":"B","strm_binding":"binding"})
}

#[test]
fn ready_intent_reuses_the_same_caller_and_named_relationships_without_rereading() {
    let fixture = Fixture::new(Some(("B", true)), Some("B"), binding());
    IDENTITY_FILE_OPENS.with(|files| files.borrow_mut().clear());
    let caller = fixture.caller();
    let observed =
        NativeMembershipObservation::observe("catalog", &fixture.runtime, &caller).unwrap();
    assert!(std::ptr::eq(observed.caller(), &caller));
    std::fs::rename(
        &fixture.transcript,
        fixture.directory.path().join("old-transcript"),
    )
    .unwrap();
    std::fs::rename(
        fixture.runtime.root(),
        fixture.directory.path().join("old-runtime"),
    )
    .unwrap();
    for _ in 0..2 {
        let NativeMembershipPlan::Ready(intent) = observed.plan().unwrap() else {
            panic!("ready")
        };
        assert_eq!(intent.host_id(), "host");
        assert_eq!(intent.session_id(), Session::id_for("host", "pi", "native"));
        assert_eq!(intent.destination_stream_id(), "B");
        let session =
            Session::new(intent.session_id(), "host", "A", "pi", "native", "Work").unwrap();
        let source = Stream::new("A", "Source", None, "host").unwrap();
        let destination = Stream::new("B", "Destination", None, "host").unwrap();
        let moved = apply_move(&session, &source, &destination, &intent)
            .unwrap()
            .unwrap();
        assert_eq!(moved.stream_id, "B");
        assert_eq!(moved.previous_stream_ids, ["A"]);
        let foreign = Stream::new("B", "Foreign", None, "another-host").unwrap();
        assert!(apply_move(&session, &source, &foreign, &intent).is_err());
    }
    IDENTITY_FILE_OPENS.with(|files| assert_eq!(files.borrow().len(), 1));
}

#[test]
fn only_ready_consistent_destinations_admit_an_intent() {
    let fixture = Fixture::new(None, None, json!({}));
    let caller = fixture.caller();
    let observed =
        NativeMembershipObservation::observe("catalog", &fixture.runtime, &caller).unwrap();
    assert_eq!(observed.plan().unwrap(), NativeMembershipPlan::Unassociated);
    for (association, claim, tokens, ready) in [
        (Some(("B", true)), None, json!({}), true),
        (Some(("B", true)), Some("B"), binding(), true),
        (
            Some(("B", true)),
            None,
            json!({"other_plugin":"value"}),
            true,
        ),
        (Some(("B", false)), None, json!({}), false),
        (None, Some("B"), json!({}), false),
        (None, None, binding(), false),
        (Some(("B", true)), Some("C"), binding(), false),
        (
            Some(("B", true)),
            None,
            json!({"strm_goal_short":"orphan"}),
            false,
        ),
    ] {
        let fixture = Fixture::new(association, claim, tokens.clone());
        let caller = fixture.caller();
        let observed =
            NativeMembershipObservation::observe("catalog", &fixture.runtime, &caller).unwrap();
        assert_eq!(
            matches!(observed.plan(), Ok(NativeMembershipPlan::Ready(_))),
            ready,
            "association={association:?}, claim={claim:?}, tokens={tokens}"
        );
    }
    for key in ["strm_catalog", "strm_host", "strm_stream", "strm_binding"] {
        for value in ["foreign", ""] {
            let mut tokens = binding();
            tokens[key] = json!(value);
            let fixture = Fixture::new(Some(("B", true)), None, tokens);
            let caller = fixture.caller();
            let observed =
                NativeMembershipObservation::observe("catalog", &fixture.runtime, &caller).unwrap();
            // A nonempty binding token is an opaque lifetime, not a fixed value.
            if key == "strm_binding" && value == "foreign" {
                assert!(matches!(
                    observed.plan(),
                    Ok(NativeMembershipPlan::Ready(_))
                ));
            } else {
                assert!(observed.plan().is_err(), "{key}");
            }
        }
    }
}

#[test]
fn unrelated_workspace_and_endpoint_records_are_not_discovered() {
    let fixture = Fixture::new(None, None, json!({}));
    let caller = fixture.caller();
    let workspace = caller.calling_workspace();
    let other_scope = herdr_scope(&fixture.directory.path().join("other.sock"), Some("test"));
    for (scope, id) in [(workspace.scope(), "other"), (other_scope.as_str(), "w")] {
        for path in fixture.runtime.workspace_binding_paths(scope, id).unwrap() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "not json").unwrap();
        }
    }
    let observed =
        NativeMembershipObservation::observe("catalog", &fixture.runtime, &caller).unwrap();
    assert_eq!(observed.plan().unwrap(), NativeMembershipPlan::Unassociated);
    let path = &fixture
        .runtime
        .workspace_binding_paths(workspace.scope(), "w")
        .unwrap()[0];
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "not json").unwrap();
    assert!(NativeMembershipObservation::observe("catalog", &fixture.runtime, &caller).is_err());
}
