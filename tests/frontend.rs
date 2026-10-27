//! Native adapters are exercised only through injected runners and scripted
//! sockets, never live servers.

use crate::support::herdr::{Reply, Scripted};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use streams_temper::frontend::cmux::CmuxClient;
use streams_temper::frontend::herdr::{HerdrClient, HerdrEndpoint, HerdrTopology};
use streams_temper::frontend::*;
use streams_temper::process::{ProcessError, ProcessOutput, ProcessRequest, ProcessRunner};

#[derive(Default)]
struct FakeRunner {
    replies: Mutex<VecDeque<Result<ProcessOutput, ProcessError>>>,
    requests: Mutex<Vec<ProcessRequest>>,
}

impl FakeRunner {
    fn json(&self, value: Value) {
        self.bytes(serde_json::to_vec(&value).unwrap());
    }
    fn bytes(&self, bytes: impl Into<Vec<u8>>) {
        self.replies.lock().unwrap().push_back(Ok(ProcessOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout: bytes.into(),
            stderr: Vec::new(),
        }));
    }
    fn uncertain(&self) {
        self.replies.lock().unwrap().push_back(Err(ProcessError {
            message: "timeout; termination/reaping attempted".into(),
            code: None,
            stdout: b"partial-created-id".to_vec(),
            stderr: b"partial-stderr".to_vec(),
        }));
    }
    fn commands(&self) -> Vec<Vec<String>> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| {
                r.args
                    .iter()
                    .map(|s| s.to_str().unwrap().to_owned())
                    .collect()
            })
            .collect()
    }
    fn exhausted(&self) {
        assert!(self.replies.lock().unwrap().is_empty());
    }
}

impl ProcessRunner for FakeRunner {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessOutput, ProcessError> {
        self.requests.lock().unwrap().push(request.clone());
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected transport call")
    }
}

fn context(values: &[(&str, &str)]) -> BTreeMap<String, String> {
    values
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect()
}
fn request() -> CreateWorkspace {
    CreateWorkspace {
        title: "An effort".into(),
        cwd: "/checkout".into(),
        environment: context(&[("STREAMS_CATALOG_DIR", "/catalog")]),
    }
}
fn cmux(runner: &Arc<FakeRunner>, extra: &[(&str, &str)]) -> CmuxClient {
    let mut env = context(&[("HOME", "/home/test"), ("PATH", "/bin")]);
    env.extend(context(extra));
    CmuxClient::connect(runner.clone(), env, 0.5).unwrap()
}
fn herdr(fake: &Scripted, extra: &[(&str, &str)]) -> HerdrClient {
    let mut env = fake.environment();
    env.extend(context(extra));
    HerdrClient::connect(env, 0.5).unwrap()
}
fn result(kind: &str, field: &str, value: Value) -> Value {
    let mut result = json!({"type":kind});
    result[field] = value;
    result
}
/// The raw reply frame retained as Herdr effect evidence.
fn response(evidence: &CommandEvidence) -> &[u8] {
    match &evidence.request {
        NativeRequest::Socket { response, .. } => response,
        NativeRequest::Process { .. } => panic!("Herdr started a process"),
    }
}
fn stdout(evidence: &CommandEvidence) -> &[u8] {
    match &evidence.request {
        NativeRequest::Process { stdout, .. } => stdout,
        NativeRequest::Socket { .. } => panic!("CMUX used a socket"),
    }
}
fn native_workspace(id: &str, title: &str) -> Value {
    json!({"workspace_id":id,"label":title})
}
fn cmux_inventory() -> Value {
    json!({"workspaces":[{"id":"uuid-1","ref":"workspace:1","title":"One","index":1,"current_directory":"/checkout"}]})
}
fn snapshot() -> Value {
    json!({"workspaces":[{"workspace_id":"w1","label":"One"}],
        "tabs":[{"tab_id":"w1:t1","workspace_id":"w1","label":"Working","number":1}],
        "panes":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term-1","foreground_cwd":"/foreground","cwd":"/shell"}],
        "layouts":[{"tab_id":"w1:t1","workspace_id":"w1","panes":[{"pane_id":"w1:p1"}]}],
        "agents":[{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","terminal_id":"term-1","agent":"pi","agent_session":{"id":"native-session"}}]})
}
fn reference(frontend: &dyn WorkspaceFrontend, id: &str) -> WorkspaceRef {
    WorkspaceRef {
        frontend: frontend.frontend_id(),
        endpoint: frontend.endpoint_id(),
        native_id: id.into(),
    }
}
fn expected(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).into()).collect()
}

#[test]
fn selection_preserves_auto_none_explicit_and_override_policy() {
    let empty = BTreeMap::new();
    assert_eq!(
        resolve_frontend(FrontendMode::Auto, None, &empty)
            .unwrap()
            .resolved,
        None
    );
    let both = context(&[
        ("CMUX_WORKSPACE_ID", "c1"),
        ("HERDR_ENV", "1"),
        ("HERDR_PANE_ID", "w1:p1"),
    ]);
    assert!(resolve_frontend(FrontendMode::Auto, None, &both).is_err());
    assert!(resolve_frontend(FrontendMode::Auto, Some(FrontendRequest::Auto), &both).is_err());
    for values in [
        vec![("HERDR_ENV", "1")],
        vec![("HERDR_PANE_ID", "w1:p1")],
        vec![("HERDR_ENV", "0"), ("HERDR_PANE_ID", "w1:p1")],
    ] {
        assert!(resolve_frontend(FrontendMode::Auto, None, &context(&values)).is_err());
    }
    for id in [
        FrontendId::cmux(),
        FrontendId::herdr(),
        FrontendId("future".into()),
    ] {
        assert_eq!(
            resolve_frontend(
                FrontendMode::Explicit(id.clone()),
                Some(FrontendRequest::Auto),
                &both
            )
            .unwrap()
            .resolved,
            Some(id.clone())
        );
        assert_eq!(
            resolve_frontend(
                FrontendMode::None,
                Some(FrontendRequest::Explicit(id.clone())),
                &both
            )
            .unwrap()
            .resolved,
            Some(id)
        );
    }
    assert_eq!(
        resolve_frontend(FrontendMode::None, None, &both)
            .unwrap()
            .resolved,
        None
    );
    for (values, id) in [
        (vec![("CMUX_WORKSPACE_ID", "c1")], FrontendId::cmux()),
        (
            vec![("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1")],
            FrontendId::herdr(),
        ),
    ] {
        assert_eq!(
            resolve_frontend(FrontendMode::Auto, None, &context(&values))
                .unwrap()
                .resolved,
            Some(id)
        );
    }
}

#[test]
fn cmux_inventory_banner_lookup_and_exact_caller_commands() {
    let fake = Arc::new(FakeRunner::default());
    let client = cmux(&fake, &[("CMUX_WORKSPACE_ID", "uuid-1")]);
    fake.bytes(format!("cmux notice\n{}", cmux_inventory()));
    assert_eq!(
        client
            .resolve(WorkspaceSelector::Caller)
            .unwrap()
            .reference
            .native_id,
        "uuid-1"
    );
    for selector in ["workspace:1", "uuid-1", "One"] {
        fake.json(cmux_inventory());
        assert_eq!(
            client
                .resolve(WorkspaceSelector::Explicit(selector))
                .unwrap()
                .title,
            "One"
        );
    }
    for command in fake.commands() {
        assert_eq!(command, expected(&["rpc", "workspace.list", "{}"]));
    }
    assert_eq!(
        fake.requests.lock().unwrap()[0]
            .env
            .get(std::ffi::OsStr::new("CMUX_SOCKET_PATH"))
            .unwrap(),
        "/home/test/.local/state/cmux/cmux.sock"
    );
    fake.exhausted();
}

#[test]
fn cmux_never_uses_gui_focus_or_title_as_caller_evidence() {
    let fake = Arc::new(FakeRunner::default());
    assert!(cmux(&fake, &[]).resolve(WorkspaceSelector::Caller).is_err());
    assert!(fake.commands().is_empty());
    let client = cmux(&fake, &[("CMUX_WORKSPACE_ID", "One")]);
    fake.json(cmux_inventory());
    assert!(client.resolve(WorkspaceSelector::Caller).is_err());
    let client = cmux(&fake, &[("CMUX_WORKSPACE_ID", "workspace:1")]);
    fake.json(cmux_inventory());
    assert!(client.resolve(WorkspaceSelector::Caller).is_err());
}

#[test]
fn cmux_rejects_malformed_incomplete_duplicate_and_ambiguous_inventory() {
    let fake = Arc::new(FakeRunner::default());
    let client = cmux(&fake, &[]);
    let mut duplicate = cmux_inventory();
    let first = duplicate["workspaces"][0].clone();
    duplicate["workspaces"].as_array_mut().unwrap().push(first);
    for payload in [
        json!([]),
        json!({}),
        json!({"workspaces":[null]}),
        json!({"workspaces":[{}]}),
        json!({"workspaces":[{"id":"x","title":3}]}),
        json!({"workspaces":[{"id":"x","title":"X","ref":5}]}),
        json!({"workspaces":[{"id":"x","title":"X","index":true}]}),
        json!({"workspaces":[{"id":"x","title":"X","current_directory":false}]}),
        duplicate,
    ] {
        fake.json(payload);
        let error = client.workspaces().unwrap_err();
        assert_eq!(error.commands.len(), 1);
    }
    fake.json(json!({"workspaces":[{"id":"1","title":"Same"},{"id":"2","title":"Same"}]}));
    assert!(client.resolve(WorkspaceSelector::Explicit("Same")).is_err());
    for bad in [b"not-json".to_vec(), b"{} trailing".to_vec(), vec![255]] {
        fake.bytes(bad);
        assert!(client.workspaces().is_err());
    }
}

#[test]
fn cmux_create_focus_and_rename_follow_native_contracts() {
    let fake = Arc::new(FakeRunner::default());
    let client = cmux(&fake, &[]);
    fake.json(json!({"workspace_id":"new-id","surface_id":"surface-1"}));
    let created = client.create(&request()).unwrap();
    assert_eq!(created.reference.native_id, "new-id");
    assert_eq!(
        fake.commands()[0],
        expected(&[
            "--json",
            "--id-format",
            "uuids",
            "new-workspace",
            "--name",
            "An effort",
            "--cwd",
            "/checkout",
            "--env",
            "STREAMS_CATALOG_DIR=/catalog",
            "--focus",
            "true"
        ])
    );
    let target = reference(&client, "uuid-1");
    fake.json(cmux_inventory());
    fake.bytes("");
    client.focus(&target).unwrap();
    fake.json(cmux_inventory());
    fake.bytes("");
    client.rename(&target, "Renamed").unwrap();
    let commands = fake.commands();
    assert_eq!(
        commands[2],
        expected(&["select-workspace", "--workspace", "uuid-1"])
    );
    assert_eq!(
        commands[4],
        expected(&[
            "workspace-action",
            "--workspace",
            "workspace:1",
            "--action",
            "rename",
            "--title",
            "Renamed"
        ])
    );
    fake.exhausted();
}

#[test]
fn wrong_frontend_endpoint_or_empty_ref_never_dispatches() {
    let fake = Arc::new(FakeRunner::default());
    let client = cmux(&fake, &[]);
    for target in [
        WorkspaceRef {
            frontend: FrontendId::herdr(),
            ..reference(&client, "w1")
        },
        WorkspaceRef {
            endpoint: EndpointId("other".into()),
            ..reference(&client, "w1")
        },
        reference(&client, ""),
    ] {
        assert!(client.focus(&target).is_err());
        assert!(client.rename(&target, "Title").is_err());
        assert!(client.resolve(WorkspaceSelector::Exact(&target)).is_err());
    }
    assert!(fake.commands().is_empty());
}

#[test]
fn creation_errors_preserve_uncertainty_raw_resources_and_never_retry() {
    let fake = Arc::new(FakeRunner::default());
    let client = cmux(&fake, &[]);
    for payload in [
        json!({"surface_id":"created-but-no-workspace"}),
        json!({"workspace_id":5}),
    ] {
        fake.json(payload.clone());
        let error = client.create(&request()).unwrap_err();
        assert_eq!(error.commands.len(), 1);
        assert_eq!(error.commands[0].outcome, CommandOutcome::Uncertain);
        assert_eq!(
            serde_json::from_slice::<Value>(stdout(&error.commands[0])).unwrap(),
            payload
        );
    }
    fake.uncertain();
    let error = client.create(&request()).unwrap_err();
    assert_eq!(error.commands.len(), 1);
    assert_eq!(
        error.commands[0].request,
        NativeRequest::Process {
            args: fake.commands()[2].clone(),
            code: None,
            stdout: b"partial-created-id".to_vec(),
            stderr: b"partial-stderr".to_vec(),
        }
    );
    assert_eq!(error.commands[0].outcome, CommandOutcome::Uncertain);
    assert_eq!(fake.commands().len(), 3);
}

#[test]
fn finite_timeout_and_launch_validation_precede_any_transport_call() {
    let fake = Arc::new(FakeRunner::default());
    let herdr_fake = Scripted::new();
    for timeout in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX] {
        assert!(CmuxClient::connect(fake.clone(), BTreeMap::new(), timeout).is_err());
        assert!(HerdrClient::connect(herdr_fake.environment(), timeout).is_err());
    }
    assert!(herdr_fake.server.requests().is_empty());
    let client = cmux(&fake, &[]);
    let mut invalid = request();
    invalid.cwd = "relative".into();
    assert!(client.create(&invalid).is_err());
    invalid = request();
    invalid.environment.insert("BAD=KEY".into(), "value".into());
    assert!(client.create(&invalid).is_err());
    assert!(fake.commands().is_empty());
}

#[test]
fn duplicate_native_identity_fields_fail_closed_with_uncertain_effect_evidence() {
    let fake = Arc::new(FakeRunner::default());
    let client = cmux(&fake, &[]);
    let reply = br#"{"workspace_id":"foreign","workspace_id":"uuid-1"}"#;
    fake.bytes(reply.to_vec());
    let error = client.create(&request()).unwrap_err();
    assert!(error.message.contains("duplicate JSON field"));
    assert_eq!(error.commands.len(), 1);
    assert_eq!(error.commands[0].outcome, CommandOutcome::Uncertain);
    assert_eq!(stdout(&error.commands[0]), reply);
    assert_eq!(fake.commands().len(), 1);
    fake.exhausted();

    let fake = Scripted::new();
    let client = herdr(&fake, &[]);
    // The ping used request ID 1.
    let reply = concat!(
        r#"{"id":"2","result":{"type":"workspace_created","workspace":{"workspace_id":"foreign","workspace_id":"w1","label":"Work"},"#,
        r#""tab":{"tab_id":"w1:t1","workspace_id":"w1"},"root_pane":{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1"}}}"#,
        "\n"
    )
    .as_bytes();
    fake.reply(Reply::Raw(reply.to_vec()));
    let error = client.create_native(&request()).unwrap_err();
    assert!(error.message.contains("duplicate JSON field"));
    assert_eq!(error.commands.len(), 1);
    assert_eq!(error.commands[0].outcome, CommandOutcome::Uncertain);
    assert_eq!(response(&error.commands[0]), reply);
    assert_eq!(fake.server.methods(), ["ping", "workspace.create"]);
    fake.exhausted();
}

#[test]
fn herdr_endpoint_resolution_follows_herdr_and_fails_closed() {
    let resolve = |values: &[(&str, &str)]| HerdrEndpoint::resolve(&context(values));
    let endpoint = resolve(&[("HOME", "/home/test")]).unwrap();
    assert_eq!(
        endpoint.socket,
        Path::new("/home/test/.config/herdr/herdr.sock")
    );
    assert_eq!(endpoint.session, None);
    let endpoint = resolve(&[
        ("HOME", "/home/test"),
        ("XDG_CONFIG_HOME", "/xdg"),
        ("HERDR_SESSION", "work.1"),
    ])
    .unwrap();
    assert_eq!(
        endpoint.socket,
        Path::new("/xdg/herdr/sessions/work.1/herdr.sock")
    );
    assert_eq!(endpoint.session.as_deref(), Some("work.1"));
    let endpoint =
        resolve(&[("HERDR_SOCKET_PATH", "/run/h.sock"), ("HERDR_SESSION", "s")]).unwrap();
    assert_eq!(endpoint.socket, Path::new("/run/h.sock"));
    for values in [
        vec![],
        vec![("HERDR_SOCKET_PATH", "relative.sock")],
        vec![("HOME", "relative")],
        vec![("HOME", "/home/test"), ("HERDR_SESSION", "..")],
        vec![("HOME", "/home/test"), ("HERDR_SESSION", "a/b")],
    ] {
        assert!(resolve(&values).is_err(), "{values:?}");
    }
    // A missing server is a connection error, not a fallback.
    assert!(
        HerdrClient::connect(
            context(&[("HERDR_SOCKET_PATH", "/nonexistent/h.sock")]),
            0.5
        )
        .is_err()
    );
}

#[test]
fn herdr_reader_context_cannot_change_the_pinned_socket_or_namespace() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[("HERDR_SESSION", "named")]);
    let mut reader = fake.environment();
    reader.insert("HERDR_SESSION".into(), "named".into());
    reader.insert("HERDR_PANE_ID".into(), "owner".into());
    assert!(client.with_context(reader.clone()).is_ok());
    for (key, value) in [
        ("HERDR_SOCKET_PATH", "/different.sock"),
        ("HERDR_SESSION", "other"),
        ("HERDR_SESSION", ""),
    ] {
        let mut changed = reader.clone();
        changed.insert(key.into(), value.into());
        assert!(client.with_context(changed).is_err(), "{key}={value}");
    }
    reader.remove("HERDR_SESSION");
    assert!(client.with_context(reader).is_err());
    assert_eq!(fake.server.methods(), ["ping"]);
}

#[test]
fn herdr_snapshot_caller_requires_one_valid_matching_workspace() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1")]);
    let good = snapshot();
    assert!(client.current_pane_from(&good).is_ok());
    for workspaces in [
        Value::Null,
        json!({}),
        json!([]),
        json!([native_workspace("other", "Other")]),
        json!([good["workspaces"][0], good["workspaces"][0]]),
        json!([{"workspace_id":"w1","label":null}]),
    ] {
        let mut changed = good.clone();
        changed["workspaces"] = workspaces;
        assert!(client.current_pane_from(&changed).is_err(), "{changed}");
    }
    assert_eq!(fake.server.methods(), ["ping"]);
}

#[test]
fn herdr_caller_pane_and_workspace_are_exact_and_endpoint_scoped() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "w1:p1")]);
    fake.result(result(
        "pane_current",
        "pane",
        snapshot()["panes"][0].clone(),
    ));
    fake.result(result(
        "workspace_info",
        "workspace",
        native_workspace("w1", "One"),
    ));
    let workspace = client.resolve(WorkspaceSelector::Caller).unwrap();
    assert_eq!(workspace.reference, reference(&client, "w1"));
    assert_eq!(workspace.cwd, "/foreground");
    assert_eq!(
        client.endpoint_id(),
        HerdrEndpoint::resolve(&fake.environment())
            .unwrap()
            .identity()
    );
    // The endpoint is pinned once; later effects do not re-check the version.
    fake.result(json!({"type":"ok"}));
    client.focus(&workspace.reference).unwrap();
    assert_eq!(
        fake.server.requests(),
        vec![
            ("ping".into(), json!({})),
            ("pane.current".into(), json!({"caller_pane_id":"w1:p1"})),
            ("workspace.get".into(), json!({"workspace_id":"w1"})),
            ("workspace.focus".into(), json!({"workspace_id":"w1"})),
        ]
    );
    fake.exhausted();
}

#[test]
fn herdr_missing_caller_mismatched_pane_and_workspace_fail_closed() {
    for values in [
        vec![],
        vec![("HERDR_ENV", "1")],
        vec![("HERDR_PANE_ID", "w1:p1")],
    ] {
        let fake = Scripted::new();
        let client = herdr(&fake, &values);
        assert!(client.resolve(WorkspaceSelector::Caller).is_err());
        assert!(client.resolve(WorkspaceSelector::Explicit(" ")).is_err());
        assert_eq!(fake.server.methods(), ["ping"]);
    }
    let fake = Scripted::new();
    let client = herdr(&fake, &[("HERDR_ENV", "1"), ("HERDR_PANE_ID", "different")]);
    fake.result(result(
        "pane_current",
        "pane",
        snapshot()["panes"][0].clone(),
    ));
    assert!(client.resolve(WorkspaceSelector::Caller).is_err());
    fake.result(result(
        "workspace_info",
        "workspace",
        native_workspace("wrong", "One"),
    ));
    assert!(client.resolve(WorkspaceSelector::Explicit("w1")).is_err());
    for target in [
        WorkspaceRef {
            frontend: FrontendId::cmux(),
            ..reference(&client, "w1")
        },
        WorkspaceRef {
            endpoint: EndpointId("other".into()),
            ..reference(&client, "w1")
        },
    ] {
        assert!(client.focus(&target).is_err());
        assert!(client.resolve(WorkspaceSelector::Exact(&target)).is_err());
    }
    assert_eq!(
        fake.server.methods(),
        ["ping", "pane.current", "workspace.get"]
    );
    fake.exhausted();
}

fn created() -> Value {
    json!({"type":"workspace_created","workspace":{"workspace_id":"w1","label":"An effort"},"tab":{"tab_id":"w1:t1","workspace_id":"w1"},"root_pane":{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"/checkout"}})
}

#[test]
fn herdr_creation_retains_native_root_tuple_without_installing_or_launching() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[]);
    fake.result(created());
    let receipt = client.create_native(&request()).unwrap();
    assert_eq!(receipt.root_pane.id, "w1:p1");
    assert_eq!(receipt.tab.id, "w1:t1");
    assert_eq!(receipt.workspace.reference.native_id, "w1");
    assert_eq!(
        fake.server.requests()[1..],
        [(
            "workspace.create".into(),
            json!({"cwd":"/checkout","label":"An effort",
                "env":{"STREAMS_CATALOG_DIR":"/catalog"},"focus":true})
        )]
    );
    fake.exhausted();
}

#[test]
fn herdr_bad_creation_receipts_keep_uncertain_resource_evidence() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[]);
    let mut mismatch = created();
    mismatch["root_pane"]["tab_id"] = json!("other");
    let mut no_root = created();
    no_root.as_object_mut().unwrap().remove("root_pane");
    for (key, payload) in [
        ("result", mismatch),
        ("result", no_root),
        (
            "result",
            json!({"type":"wrong","workspace":{"workspace_id":"surviving"}}),
        ),
        ("error", json!({"message":"native error"})),
    ] {
        fake.reply(if key == "error" {
            Reply::Error(payload.clone())
        } else {
            Reply::Result(payload.clone())
        });
        let error = client.create(&request()).unwrap_err();
        let evidence = error.commands.last().unwrap();
        assert_eq!(evidence.outcome, CommandOutcome::Uncertain);
        assert_eq!(
            serde_json::from_slice::<Value>(response(evidence)).unwrap()[key],
            payload
        );
    }
    fake.reply(Reply::Raw(b"banner {}\n".to_vec()));
    assert!(client.create(&request()).is_err());
    let creates = fake
        .server
        .methods()
        .iter()
        .filter(|m| *m == "workspace.create")
        .count();
    assert_eq!(creates, 5);
    fake.exhausted();
}

#[test]
fn herdr_focus_and_rename_contracts_and_uncertain_effects() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[]);
    let target = reference(&client, "w1");
    fake.result(json!({"type":"ok"}));
    client.focus(&target).unwrap();
    fake.result(result(
        "workspace_info",
        "workspace",
        native_workspace("w1", "Title"),
    ));
    client.rename(&target, "Title").unwrap();
    assert_eq!(
        fake.server.requests()[1..],
        [
            ("workspace.focus".into(), json!({"workspace_id":"w1"})),
            (
                "workspace.rename".into(),
                json!({"workspace_id":"w1","label":"Title"})
            ),
        ]
    );
    fake.reply(Reply::Raw(Vec::new()));
    let error = client.rename(&target, "Title").unwrap_err();
    assert_eq!(error.commands.len(), 1);
    assert_eq!(error.commands[0].outcome, CommandOutcome::Uncertain);
    fake.exhausted();
}

#[test]
fn herdr_complete_topology_preserves_exact_identities_and_native_agent_payload() {
    let fake = Scripted::new();
    let client = herdr(&fake, &[]);
    fake.result(result("session_snapshot", "snapshot", snapshot()));
    let topology = client.topology().unwrap();
    assert_eq!(topology.panes[0].terminal_id, "term-1");
    assert_eq!(topology.panes[0].pane.cwd, "/foreground");
    assert_eq!(
        topology.agents[0].native["agent_session"]["id"],
        "native-session"
    );
    fake.result(result("session_snapshot", "snapshot", snapshot()));
    assert_eq!(client.workspaces().unwrap().len(), 1);
    assert_eq!(
        fake.server.methods(),
        ["ping", "session.snapshot", "session.snapshot"]
    );
    fake.exhausted();
}

#[test]
fn herdr_topology_rejects_every_missing_collection_and_broken_relationship() {
    let good = snapshot();
    for field in ["workspaces", "tabs", "panes", "layouts", "agents"] {
        let mut bad = good.clone();
        bad.as_object_mut().unwrap().remove(field);
        assert!(HerdrTopology::parse(&bad).is_err(), "{field}");
        let mut bad = good.clone();
        bad[field] = json!({});
        assert!(HerdrTopology::parse(&bad).is_err(), "{field}");
    }
    let mut bads = Vec::new();
    for field in ["workspaces", "tabs", "panes", "layouts"] {
        let mut bad = good.clone();
        let first = bad[field][0].clone();
        bad[field].as_array_mut().unwrap().push(first);
        bads.push(bad);
    }
    for (collection, key, value) in [
        ("tabs", "workspace_id", json!("missing")),
        ("tabs", "label", json!(null)),
        ("panes", "tab_id", json!("missing")),
        ("panes", "workspace_id", json!("other")),
        ("panes", "terminal_id", json!("")),
        ("panes", "cwd", json!(4)),
        ("layouts", "workspace_id", json!("other")),
        ("layouts", "panes", json!([])),
        ("layouts", "panes", json!([{"pane_id":"missing"}])),
        (
            "layouts",
            "panes",
            json!([{"pane_id":"w1:p1"},{"pane_id":"w1:p1"}]),
        ),
        ("agents", "tab_id", json!("other")),
        ("agents", "workspace_id", json!("other")),
        ("agents", "terminal_id", json!("other")),
    ] {
        let mut bad = good.clone();
        bad[collection][0][key] = value;
        bads.push(bad);
    }
    let mut no_layout = good.clone();
    no_layout["layouts"] = json!([]);
    bads.push(no_layout);
    for bad in bads {
        assert!(HerdrTopology::parse(&bad).is_err(), "accepted {bad}");
    }
}

#[test]
fn frontend_trait_accepts_an_independent_adapter_without_closed_enum_dispatch() {
    struct Plugin;
    impl WorkspaceFrontend for Plugin {
        fn frontend_id(&self) -> FrontendId {
            FrontendId("independent".into())
        }
        fn endpoint_id(&self) -> EndpointId {
            EndpointId("plugin-server".into())
        }
        fn workspaces(&self) -> Result<Vec<Workspace>, FrontendError> {
            Ok(Vec::new())
        }
        fn resolve(&self, _: WorkspaceSelector<'_>) -> Result<Workspace, FrontendError> {
            Err(FrontendError::invalid("not found"))
        }
        fn create(&self, _: &CreateWorkspace) -> Result<Workspace, FrontendError> {
            Err(FrontendError::invalid("unsupported"))
        }
        fn focus(&self, _: &WorkspaceRef) -> Result<(), FrontendError> {
            Err(FrontendError::invalid("unsupported"))
        }
        fn rename(&self, _: &WorkspaceRef, _: &str) -> Result<(), FrontendError> {
            Err(FrontendError::invalid("unsupported"))
        }
    }
    let frontend: Box<dyn WorkspaceFrontend> = Box::new(Plugin);
    assert_eq!(frontend.frontend_id(), FrontendId("independent".into()));
}
