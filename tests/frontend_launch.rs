//! Launch adapters use a scripted protocol-22 socket; no live Herdr effects.
use crate::support::herdr::Scripted;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use streams::frontend::herdr::{HerdrClient, HerdrPane};
use streams::frontend::*;

fn client() -> (Scripted, HerdrClient, WorkspaceRef) {
    let fake = Scripted::new();
    let client = HerdrClient::connect(fake.environment(), 0.5).unwrap();
    let workspace = WorkspaceRef {
        frontend: client.frontend_id(),
        endpoint: client.endpoint_id(),
        native_id: "w1".into(),
    };
    (fake, client, workspace)
}

fn response(evidence: &CommandEvidence) -> &[u8] {
    match &evidence.request {
        NativeRequest::Socket { response, .. } => response,
        NativeRequest::Process { .. } => panic!("Herdr started a process"),
    }
}

fn pane(id: &str) -> HerdrPane {
    HerdrPane {
        id: id.into(),
        tab_id: "w1:t1".into(),
        workspace_id: "w1".into(),
        cwd: "/observed".into(),
    }
}

fn native_pane(id: &str) -> Value {
    json!({"pane_id":id,"tab_id":"w1:t1","workspace_id":"w1","cwd":"/observed"})
}

fn created() -> Value {
    json!({"type":"tab_created","tab":{"tab_id":"w1:t1","workspace_id":"w1"},"root_pane":native_pane("w1:p1")})
}

fn started() -> Value {
    json!({"type":"agent_started","agent":{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1",
        "name":"streams-one","agent":"future-kind","interactive_ready":true,"terminal_id":"term-1"}})
}

fn uncertain(error: FrontendError) {
    assert_eq!(error.commands.len(), 1);
    assert_eq!(error.commands[0].outcome, CommandOutcome::Uncertain);
    assert!(!response(&error.commands[0]).is_empty());
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}

#[test]
fn exact_commands_preserve_shell_metacharacters_as_arguments() {
    let (fake, client, workspace) = client();
    let environment = BTreeMap::from([
        ("Z".into(), "$(touch /never); quoted ' text".into()),
        ("A".into(), "a b".into()),
    ]);
    let cwd = Path::new("/checkout with spaces/$(no)");
    fake.result(created());
    let (created_tab, root) = client
        .create_tab(&workspace, cwd, &environment, Some("Plan; $(no)"))
        .unwrap();
    assert_eq!(created_tab.id, "w1:t1");
    assert_eq!(root, pane("w1:p1"));
    fake.result(started());
    let receipt = client
        .start_agent(
            &root,
            "streams-one",
            "future-kind",
            &strings(&["", "--resume", "id ' ; $(no)"]),
        )
        .unwrap();
    assert_eq!(receipt.pane, root);
    assert_eq!(receipt.terminal_id.as_deref(), Some("term-1"));
    assert_eq!(receipt.name, "streams-one");
    assert_eq!(receipt.agent, "future-kind");
    assert!(receipt.interactive_ready);
    let cwd = cwd.to_str().unwrap();
    let env = json!({"A":"a b","Z":"$(touch /never); quoted ' text"});
    assert_eq!(
        fake.server.requests(),
        vec![
            ("ping".into(), json!({})),
            (
                "tab.create".into(),
                json!({"workspace_id":"w1","cwd":cwd,"env":env,"focus":false,"label":"Plan; $(no)"})
            ),
            (
                "agent.start".into(),
                json!({"name":"streams-one","kind":"future-kind","pane_id":"w1:p1",
                    "timeout_ms":20000,"args":["", "--resume", "id ' ; $(no)"]})
            ),
        ]
    );
    fake.exhausted();
}

#[test]
fn optional_terminal_id_accepts_absent_and_null() {
    let (fake, client, _) = client();
    for absent in [false, true] {
        let mut response = started();
        if absent {
            response["agent"]
                .as_object_mut()
                .unwrap()
                .remove("terminal_id");
        } else {
            response["agent"]["terminal_id"] = Value::Null;
        }
        fake.result(response);
        let receipt = client
            .start_agent(&pane("w1:p1"), "streams-one", "future-kind", &[])
            .unwrap();
        assert_eq!(receipt.terminal_id, None);
        assert_eq!(receipt.pane.cwd, "/observed");
    }
    fake.exhausted();
}

#[test]
fn wrong_creation_linkages_remain_uncertain() {
    let (fake, client, workspace) = client();
    for (object, field) in [
        ("tab", "workspace_id"),
        ("root_pane", "workspace_id"),
        ("root_pane", "tab_id"),
    ] {
        let mut response = created();
        response[object][field] = json!("wrong");
        fake.result(response);
        uncertain(
            client
                .create_tab(&workspace, Path::new("/checkout"), &BTreeMap::new(), None)
                .unwrap_err(),
        );
    }
    fake.exhausted();
}

#[test]
fn wrong_start_identity_readiness_or_terminal_retains_raw_evidence() {
    let (fake, client, _) = client();
    for (key, value) in [
        ("pane_id", json!("wrong")),
        ("tab_id", json!("wrong")),
        ("workspace_id", json!("wrong")),
        ("name", json!("wrong")),
        ("agent", json!("wrong")),
        ("interactive_ready", json!(false)),
        ("interactive_ready", json!("true")),
        ("interactive_ready", Value::Null),
        ("terminal_id", json!(5)),
        ("terminal_id", json!("")),
    ] {
        let mut response = started();
        response["agent"][key] = value;
        fake.result(response);
        uncertain(
            client
                .start_agent(&pane("w1:p1"), "streams-one", "future-kind", &[])
                .unwrap_err(),
        );
    }
    fake.exhausted();
}

#[test]
fn invalid_arguments_never_dispatch_effects() {
    let (fake, client, workspace) = client();
    for cwd in ["relative", "/bad\npath", "/bad\0path"] {
        assert!(
            client
                .create_tab(&workspace, Path::new(cwd), &BTreeMap::new(), None)
                .is_err()
        );
    }
    for (key, value) in [
        ("", "value"),
        ("A=B", "value"),
        ("A", "bad\nvalue"),
        ("A", "bad\0value"),
    ] {
        let environment = BTreeMap::from([(key.into(), value.into())]);
        assert!(
            client
                .create_tab(&workspace, Path::new("/checkout"), &environment, None)
                .is_err()
        );
    }
    for label in ["", "bad\nlabel"] {
        assert!(
            client
                .create_tab(
                    &workspace,
                    Path::new("/checkout"),
                    &BTreeMap::new(),
                    Some(label)
                )
                .is_err()
        );
    }
    for name in [
        "",
        "Upper",
        "1name",
        "bad.name",
        "bad\nname",
        "abcdefghijklmnopqrstuvwxyz1234567",
    ] {
        assert!(client.start_agent(&pane("w1:p1"), name, "pi", &[]).is_err());
    }
    for agent in ["", "pi\n"] {
        assert!(
            client
                .start_agent(&pane("w1:p1"), "valid", agent, &[])
                .is_err()
        );
    }
    assert!(
        client
            .start_agent(&pane("w1:p1"), "valid", "pi", &strings(&["bad\rarg"]))
            .is_err()
    );
    for reference in [
        WorkspaceRef {
            endpoint: EndpointId("other".into()),
            ..workspace.clone()
        },
        WorkspaceRef {
            frontend: FrontendId::Cmux,
            ..workspace.clone()
        },
    ] {
        assert!(
            client
                .create_tab(&reference, Path::new("/checkout"), &BTreeMap::new(), None)
                .is_err()
        );
    }
    let invalid = pane("");
    assert!(client.start_agent(&invalid, "valid", "pi", &[]).is_err());
    assert_eq!(fake.server.methods(), ["ping"]);
    fake.exhausted();
}
