//! Launch adapters use queued process replies; no live Herdr effects.
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use streams_temper::frontend::herdr::{HerdrClient, HerdrPane, HerdrTab};
use streams_temper::frontend::*;
use streams_temper::process::{ProcessError, ProcessOutput, ProcessRequest, ProcessRunner};

#[derive(Default)]
struct QueueRunner {
    replies: Mutex<VecDeque<Result<ProcessOutput, ProcessError>>>,
    requests: Mutex<Vec<ProcessRequest>>,
}

impl QueueRunner {
    fn reply(&self, value: Value) {
        self.replies.lock().unwrap().push_back(Ok(ProcessOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout: serde_json::to_vec(&value).unwrap(),
            stderr: Vec::new(),
        }));
    }

    fn effect(&self, result: Value) {
        self.reply(server("original"));
        self.reply(json!({"result":result}));
    }

    fn commands(&self) -> Vec<Vec<String>> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.args.iter().map(|a| a.to_str().unwrap().into()).collect())
            .collect()
    }

    fn exhausted(&self) {
        assert!(self.replies.lock().unwrap().is_empty());
    }
}

impl ProcessRunner for QueueRunner {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessOutput, ProcessError> {
        self.requests.lock().unwrap().push(request.clone());
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected command")
    }
}

fn server(session: &str) -> Value {
    json!({"server":{"running":true,"compatible":true,"endpoint_compatible":true,
        "version":"0.9.5","socket":"/tmp/herdr-test.sock","session":session}})
}

fn client() -> (Arc<QueueRunner>, HerdrClient, WorkspaceRef) {
    let runner = Arc::new(QueueRunner::default());
    runner.reply(server("original"));
    let client = HerdrClient::connect(
        runner.clone(),
        BTreeMap::from([("HERDR_BIN_PATH".into(), "/fake/herdr".into())]),
        0.5,
    )
    .unwrap();
    let workspace = WorkspaceRef {
        frontend: client.frontend_id(),
        endpoint: client.endpoint_id(),
        native_id: "w1".into(),
    };
    (runner, client, workspace)
}

fn pane(id: &str) -> HerdrPane {
    HerdrPane {
        id: id.into(),
        tab_id: "w1:t1".into(),
        workspace_id: "w1".into(),
        cwd: "/observed".into(),
    }
}

fn tab() -> HerdrTab {
    HerdrTab {
        id: "w1:t1".into(),
        workspace_id: "w1".into(),
        label: None,
        number: None,
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
    assert_eq!(error.commands.len(), 2);
    assert_eq!(error.commands[0].outcome, CommandOutcome::ReadOnly);
    assert_eq!(error.commands[1].outcome, CommandOutcome::Uncertain);
    assert!(!error.commands[1].stdout.is_empty());
}

fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).into()).collect()
}

#[test]
fn exact_commands_preserve_shell_metacharacters_as_arguments() {
    let (runner, client, workspace) = client();
    let environment = BTreeMap::from([
        ("Z".into(), "$(touch /never); quoted ' text".into()),
        ("A".into(), "a b".into()),
    ]);
    let cwd = Path::new("/checkout with spaces/$(no)");
    runner.effect(created());
    let (created_tab, root) = client
        .create_tab(&workspace, cwd, &environment, Some("Plan; $(no)"))
        .unwrap();
    assert_eq!(created_tab.id, "w1:t1");
    assert_eq!(root, pane("w1:p1"));
    runner.effect(json!({"type":"pane_split","pane":native_pane("w1:p2")}));
    assert_eq!(
        client.split_pane(&root, cwd, &environment).unwrap(),
        pane("w1:p2")
    );
    runner.effect(json!({"type":"tab_info","tab":{"tab_id":"w1:t1","workspace_id":"w1","label":"Plan; $(no)"}}));
    client.rename_tab(&created_tab, "Plan; $(no)").unwrap();
    runner.effect(started());
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
    runner.effect(json!({"type":"ok"}));
    client.focus_agent(&root).unwrap();
    let commands = runner.commands();
    assert_eq!(
        commands[2],
        strings(&[
            "tab",
            "create",
            "--workspace",
            "w1",
            "--cwd",
            cwd.to_str().unwrap(),
            "--env",
            "A=a b",
            "--env",
            "Z=$(touch /never); quoted ' text",
            "--no-focus",
            "--label",
            "Plan; $(no)"
        ])
    );
    assert_eq!(
        commands[4],
        strings(&[
            "pane",
            "split",
            "--pane",
            "w1:p1",
            "--direction",
            "right",
            "--cwd",
            cwd.to_str().unwrap(),
            "--env",
            "A=a b",
            "--env",
            "Z=$(touch /never); quoted ' text",
            "--no-focus"
        ])
    );
    assert_eq!(
        commands[6],
        strings(&["tab", "rename", "w1:t1", "Plan; $(no)"])
    );
    assert_eq!(
        commands[8],
        strings(&[
            "agent",
            "start",
            "streams-one",
            "--kind",
            "future-kind",
            "--pane",
            "w1:p1",
            "--timeout",
            "20000",
            "--",
            "",
            "--resume",
            "id ' ; $(no)"
        ])
    );
    assert_eq!(commands[10], strings(&["agent", "focus", "w1:p1"]));
    for request in runner.requests.lock().unwrap().iter() {
        assert_eq!(request.program, Path::new("/fake/herdr"));
        assert!(request.input.is_none());
    }
    runner.exhausted();
}

#[test]
fn optional_terminal_id_accepts_absent_and_null() {
    let (runner, client, _) = client();
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
        runner.effect(response);
        let receipt = client
            .start_agent(&pane("w1:p1"), "streams-one", "future-kind", &[])
            .unwrap();
        assert_eq!(receipt.terminal_id, None);
        assert_eq!(receipt.pane.cwd, "/observed");
    }
    runner.exhausted();
}

#[test]
fn wrong_creation_linkages_remain_uncertain() {
    let (runner, client, workspace) = client();
    for (object, field) in [
        ("tab", "workspace_id"),
        ("root_pane", "workspace_id"),
        ("root_pane", "tab_id"),
    ] {
        let mut response = created();
        response[object][field] = json!("wrong");
        runner.effect(response);
        uncertain(
            client
                .create_tab(&workspace, Path::new("/checkout"), &BTreeMap::new(), None)
                .unwrap_err(),
        );
    }
    for (field, value) in [
        ("workspace_id", "wrong"),
        ("tab_id", "wrong"),
        ("pane_id", "w1:p1"),
    ] {
        let mut returned = native_pane("w1:p2");
        returned[field] = json!(value);
        runner.effect(json!({"type":"pane_split","pane":returned}));
        uncertain(
            client
                .split_pane(&pane("w1:p1"), Path::new("/checkout"), &BTreeMap::new())
                .unwrap_err(),
        );
    }
    runner.exhausted();
}

#[test]
fn wrong_start_identity_readiness_or_terminal_retains_raw_evidence() {
    let (runner, client, _) = client();
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
        runner.effect(response);
        uncertain(
            client
                .start_agent(&pane("w1:p1"), "streams-one", "future-kind", &[])
                .unwrap_err(),
        );
    }
    runner.exhausted();
}

#[test]
fn rename_requires_exact_tuple_and_label() {
    let (runner, client, _) = client();
    for (key, value) in [
        ("tab_id", "wrong"),
        ("workspace_id", "wrong"),
        ("label", "wrong"),
    ] {
        let mut returned = json!({"tab_id":"w1:t1","workspace_id":"w1","label":"New"});
        returned[key] = json!(value);
        runner.effect(json!({"type":"tab_info","tab":returned}));
        uncertain(client.rename_tab(&tab(), "New").unwrap_err());
    }
    runner.exhausted();
}

#[test]
fn invalid_arguments_never_dispatch_effects() {
    let (runner, client, workspace) = client();
    for cwd in ["relative", "/bad\npath", "/bad\0path"] {
        assert!(
            client
                .create_tab(&workspace, Path::new(cwd), &BTreeMap::new(), None)
                .is_err()
        );
        assert!(
            client
                .split_pane(&pane("w1:p1"), Path::new(cwd), &BTreeMap::new())
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
        assert!(
            client
                .split_pane(&pane("w1:p1"), Path::new("/checkout"), &environment)
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
        assert!(client.rename_tab(&tab(), label).is_err());
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
            frontend: FrontendId::cmux(),
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
    assert!(
        client
            .split_pane(&invalid, Path::new("/checkout"), &BTreeMap::new())
            .is_err()
    );
    assert!(client.focus_agent(&invalid).is_err());
    assert!(client.start_agent(&invalid, "valid", "pi", &[]).is_err());
    assert_eq!(runner.commands(), vec![strings(&["status", "--json"])]);
    runner.exhausted();
}

#[test]
fn changed_endpoint_refuses_every_effect_and_focus_timeout_never_retries() {
    let (runner, client, workspace) = client();
    for action in 0..5 {
        runner.reply(server("replacement"));
        let error = match action {
            0 => client
                .create_tab(&workspace, Path::new("/checkout"), &BTreeMap::new(), None)
                .map(|_| ()),
            1 => client
                .split_pane(&pane("w1:p1"), Path::new("/checkout"), &BTreeMap::new())
                .map(|_| ()),
            2 => client.rename_tab(&tab(), "New"),
            3 => client
                .start_agent(&pane("w1:p1"), "valid", "pi", &[])
                .map(|_| ()),
            _ => client.focus_agent(&pane("w1:p1")),
        }
        .unwrap_err();
        assert_eq!(error.commands.len(), 1);
        assert_eq!(error.commands[0].outcome, CommandOutcome::ReadOnly);
    }
    assert!(
        runner
            .commands()
            .iter()
            .all(|c| c == &strings(&["status", "--json"]))
    );
    runner.reply(server("original"));
    runner.replies.lock().unwrap().push_back(Err(ProcessError {
        message: "timeout".into(),
        code: None,
        stdout: b"partial".to_vec(),
        stderr: Vec::new(),
    }));
    uncertain(client.focus_agent(&pane("w1:p1")).unwrap_err());
    assert_eq!(runner.commands().len(), 8);
    runner.exhausted();
}
