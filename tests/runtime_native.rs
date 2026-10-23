//! Read-only native attestation uses injected transport; no live native service is contacted.

use crate::support::herdr::{FakeHerdr, Reply};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};
use streams_temper::{
    frontend::herdr::HerdrClient,
    model::{Session, Stream},
    operations::Authority,
    runtime::{
        native::{BuiltinFrontend, attest_context, native_inventory, verify_pi_caller},
        state::{Association, RuntimeState, herdr_scope},
    },
};

struct Fixture {
    server: FakeHerdr,
    _directory: tempfile::TempDir,
    pane: Value,
    agent: Arc<Mutex<Value>>,
    snapshot: Arc<Mutex<Value>>,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("herdr-native-")
            .tempdir_in("/tmp")
            .unwrap();
        let pane =
            json!({"pane_id":"w:p1","tab_id":"w:t1","workspace_id":"w","terminal_id":"term1"});
        let agent = Arc::new(Mutex::new(
            json!({"pane_id":"w:p1","workspace_id":"w","agent":"pi", "agent_session":{"agent":"pi","kind":"id","value":"native"}}),
        ));
        let snapshot = Arc::new(Mutex::new(Value::Null));
        let server = FakeHerdr::start(&directory.path().join("herdr.sock"), {
            let (pane, agent, snapshot) = (pane.clone(), agent.clone(), snapshot.clone());
            move |method, params| {
                Reply::Result(match method {
                    "pane.current" => {
                        assert_eq!(params["caller_pane_id"], pane["pane_id"]);
                        json!({"type":"pane_current","pane":pane})
                    }
                    "workspace.get" => {
                        json!({"type":"workspace_info","workspace":{"workspace_id":"w","label":"Working"}})
                    }
                    "session.snapshot" => {
                        let mut snapshot = snapshot.lock().unwrap().clone();
                        if snapshot.is_null() {
                            let mut pane = pane.clone();
                            let mut agent_row = pane.clone();
                            agent_row
                                .as_object_mut()
                                .unwrap()
                                .extend(agent.lock().unwrap().as_object().unwrap().clone());
                            pane["agent"] = agent_row["agent"].clone();
                            pane["agent_session"] = agent_row["agent_session"].clone();
                            snapshot = json!({"workspaces":[{"workspace_id":"w","label":"Working"}],
                                "tabs":[{"workspace_id":"w","tab_id":"w:t1","label":"Working"}],
                                "panes":[pane], "agents":[agent_row],
                                "layouts":[{"workspace_id":"w","tab_id":"w:t1","panes":[{"pane_id":"w:p1"}]}]});
                        }
                        json!({"type":"session_snapshot","snapshot":snapshot})
                    }
                    _ => panic!("unexpected or mutating native request: {method} {params}"),
                })
            }
        });
        Self {
            server,
            _directory: directory,
            pane,
            agent,
            snapshot,
        }
    }
    fn socket(&self) -> &Path {
        &self.server.socket
    }
    fn environment(&self, pane: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("HERDR_ENV".into(), "1".into()),
            ("HERDR_PANE_ID".into(), pane.into()),
            ("HERDR_SESSION".into(), "test".into()),
            (
                "HERDR_SOCKET_PATH".into(),
                self.socket().to_str().unwrap().into(),
            ),
        ])
    }
    fn client(&self, pane: &str) -> HerdrClient {
        HerdrClient::connect(self.environment(pane), 1.0).unwrap()
    }
}
fn records() -> (Stream, Session, Authority) {
    (
        Stream::new("effort", "Effort", None, "host").unwrap(),
        Session::new("card", "host", "effort", "pi", "native", "Conversation").unwrap(),
        Authority {
            host_id: "host".into(),
            token: "creation".into(),
        },
    )
}
fn state(socket: &Path) -> (tempfile::TempDir, RuntimeState, String) {
    let temp = tempfile::tempdir().unwrap();
    let state =
        RuntimeState::new(temp.path().canonicalize().unwrap().join("runtime"), "host").unwrap();
    let scope = herdr_scope(socket, Some("test"));
    (temp, state, scope)
}

#[test]
fn reporting_and_current_inspection_have_distinct_read_only_identity_rules() {
    let fixture = Fixture::new();
    let frontend = BuiltinFrontend::Herdr(fixture.client("w:p1"));
    let caller = frontend.caller().unwrap();
    let (_temp, state, scope) = state(fixture.socket());
    assert_eq!(caller.scope(), scope);
    assert_eq!(caller.context_stream_id(&state, None).unwrap(), None);
    assert!(caller.current_stream_id(&state, None).is_err());
    assert!(!state.root().exists());
    {
        let mut enrollment = state
            .enrollment(Some((caller.scope(), caller.workspace_id())), None)
            .unwrap();
        enrollment.claim("effort").unwrap();
    }
    assert_eq!(
        caller.context_stream_id(&state, None).unwrap().as_deref(),
        Some("effort")
    );
    assert!(caller.current_stream_id(&state, None).is_err());
    for ready in [false, true] {
        state
            .stream_lock(Some(&scope), "effort")
            .unwrap()
            .write_association(&Association::new("effort", "w", None, ready).unwrap())
            .unwrap();
        assert_eq!(caller.current_stream_id(&state, None).unwrap(), "effort");
        assert_eq!(caller.context_stream_id(&state, None).is_ok(), ready);
        assert!(caller.context_stream_id(&state, Some("other")).is_err());
        assert_eq!(
            caller.current_stream_id(&state, Some("other")).is_err(),
            ready
        );
    }
}

#[test]
fn unknown_builtin_frontends_fail_before_transport() {
    let fixture = Fixture::new();
    assert!(
        BuiltinFrontend::connect(
            &streams_temper::frontend::FrontendId("other".into()),
            Arc::new(streams_temper::process::NativeProcessRunner),
            fixture.environment("w:p1"),
            1.0
        )
        .is_err()
    );
    assert!(fixture.server.requests().is_empty());
}

#[test]
fn exact_pi_caller_serializes_only_native_context() {
    let fixture = Fixture::new();
    let caller = verify_pi_caller(&fixture.client("w:p1"), "native").unwrap();
    assert_eq!(caller.native_session_id(), "native");
    assert_eq!(
        serde_json::to_value(caller).unwrap(),
        json!({"frontend":"herdr","pane_id":"w:p1","tab_id":"w:t1","workspace_id":"w"})
    );
}

#[test]
fn inherited_and_contradictory_native_identities_fail_closed() {
    let fixture = Fixture::new();
    assert!(verify_pi_caller(&fixture.client("w:p2"), "native").is_err());
    let client = fixture.client("w:p1");
    assert!(verify_pi_caller(&client, "background-helper").is_err());
    for (field, value) in [
        ("pane_id", "w:p2"),
        ("workspace_id", "other"),
        ("agent", "codex"),
    ] {
        let original = fixture.agent.lock().unwrap()[field].clone();
        fixture.agent.lock().unwrap()[field] = json!(value);
        assert!(verify_pi_caller(&client, "native").is_err());
        fixture.agent.lock().unwrap()[field] = original;
    }
    fixture.agent.lock().unwrap()["agent_session"]["kind"] = json!({"id":null});
    assert!(verify_pi_caller(&client, "native").is_err());
}

#[test]
fn attestation_requires_effective_local_authority_and_ready_binding() {
    let fixture = Fixture::new();
    let client = fixture.client("w:p1");
    let (_temp, state, scope) = state(fixture.socket());
    let (stream, mut session, authority) = records();
    assert!(attest_context(&client, &state, &stream, &session, Some(&authority)).is_err());
    assert!(!state.root().exists());
    state
        .stream_lock(Some(&scope), "effort")
        .unwrap()
        .write_association(&Association::new("effort", "w", None, true).unwrap())
        .unwrap();
    assert!(attest_context(&client, &state, &stream, &session, Some(&authority)).is_ok());
    assert!(attest_context(&client, &state, &stream, &session, None).is_err());
    let foreign = Authority {
        host_id: "foreign".into(),
        token: "creation".into(),
    };
    assert!(attest_context(&client, &state, &stream, &session, Some(&foreign)).is_err());
    for token in ["", " \t ", "bad\0token"] {
        let invalid = Authority {
            host_id: state.host_id().into(),
            token: token.into(),
        };
        assert!(attest_context(&client, &state, &stream, &session, Some(&invalid)).is_err());
    }
    // Opaque authority tokens retain the catalog's accepted text grammar.
    let opaque = Authority {
        host_id: state.host_id().into(),
        token: "retained\ncreation".into(),
    };
    assert!(attest_context(&client, &state, &stream, &session, Some(&opaque)).is_ok());
    session.host_id = "foreign".into();
    assert!(attest_context(&client, &state, &stream, &session, Some(&authority)).is_err());
}

#[test]
fn attestation_rejects_other_effort_and_unready_binding() {
    for (bound_stream, ready) in [("other", true), ("effort", false)] {
        let fixture = Fixture::new();
        let (_temp, state, scope) = state(fixture.socket());
        state
            .stream_lock(Some(&scope), bound_stream)
            .unwrap()
            .write_association(&Association::new(bound_stream, "w", None, ready).unwrap())
            .unwrap();
        let (stream, session, authority) = records();
        assert!(
            attest_context(
                &fixture.client("w:p1"),
                &state,
                &stream,
                &session,
                Some(&authority)
            )
            .is_err()
        );
    }
}

#[test]
fn inventory_preserves_pi_identity_without_transcripts_and_non_pi_panes() {
    let fixture = Fixture::new();
    let mut pi = fixture.pane.clone();
    pi["agent_session"] = json!({"agent":"pi","kind":"id","value":"missing-transcript"});
    let mut shell = fixture.pane.clone();
    shell["pane_id"] = json!("w:p2");
    shell["terminal_id"] = json!("term2");
    let mut other = shell.clone();
    other["pane_id"] = json!("w:p3");
    other["terminal_id"] = json!("term3");
    other["agent"] = json!("gemini");
    other["agent_session"] =
        json!({"agent":"gemini","kind":"path","value":"/never-read-this-unsupported-transcript"});
    *fixture.snapshot.lock().unwrap() = json!({"panes":[pi,shell,other]});
    let inventory = native_inventory(&fixture.client("w:p1")).unwrap();
    assert_eq!(
        inventory.panes()[0].native_session_id(),
        Some("missing-transcript")
    );
    assert_eq!(inventory.panes()[1].native_session_id(), None);
    assert_eq!(inventory.panes()[2].native_session_id(), None);
    assert_eq!(
        serde_json::to_value(inventory).unwrap()["socket"],
        fixture.socket().to_str().unwrap()
    );
}

#[test]
fn malformed_or_incomplete_inventory_is_not_evidence_of_death() {
    let fixture = Fixture::new();
    let client = fixture.client("w:p1");
    for snapshot in [
        json!({}),
        json!({"panes":[{}]}),
        json!({"panes":[{"pane_id":"p","tab_id":"t","workspace_id":"w","terminal_id":"term","agent":"pi"}]}),
        json!({"panes":[{"pane_id":"p","tab_id":"t","workspace_id":"w","terminal_id":"term","tokens":{"strm_role":"agent"}}]}),
    ] {
        *fixture.snapshot.lock().unwrap() = snapshot;
        assert!(native_inventory(&client).is_err());
    }
    for (agent, metadata, tokens) in [
        (
            json!(123),
            json!({"agent":"pi","kind":"id","value":"n"}),
            json!({}),
        ),
        (
            json!("codex"),
            json!({"agent":"pi","kind":"id","value":"n"}),
            json!({}),
        ),
        (
            json!("codex"),
            json!({"agent":"pi","kind":"id","value":"n"}),
            json!({"strm_role":"agent"}),
        ),
        (Value::Null, json!("corrupt"), json!({})),
        (Value::Null, json!({"kind":"id","value":"n"}), json!({})),
        (
            Value::Null,
            json!({"agent":"codex","kind":"other","value":"n"}),
            json!({}),
        ),
        (
            Value::Null,
            json!({"agent":"codex","kind":"id","value":""}),
            json!({}),
        ),
        (Value::Null, Value::Null, json!({"strm_role":123})),
    ] {
        let mut pane = fixture.pane.clone();
        pane["agent"] = agent;
        pane["agent_session"] = metadata;
        pane["tokens"] = tokens;
        *fixture.snapshot.lock().unwrap() = json!({"panes":[pane]});
        assert!(native_inventory(&client).is_err());
    }
    *fixture.snapshot.lock().unwrap() = json!({"panes":[fixture.pane,fixture.pane]});
    assert!(native_inventory(&client).is_err());
}
