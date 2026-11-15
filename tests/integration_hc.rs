use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    sync::{Arc, Mutex},
    thread,
};
use streams::{
    frontend::{
        CommandOutcome, EndpointId, FrontendId, NativeRequest,
        hc::{HcClient, HcEndpoint},
    },
    integration::{Effect, NativeRef, WorkScope},
    runtime::hc::{
        AssociationDocument, AssociationSnapshot, HcOperation, PublishedLayout, PublishedPane,
        SpaceAssociation, prepare_associations,
    },
};

pub(super) enum Reply {
    Value(Value),
    Reject(&'static str),
    Lost,
}
pub(super) struct Step(pub &'static str, pub Value, pub Reply);
pub(super) struct FakeCore {
    _directory: tempfile::TempDir,
    pub socket: std::path::PathBuf,
    calls: Arc<Mutex<Vec<Value>>>,
    task: thread::JoinHandle<()>,
}
impl FakeCore {
    pub fn new(steps: Vec<Step>) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("hc-assoc-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let observed = calls.clone();
        let task = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            let steps=std::iter::once(Step("hello",json!({"generation":1,"framed":false}),Reply::Value(json!({"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","doc.get","doc.set"]})))).chain(steps);
            for (index, Step(method, params, reply)) in steps.enumerate() {
                let mut line = String::new();
                assert_ne!(reader.read_line(&mut line).unwrap(), 0, "missing {method}");
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(
                    request,
                    json!({"jsonrpc":"2.0","id":index+1,"method":method,"params":params})
                );
                observed.lock().unwrap().push(request.clone());
                let response = match reply {
                    Reply::Value(value) => {
                        json!({"jsonrpc":"2.0","id":request["id"],"result":value})
                    }
                    Reply::Reject(kind) => {
                        json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"owner-private error","data":{"kind":kind}}})
                    }
                    Reply::Lost => return,
                };
                writeln!(reader.get_mut(), "{response}").unwrap();
            }
            let mut remaining = Vec::new();
            reader.read_to_end(&mut remaining).unwrap();
            assert!(
                remaining.is_empty(),
                "no additional reads, writes or retries"
            );
        });
        Self {
            _directory: directory,
            socket,
            calls,
            task,
        }
    }
    fn operation(&self) -> HcOperation {
        let socket = HcEndpoint::resolve(&BTreeMap::from([(
            "HC_SOCKET".into(),
            self.socket.display().to_string(),
        )]))
        .unwrap();
        HcOperation::new(
            HcClient::connect(socket, Some(&EndpointId("core".into())), 5.0, None).unwrap(),
        )
    }
    pub fn finish(self) -> Vec<Value> {
        self.task.join().unwrap();
        Arc::try_unwrap(self.calls).unwrap().into_inner().unwrap()
    }
}
fn scope() -> WorkScope {
    WorkScope {
        catalog: "catalog".into(),
        host: "host".into(),
    }
}
fn reference(key: &str) -> NativeRef {
    NativeRef {
        adapter: FrontendId::hc(),
        endpoint: EndpointId("core".into()),
        key: key.into(),
    }
}
fn row(key: &str) -> SpaceAssociation {
    SpaceAssociation {
        scope: scope(),
        stream: "work".into(),
        space: reference(key),
    }
}
fn document(rows: Vec<SpaceAssociation>) -> Value {
    serde_json::to_value(AssociationDocument {
        version: 1,
        spaces: rows
            .into_iter()
            .map(|row| (row.space.key.clone(), row))
            .collect(),
    })
    .unwrap()
}
fn read_steps(version: u64, associations: Value, layout: Value) -> Vec<Step> {
    vec![
        Step(
            "doc.get",
            json!({"name":"streams"}),
            Reply::Value(json!({"version":version,"value":associations})),
        ),
        Step(
            "doc.get",
            json!({"name":"herdr-app"}),
            Reply::Value(json!({"version":90,"value":layout})),
        ),
    ]
}

#[test]
fn binding_retains_one_version_and_commits_cleanup_and_binding_in_one_write() {
    let stale = row("gone");
    let mut foreign = row("other");
    foreign.scope.host = "another-host".into();
    let wanted = row("target");
    let desired = document(vec![foreign.clone(), wanted.clone()]);
    let mut steps = read_steps(
        7,
        document(vec![stale, foreign]),
        json!({"spaces":[{"id":"target"},{"id":"other"}]}),
    );
    steps.push(Step(
        "doc.set",
        json!({"name":"streams","version":7,"value":desired}),
        Reply::Value(json!({"version":8})),
    ));
    let core = FakeCore::new(steps);
    let mut operation = core.operation();
    let (before, published) = operation.snapshots().unwrap();
    assert!(matches!(
        operation.finish_associations(&before, &published, Some(&wanted)),
        Effect::Applied { .. }
    ));
    assert_eq!(operation.commands.len(), 3);
    assert_eq!(operation.commands[2].outcome, CommandOutcome::Acknowledged);
    assert!(
        operation
            .commands
            .iter()
            .all(|command| matches!(command.request, NativeRequest::Socket { .. }))
    );
    drop(operation);
    assert_eq!(core.finish().len(), 4);
}
#[test]
fn identical_bindings_are_noops_and_occupied_or_unpublished_spaces_are_not_changed() {
    for case in [
        "identical",
        "occupied-stream",
        "occupied-scope",
        "unpublished",
        "null-layout",
    ] {
        let wanted = row("target");
        let mut current = wanted.clone();
        if case == "occupied-stream" {
            current.stream = "other".into();
        }
        if case == "occupied-scope" {
            current.scope.catalog = "other".into();
        }
        let layout = match case {
            "unpublished" => json!({}),
            "null-layout" => Value::Null,
            _ => json!({"spaces":[{"id":"target"}]}),
        };
        let core = FakeCore::new(read_steps(5, document(vec![current]), layout));
        let mut operation = core.operation();
        let (before, published) = operation.snapshots().unwrap();
        let effect = operation.finish_associations(&before, &published, Some(&wanted));
        assert_eq!(
            matches!(effect, Effect::Applied { .. }),
            case == "identical",
            "{case}: {effect:?}"
        );
        drop(operation);
        assert_eq!(core.finish().len(), 3);
    }
}
#[test]
fn conflicts_are_not_applied_and_lost_or_unconfirmed_writes_are_uncertain_without_retry() {
    for case in [
        "conflict",
        "invalid_params",
        "persistence",
        "lost",
        "bad-ack",
        "accepted",
    ] {
        let wanted = row("target");
        let mut steps = read_steps(0, Value::Null, json!({"spaces":[{"id":"target"}]}));
        let reply = match case {
            "lost" => Reply::Lost,
            "bad-ack" => Reply::Value(json!({"version":42})),
            "accepted" => Reply::Value(json!({"version":1})),
            other => Reply::Reject(other),
        };
        steps.push(Step(
            "doc.set",
            json!({"name":"streams","version":0,"value":document(vec![wanted.clone()])}),
            reply,
        ));
        let core = FakeCore::new(steps);
        let mut operation = core.operation();
        let (before, published) = operation.snapshots().unwrap();
        let effect = operation.finish_associations(&before, &published, Some(&wanted));
        match case {
            "conflict" | "invalid_params" => assert!(matches!(effect, Effect::NotApplied { .. })),
            "accepted" => assert!(matches!(effect, Effect::Applied { .. })),
            _ => assert!(matches!(effect, Effect::Uncertain { .. })),
        }
        assert_eq!(operation.commands.len(), 3);
        drop(operation);
        assert_eq!(core.finish().len(), 4);
    }
}
#[test]
fn invalid_associations_stop_before_layout_and_never_supply_absence_or_a_replacement() {
    for case in ["version", "row-key", "endpoint", "scope", "oversize"] {
        let mut value = document(vec![row("target")]);
        match case {
            "version" => value["version"] = json!(2),
            "row-key" => value["spaces"]["target"]["space"]["key"] = json!("different"),
            "endpoint" => value["spaces"]["target"]["space"]["endpoint"] = json!("other-core"),
            "scope" => value["spaces"]["target"]["scope"]["host"] = json!(""),
            _ => value["padding"] = json!("x".repeat(1024 * 1024)),
        }
        let core = FakeCore::new(vec![Step(
            "doc.get",
            json!({"name":"streams"}),
            Reply::Value(json!({"version":4,"value":value})),
        )]);
        let mut operation = core.operation();
        assert!(operation.snapshots().is_err(), "{case}");
        drop(operation);
        assert_eq!(core.finish().len(), 2);
    }
}
#[test]
fn malformed_document_envelopes_do_not_default_missing_fields_to_empty_documents() {
    for value in [
        json!({}),
        json!({"version":0}),
        json!({"value":null}),
        json!({"version":-1,"value":null}),
        json!({"version":"0","value":null}),
        json!([]),
    ] {
        let core = FakeCore::new(vec![Step(
            "doc.get",
            json!({"name":"streams"}),
            Reply::Value(value.clone()),
        )]);
        let mut operation = core.operation();
        assert!(operation.snapshots().is_err(), "{value}");
        drop(operation);
        assert_eq!(core.finish().len(), 2);
    }
}

#[test]
fn publication_defaults_hidden_order_and_floating_placement_match_the_app_subset() {
    for value in [Value::Null, json!({})] {
        assert!(
            PublishedLayout::decode(EndpointId("core".into()), value)
                .unwrap()
                .spaces
                .is_empty()
        );
    }
    let layout=PublishedLayout::decode(EndpointId("core".into()),json!({"spaces":[{"id":"empty"},{"id":"space","tabs":[{"id":"blank","name":"Named empty tab"},{"id":"tab","floating":[{"ignored":"UI-only"}],"root":{"type":"stack","active":"last","floating":[{"id":"float-a","content":{"terminal":"t3"}}],"children":[{"type":"block","id":"mod","content":{"mod":"files"}},{"type":"row","children":[{"type":"block","hidden":true,"content":{"terminal":"t1"},"floating":[{"id":"float-b","content":{"terminal":"t4"}}]},{"type":"column","children":[{"type":"block","id":"last","content":{"terminal":"t2"}},{"type":"block","content":{"context":"active"}},{"type":"block","content":{"agent":"focused"}}]}]}]}}]}]})).unwrap();
    assert_eq!(layout.spaces[0].name, "");
    assert!(layout.spaces[0].tabs.is_empty());
    assert!(layout.spaces[1].tabs[0].panes.is_empty());
    let tab = &layout.spaces[1].tabs[1];
    assert_eq!(
        tab.panes,
        vec![
            PublishedPane::Mod { key: "mod".into() },
            PublishedPane::Terminal {
                key: "".into(),
                terminal: "t1".into()
            },
            PublishedPane::Terminal {
                key: "last".into(),
                terminal: "t2".into()
            }
        ]
    );
    assert_eq!(
        tab.floating_terminals,
        BTreeSet::from(["t3".into(), "t4".into()])
    );
}
#[test]
fn malformed_or_conflicting_layouts_are_unavailable_and_wrong_endpoint_cannot_bind() {
    let block = |terminal: &str| json!({"type":"block","content":{"terminal":terminal}});
    for value in [
        json!({"spaces":null}),
        json!({"spaces":[{"id":"same"},{"id":"same"}]}),
        json!({"spaces":[{"id":"s","tabs":[{"id":"same"},{"id":"same"}]}]}),
        json!({"spaces":[{}]}),
        json!({"spaces":[{"id":"s","name":null}]}),
        json!({"spaces":[{"id":"s","tabs":[{}]}]}),
        json!({"spaces":[{"id":"s","tabs":[{"id":"tab","root":{}}]}]}),
        json!({"spaces":[{"id":"s","tabs":[{"id":"tab","root":{"type":"row"}}]}]}),
        json!({"spaces":[{"id":"s","tabs":[{"id":"tab","root":{"type":"block"}}]}]}),
        json!({"spaces":[{"id":"s","tabs":[{"id":"tab","root":block("")}]}]}),
        json!({"spaces":[{"id":"s","tabs":[{"id":"tab","root":{"type":"future","children":[]}}]}]}),
        json!({"spaces":[{"id":"s","tabs":[{"id":"one","root":block("t1")},{"id":"two","root":block("t1")}]}]}),
    ] {
        assert!(
            PublishedLayout::decode(EndpointId("core".into()), value.clone()).is_err(),
            "{value}"
        );
    }
    let before = AssociationSnapshot::decode(EndpointId("core".into()), 0, Value::Null).unwrap();
    let published = PublishedLayout::decode(
        EndpointId("different-core".into()),
        json!({"spaces":[{"id":"target"}]}),
    )
    .unwrap();
    assert!(prepare_associations(&before, &published, Some(&row("target"))).is_err());
}
