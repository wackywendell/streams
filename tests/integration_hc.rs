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
            let steps=std::iter::once(Step("hello",json!({"generation":1,"framed":false}),Reply::Value(json!({"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","doc.get","doc.set","terminal.get","terminal.label","terminal.list"]})))).chain(steps);
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
        let socket = HcEndpoint::resolve(
            &BTreeMap::from([("HC_SOCKET".into(), self.socket.display().to_string())]),
            self.socket.parent().unwrap(),
        )
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

#[test]
fn source_route_requires_stable_exact_process_keys_and_terminal_ancestry() {
    use std::{cell::RefCell, io};
    use streams::{
        frontend::hc::processes::{ProcessKey, ProcessObservation, ProcessObserver},
        integration::{ConversationId, ForegroundConversation},
        runtime::hc::{
            PiSourceClaim, labels::source_key, registered_foreground, verify_claim_route,
        },
    };
    struct FakeProcesses {
        rows: BTreeMap<u32, ProcessObservation>,
        counts: RefCell<BTreeMap<u32, usize>>,
        changed: Option<u32>,
    }
    impl ProcessObserver for FakeProcesses {
        fn observe(&self, pid: u32) -> io::Result<ProcessObservation> {
            let mut row = self
                .rows
                .get(&pid)
                .cloned()
                .ok_or_else(|| io::Error::other("missing"))?;
            let mut counts = self.counts.borrow_mut();
            let count = counts.entry(pid).or_default();
            *count += 1;
            if self.changed == Some(pid) && *count > 1 {
                row.key.started = "linux:999".into();
            }
            Ok(row)
        }
    }
    let key = |pid| ProcessKey {
        boot: "08f95cf6-6657-40c4-b5b9-6ee493d934ab".into(),
        pid,
        started: format!("linux:{pid}"),
    };
    let endpoint = EndpointId("core".into());
    let claim = PiSourceClaim {
        version: 1,
        endpoint: endpoint.clone(),
        conversation: ConversationId {
            harness: "pi".into(),
            native_id: "selected".into(),
        },
        terminal_root: key(10),
        sdk: key(30),
        helper: key(40),
    };
    let mut rows = BTreeMap::new();
    for (pid, parent) in [(10, 1), (20, 10), (30, 20), (40, 30)] {
        rows.insert(
            pid,
            ProcessObservation {
                key: key(pid),
                parent_pid: parent,
                process_group: 10,
                stopped: pid == 30,
            },
        );
    }
    let fake = |rows: BTreeMap<_, _>, changed| FakeProcesses {
        rows,
        counts: RefCell::new(BTreeMap::new()),
        changed,
    };
    let label = source_key(&claim.helper).unwrap();
    let value = serde_json::to_string(&claim).unwrap();
    let verify =
        |observer: &FakeProcesses| verify_claim_route(&label, &value, &endpoint, 10, observer);
    assert_eq!(verify(&fake(rows.clone(), None)).unwrap(), claim);
    for pid in [10, 20, 30, 40] {
        assert!(
            verify(&fake(rows.clone(), Some(pid))).is_err(),
            "changing pid {pid}"
        );
        let mut missing = rows.clone();
        missing.remove(&pid);
        assert!(verify(&fake(missing, None)).is_err(), "missing pid {pid}");
    }
    let mut reused = rows.clone();
    reused.get_mut(&30).unwrap().key.started = "linux:999".into();
    assert!(verify(&fake(reused, None)).is_err());
    let mut wrong_parent = rows.clone();
    wrong_parent.get_mut(&40).unwrap().parent_pid = 20;
    assert!(verify(&fake(wrong_parent, None)).is_err());
    let mut cycle = rows.clone();
    cycle.get_mut(&20).unwrap().parent_pid = 30;
    assert!(verify(&fake(cycle, None)).is_err());
    assert!(
        verify_claim_route(
            "streams.pi.wrong",
            &value,
            &endpoint,
            10,
            &fake(rows.clone(), None)
        )
        .is_err()
    );
    assert!(
        verify_claim_route(
            &label,
            &value,
            &EndpointId("other".into()),
            10,
            &fake(rows.clone(), None)
        )
        .is_err()
    );
    assert!(verify_claim_route(&label, &value, &endpoint, 20, &fake(rows.clone(), None)).is_err());
    let duplicate = value.replacen("{", "{\"version\":1,", 1);
    assert!(
        verify_claim_route(&label, &duplicate, &endpoint, 10, &fake(rows.clone(), None)).is_err()
    );

    let terminal = NativeRef {
        adapter: FrontendId::hc(),
        endpoint: endpoint.clone(),
        key: "t1".into(),
    };
    let native = |labels: Value, foreground: Value| json!({"address":"t1","state":"live","pid":10,"labels":labels,"foreground":foreground});
    let labels = json!({label.clone():value});
    let foreground = |rows: BTreeMap<_, _>, native: Value| {
        registered_foreground(&terminal, native, &fake(rows, None))
    };
    let mut running = rows.clone();
    running.get_mut(&30).unwrap().stopped = false;
    assert_eq!(
        foreground(running.clone(), native(labels.clone(), json!({"pid":10}))),
        ForegroundConversation::Current(claim.conversation.clone())
    );
    assert_eq!(
        foreground(running.clone(), native(labels.clone(), json!({"pid":99}))),
        ForegroundConversation::NotApplicable
    );
    assert_eq!(
        foreground(rows.clone(), native(labels.clone(), json!({"pid":10}))),
        ForegroundConversation::NotApplicable
    );
    assert!(matches!(
        foreground(rows.clone(), native(labels.clone(), Value::Null)),
        ForegroundConversation::Unknown { .. }
    ));
    assert!(matches!(
        foreground(
            rows.clone(),
            native(json!({"streams.pi.bad":"not-json"}), json!({"pid":10}))
        ),
        ForegroundConversation::Unknown { .. }
    ));
    assert!(matches!(
        foreground(rows, native(labels, json!({"pid":10}))),
        ForegroundConversation::NotApplicable
    ));
}

#[test]
fn stale_source_requires_positive_helper_exit_evidence() {
    use std::io;
    use streams::{
        frontend::hc::processes::{ProcessKey, ProcessObservation, ProcessObserver},
        integration::{ConversationId, ForegroundConversation},
        runtime::hc::{PiSourceClaim, labels::source_key, registered_foreground},
    };
    struct Processes {
        rows: BTreeMap<u32, ProcessObservation>,
        exited: Option<bool>,
    }
    impl ProcessObserver for Processes {
        fn observe(&self, pid: u32) -> io::Result<ProcessObservation> {
            self.rows
                .get(&pid)
                .cloned()
                .ok_or_else(|| io::Error::other("unavailable"))
        }
        fn exited(&self, key: &ProcessKey) -> io::Result<bool> {
            if key.pid == 40 {
                self.exited.ok_or_else(|| io::Error::other("unavailable"))
            } else {
                Ok(self.observe(key.pid)?.key != *key)
            }
        }
    }
    let key = |pid| ProcessKey {
        boot: "08f95cf6-6657-40c4-b5b9-6ee493d934ab".into(),
        pid,
        started: format!("linux:{pid}"),
    };
    let endpoint = EndpointId("core".into());
    let terminal = NativeRef {
        adapter: FrontendId::hc(),
        endpoint: endpoint.clone(),
        key: "t1".into(),
    };
    let claim = PiSourceClaim {
        version: 1,
        endpoint,
        conversation: ConversationId {
            harness: "pi".into(),
            native_id: "old".into(),
        },
        terminal_root: key(10),
        sdk: key(30),
        helper: key(40),
    };
    let replacement = PiSourceClaim {
        conversation: ConversationId {
            harness: "pi".into(),
            native_id: "new".into(),
        },
        helper: key(41),
        ..claim.clone()
    };
    let rows: BTreeMap<_, _> = [(10, 1), (30, 10), (41, 30)]
        .into_iter()
        .map(|(pid, parent_pid)| {
            (
                pid,
                ProcessObservation {
                    key: key(pid),
                    parent_pid,
                    process_group: 10,
                    stopped: false,
                },
            )
        })
        .collect();
    let labels = json!({source_key(&claim.helper).unwrap():serde_json::to_string(&claim).unwrap(), source_key(&replacement.helper).unwrap():serde_json::to_string(&replacement).unwrap()});
    let native = |labels| json!({"address":"t1","state":"live","pid":10,"labels":labels,"foreground":{"pid":10}});
    assert_eq!(
        registered_foreground(
            &terminal,
            native(labels.clone()),
            &Processes {
                rows: rows.clone(),
                exited: Some(true)
            }
        ),
        ForegroundConversation::Current(replacement.conversation)
    );
    assert!(matches!(
        registered_foreground(
            &terminal,
            native(labels.clone()),
            &Processes {
                rows: rows.clone(),
                exited: None
            }
        ),
        ForegroundConversation::Unknown { .. }
    ));
    assert!(matches!(
        registered_foreground(
            &terminal,
            native(labels),
            &Processes {
                rows: rows.clone(),
                exited: Some(false)
            }
        ),
        ForegroundConversation::Unknown { .. }
    ));
    let malformed = json!({source_key(&claim.helper).unwrap():"not-json"});
    assert!(matches!(
        registered_foreground(
            &terminal,
            native(malformed),
            &Processes {
                rows: rows.clone(),
                exited: Some(true)
            }
        ),
        ForegroundConversation::Unknown { .. }
    ));
    let impossible = PiSourceClaim {
        sdk: claim.helper.clone(),
        ..claim.clone()
    };
    let malformed =
        json!({source_key(&claim.helper).unwrap():serde_json::to_string(&impossible).unwrap()});
    assert!(matches!(
        registered_foreground(
            &terminal,
            native(malformed),
            &Processes {
                rows,
                exited: Some(true)
            }
        ),
        ForegroundConversation::Unknown { .. }
    ));
}

#[test]
fn source_registration_and_retirement_touch_only_the_helper_key() {
    use streams::{
        frontend::hc::processes::{ProcessKey, ProcessObservation, ProcessObserver},
        integration::ConversationId,
        runtime::hc::{PiSourceClaim, labels::source_key, sources::SourceRegistration},
    };
    #[derive(Clone)]
    struct Processes(BTreeMap<u32, ProcessObservation>);
    impl ProcessObserver for Processes {
        fn observe(&self, pid: u32) -> std::io::Result<ProcessObservation> {
            self.0
                .get(&pid)
                .cloned()
                .ok_or_else(|| std::io::Error::other("unavailable"))
        }
    }
    let key = |pid| ProcessKey {
        boot: "08f95cf6-6657-40c4-b5b9-6ee493d934ab".into(),
        pid,
        started: format!("linux:{pid}"),
    };
    let processes = Processes(
        [(10, 1), (30, 10), (40, 30)]
            .into_iter()
            .map(|(pid, parent_pid)| {
                (
                    pid,
                    ProcessObservation {
                        key: key(pid),
                        parent_pid,
                        process_group: 10,
                        stopped: false,
                    },
                )
            })
            .collect(),
    );
    let conversation = ConversationId {
        harness: "pi".into(),
        native_id: "session".into(),
    };
    let claim = PiSourceClaim {
        version: 1,
        endpoint: EndpointId("core".into()),
        conversation: conversation.clone(),
        terminal_root: key(10),
        sdk: key(30),
        helper: key(40),
    };
    let label = source_key(&claim.helper).unwrap();
    let value = serde_json::to_string(&claim).unwrap();
    let terminal = |labels| json!({"address":"t1","state":"live","pid":10,"labels":labels});
    let core = FakeCore::new(vec![
        Step(
            "terminal.get",
            json!({"terminal":"hint"}),
            Reply::Value(terminal(json!({"other":"preserved"}))),
        ),
        Step(
            "terminal.label",
            json!({"terminal":"t1","labels":{label.clone():value.clone()}}),
            Reply::Value(terminal(
                json!({"other":"preserved",label.clone():value.clone()}),
            )),
        ),
        Step(
            "terminal.label",
            json!({"terminal":"t1","labels":{label.clone():""}}),
            Reply::Value(terminal(
                json!({"other":"preserved","streams.pi.replacement":"replacement"}),
            )),
        ),
    ]);
    let socket = HcEndpoint::resolve(
        &BTreeMap::from([("HC_SOCKET".into(), core.socket.display().to_string())]),
        core.socket.parent().unwrap(),
    )
    .unwrap();
    let mut client =
        HcClient::connect(socket, Some(&EndpointId("core".into())), 5.0, None).unwrap();
    let Effect::Applied { value: source } = SourceRegistration::register(
        &mut client,
        "hint",
        conversation.clone(),
        30,
        40,
        &processes,
        &std::sync::atomic::AtomicBool::new(false),
    ) else {
        panic!("registration failed")
    };
    assert!(matches!(source.retire(&mut client), Effect::Applied { .. }));
    drop(client);
    assert_eq!(core.finish().len(), 4);

    let mut wrong = processes.clone();
    wrong.0.get_mut(&40).unwrap().parent_pid = 10;
    let core = FakeCore::new(vec![Step(
        "terminal.get",
        json!({"terminal":"hint"}),
        Reply::Value(terminal(json!({}))),
    )]);
    let socket = HcEndpoint::resolve(
        &BTreeMap::from([("HC_SOCKET".into(), core.socket.display().to_string())]),
        core.socket.parent().unwrap(),
    )
    .unwrap();
    let mut client = HcClient::connect(socket, None, 5.0, None).unwrap();
    assert!(matches!(
        SourceRegistration::register(
            &mut client,
            "hint",
            conversation.clone(),
            30,
            40,
            &wrong,
            &std::sync::atomic::AtomicBool::new(false)
        ),
        Effect::NotApplied { .. }
    ));
    drop(client);
    assert_eq!(core.finish().len(), 2);

    let core = FakeCore::new(vec![
        Step(
            "terminal.get",
            json!({"terminal":"hint"}),
            Reply::Value(terminal(json!({}))),
        ),
        Step(
            "terminal.label",
            json!({"terminal":"t1","labels":{label:value}}),
            Reply::Lost,
        ),
    ]);
    let socket = HcEndpoint::resolve(
        &BTreeMap::from([("HC_SOCKET".into(), core.socket.display().to_string())]),
        core.socket.parent().unwrap(),
    )
    .unwrap();
    let mut client = HcClient::connect(socket, None, 5.0, None).unwrap();
    assert!(matches!(
        SourceRegistration::register(
            &mut client,
            "hint",
            conversation,
            30,
            40,
            &processes,
            &std::sync::atomic::AtomicBool::new(false)
        ),
        Effect::Uncertain { .. }
    ));
    drop(client);
    assert_eq!(core.finish().len(), 3);
}

#[test]
fn execution_inspects_only_registered_sources_on_the_selected_core() {
    use std::{cell::Cell, io};
    use streams::{
        frontend::hc::processes::{ProcessKey, ProcessObservation, ProcessObserver},
        integration::{ConversationId, Presence},
        runtime::hc::{PiSourceClaim, RegisteredSources, labels::source_key},
    };
    struct Processes {
        rows: BTreeMap<u32, ProcessObservation>,
        calls: Cell<usize>,
    }
    impl ProcessObserver for Processes {
        fn observe(&self, pid: u32) -> io::Result<ProcessObservation> {
            self.calls.set(self.calls.get() + 1);
            self.rows
                .get(&pid)
                .cloned()
                .ok_or_else(|| io::Error::other("observation unavailable"))
        }
        fn exited(&self, key: &ProcessKey) -> io::Result<bool> {
            if key.pid == 41 {
                return Ok(true);
            }
            Ok(self.observe(key.pid)?.key != *key)
        }
    }
    let key = |pid| ProcessKey {
        boot: "08f95cf6-6657-40c4-b5b9-6ee493d934ab".into(),
        pid,
        started: format!("linux:{pid}"),
    };
    let processes = Processes {
        rows: [(10, 1), (30, 10), (40, 30)]
            .into_iter()
            .map(|(pid, parent_pid)| {
                (
                    pid,
                    ProcessObservation {
                        key: key(pid),
                        parent_pid,
                        process_group: 10,
                        stopped: pid == 30,
                    },
                )
            })
            .collect(),
        calls: Cell::new(0),
    };
    let conversation = ConversationId {
        harness: "pi".into(),
        native_id: "saved".into(),
    };
    let endpoint = EndpointId("core".into());
    let claim = PiSourceClaim {
        version: 1,
        endpoint: endpoint.clone(),
        conversation: conversation.clone(),
        terminal_root: key(10),
        sdk: key(30),
        helper: key(40),
    };
    let stale = PiSourceClaim {
        helper: key(41),
        ..claim.clone()
    };
    let labels = json!({source_key(&claim.helper).unwrap():serde_json::to_string(&claim).unwrap(), source_key(&stale.helper).unwrap():serde_json::to_string(&stale).unwrap()});
    let unregistered = json!({"address":"t2","state":"live","pid":99,"labels":{}});
    let remote = json!({"address":"other/t7","state":"live","pid":99,"labels":{"streams.pi.invalid":"broken"}});
    let local = json!({"address":"t1","state":"live","pid":10,"labels":labels});
    let result = json!({"terminals":[unregistered.clone(),remote.clone(),local.clone()]});
    let core = FakeCore::new(vec![Step("terminal.list", json!({}), Reply::Value(result))]);
    let mut operation = core.operation();
    let execution = operation.registered_sources(&processes).unwrap();
    assert!(matches!(
        execution.presence(&conversation),
        Presence::Running { .. }
    ));
    assert_eq!(
        execution.verified_terminals(&conversation),
        vec![reference("t1")]
    );
    assert_eq!(
        execution.presence(&ConversationId {
            harness: "pi".into(),
            native_id: "other".into()
        }),
        Presence::Absent
    );
    assert!(matches!(
        execution.presence(&ConversationId {
            harness: "claude".into(),
            native_id: "saved".into()
        }),
        Presence::Unknown { .. }
    ));
    drop(operation);
    assert_eq!(core.finish().len(), 2);
    let before = processes.calls.get();
    let empty = RegisteredSources::decode(
        &endpoint,
        json!({"terminals":[unregistered,remote]}),
        &processes,
    )
    .unwrap();
    assert_eq!(empty.presence(&conversation), Presence::Absent);
    assert_eq!(
        processes.calls.get(),
        before,
        "unregistered and remote terminals need no process reads"
    );
    assert!(
        RegisteredSources::decode(
            &endpoint,
            json!({"terminals":[local.clone(),local]}),
            &processes
        )
        .is_err()
    );
    let competing = PiSourceClaim {
        helper: key(42),
        ..claim.clone()
    };
    let mut rows = processes.rows.clone();
    rows.insert(
        42,
        ProcessObservation {
            key: key(42),
            parent_pid: 30,
            process_group: 10,
            stopped: false,
        },
    );
    let same_sdk = json!({source_key(&claim.helper).unwrap():serde_json::to_string(&claim).unwrap(), source_key(&competing.helper).unwrap():serde_json::to_string(&competing).unwrap()});
    assert!(
        RegisteredSources::decode(
            &endpoint,
            json!({"terminals":[{"address":"t1","state":"live","pid":10,"labels":same_sdk}]}),
            &Processes {
                rows,
                calls: Cell::new(0)
            }
        )
        .is_err()
    );
    assert!(RegisteredSources::decode(&endpoint, json!({"terminals":[{"address":"t3","state":"live","pid":10,"labels":{"streams.pi.bad":"broken"}}]}), &processes).is_err());
    assert!(RegisteredSources::decode(&endpoint, json!({"terminals":[{"address":"t3","state":"live","pid":10,"labels":{source_key(&claim.helper).unwrap():serde_json::to_string(&claim).unwrap()}}]}), &Processes { rows:BTreeMap::new(), calls:Cell::new(0) }).is_err());
}

#[test]
fn registered_foreground_reads_only_the_requested_terminal() {
    use streams::{
        frontend::hc::processes::{ProcessObservation, ProcessObserver},
        integration::ForegroundConversation,
    };
    struct NoProcesses;
    impl ProcessObserver for NoProcesses {
        fn observe(&self, _: u32) -> std::io::Result<ProcessObservation> {
            panic!("an unregistered terminal needs no process inspection")
        }
    }
    let core = FakeCore::new(vec![Step(
        "terminal.get",
        json!({"terminal":"t1"}),
        Reply::Value(
            json!({"address":"t1","state":"live","pid":null,"labels":{},"foreground":null}),
        ),
    )]);
    let terminal = NativeRef {
        adapter: FrontendId::hc(),
        endpoint: EndpointId("core".into()),
        key: "t1".into(),
    };
    let mut operation = core.operation();
    assert_eq!(
        operation.foreground(&terminal, &NoProcesses),
        ForegroundConversation::NotApplicable
    );
    drop(operation);
    assert_eq!(core.finish().len(), 2);
}
