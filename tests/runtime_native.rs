//! Read-only native attestation uses injected transport; no live native service is contacted.

use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::process::ExitStatusExt,
    sync::{Arc, Mutex},
};
use streams_temper::{
    frontend::herdr::HerdrClient,
    model::{Session, Stream},
    operations::Authority,
    process::{ProcessError, ProcessOutput, ProcessRequest, ProcessRunner},
    runtime::{
        native::{BuiltinFrontend, attest_context, native_inventory, verify_pi_caller},
        state::{Association, RuntimeState, herdr_scope},
    },
};

struct Fixture {
    pane: Value,
    agent: Mutex<Value>,
    snapshot: Mutex<Value>,
    commands: Mutex<Vec<Vec<String>>>,
}
impl Fixture {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            pane: json!({"pane_id":"w:p1","tab_id":"w:t1","workspace_id":"w","terminal_id":"term1"}),
            agent: Mutex::new(
                json!({"pane_id":"w:p1","workspace_id":"w","agent":"pi", "agent_session":{"agent":"pi","kind":"id","value":"native"}}),
            ),
            snapshot: Mutex::new(json!({"panes":[]})),
            commands: Mutex::new(vec![]),
        })
    }
    fn client(self: &Arc<Self>, pane: &str) -> HerdrClient {
        HerdrClient::connect(
            self.clone(),
            BTreeMap::from([
                ("HERDR_ENV".into(), "1".into()),
                ("HERDR_PANE_ID".into(), pane.into()),
                ("HERDR_BIN_PATH".into(), "/never/execute/herdr".into()),
            ]),
            1.0,
        )
        .unwrap()
    }
}
impl ProcessRunner for Fixture {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessOutput, ProcessError> {
        assert_eq!(request.program.to_str(), Some("/never/execute/herdr"));
        let args: Vec<String> = request
            .args
            .iter()
            .map(|arg| arg.to_str().unwrap().into())
            .collect();
        self.commands.lock().unwrap().push(args.clone());
        let response = match (args[0].as_str(), args[1].as_str()) {
            ("status", "--json") => {
                json!({"server":{"running":true,"compatible":true,"endpoint_compatible":true,"version":"0.9.5","socket":"/tmp/herdr.sock","session":"test"}})
            }
            ("pane", "current") => json!({"result":{"type":"pane_current","pane":self.pane}}),
            ("agent", "get") => {
                json!({"result":{"type":"agent_info","agent":*self.agent.lock().unwrap()}})
            }
            ("workspace", "get") => {
                json!({"result":{"type":"workspace_info","workspace":{"workspace_id":"w","label":"Working"}}})
            }
            ("api", "snapshot") => {
                json!({"result":{"type":"session_snapshot","snapshot":*self.snapshot.lock().unwrap()}})
            }
            _ => panic!("unexpected or mutating native command: {args:?}"),
        };
        Ok(ProcessOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout: serde_json::to_vec(&response).unwrap(),
            stderr: vec![],
        })
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
fn state() -> (tempfile::TempDir, RuntimeState, String) {
    let temp = tempfile::tempdir().unwrap();
    let state =
        RuntimeState::new(temp.path().canonicalize().unwrap().join("runtime"), "host").unwrap();
    let scope = herdr_scope(std::path::Path::new("/tmp/herdr.sock"), Some("test"));
    (temp, state, scope)
}

#[test]
fn reporting_and_current_inspection_have_distinct_read_only_identity_rules() {
    let fixture = Fixture::new();
    let frontend = BuiltinFrontend::Herdr(fixture.client("w:p1"));
    let caller = frontend.caller().unwrap();
    let (_temp, state, scope) = state();
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
            fixture.clone(),
            BTreeMap::new(),
            1.0
        )
        .is_err()
    );
    assert!(fixture.commands.lock().unwrap().is_empty());
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
    let (_temp, state, scope) = state();
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
        let (_temp, state, scope) = state();
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
        "/tmp/herdr.sock"
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
