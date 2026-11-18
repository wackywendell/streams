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
        hc::{
            HcClient, HcEndpoint,
            processes::{ProcessKey, ProcessObserver},
        },
    },
    integration::{Effect, NativeRef, WorkScope},
    runtime::hc::{
        AssociationDocument, AssociationSnapshot, HcOperation, PiSourceClaim, PublishedLayout,
        PublishedPane, SpaceAssociation, prepare_associations,
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
/// A fake process table: `live` PIDs run with the key `process(pid)`, `gaps` cannot be
/// observed, and every other process has exited.
struct Processes {
    live: BTreeSet<u32>,
    gaps: BTreeSet<u32>,
    calls: std::cell::Cell<usize>,
}
impl Processes {
    fn live(pids: impl IntoIterator<Item = u32>) -> Self {
        Self {
            live: pids.into_iter().collect(),
            gaps: BTreeSet::new(),
            calls: std::cell::Cell::new(0),
        }
    }
}
impl ProcessObserver for Processes {
    fn alive(&self, key: &ProcessKey) -> std::io::Result<bool> {
        self.calls.set(self.calls.get() + 1);
        if self.gaps.contains(&key.pid) {
            return Err(std::io::Error::other("observation unavailable"));
        }
        Ok(self.live.contains(&key.pid) && *key == process(key.pid))
    }
}
fn process(pid: u32) -> ProcessKey {
    ProcessKey {
        boot: "08f95cf6-6657-40c4-b5b9-6ee493d934ab".into(),
        pid,
        started: u64::from(pid),
    }
}
fn pi_claim(native_id: &str, helper: u32) -> PiSourceClaim {
    PiSourceClaim {
        version: PiSourceClaim::VERSION,
        conversation: streams::integration::ConversationId {
            harness: "pi".into(),
            native_id: native_id.into(),
        },
        helper: process(helper),
    }
}
/// Terminal labels publishing `claims`, each under its helper's key.
fn claim_labels(claims: &[&PiSourceClaim]) -> Value {
    claims
        .iter()
        .map(|claim| {
            (
                streams::runtime::hc::labels::source_key(&claim.helper),
                Value::String(serde_json::to_string(claim).unwrap()),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
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
fn requested_space_is_bound_only_after_fresh_publication_evidence() {
    use streams::integration::{PresentationState, SpaceEvidence};
    for case in [
        "absent",
        "malformed",
        "lost-read",
        "accepted",
        "occupied",
        "conflict",
        "lost-write",
        "bad-ack",
    ] {
        let mut occupied = row("target");
        occupied.stream = "other".into();
        let before = AssociationSnapshot::decode(
            EndpointId("core".into()),
            7,
            if case == "occupied" {
                document(vec![occupied])
            } else {
                Value::Null
            },
        )
        .unwrap();
        let layout = match case {
            "absent" => json!({"spaces":[]}),
            "malformed" => json!({"spaces":"bad"}),
            _ => json!({"spaces":[{"id":"target"}]}),
        };
        let mut steps = vec![Step(
            "doc.get",
            json!({"name":"herdr-app"}),
            if case == "lost-read" {
                Reply::Lost
            } else {
                Reply::Value(json!({"version":22,"value":layout}))
            },
        )];
        if matches!(case, "accepted" | "conflict" | "lost-write" | "bad-ack") {
            let reply = match case {
                "accepted" => Reply::Value(json!({"version":8})),
                "conflict" => Reply::Reject("conflict"),
                "lost-write" => Reply::Lost,
                _ => Reply::Value(json!({"version":9})),
            };
            steps.push(Step(
                "doc.set",
                json!({"name":"streams","version":7,"value":document(vec![row("target")])}),
                reply,
            ));
        }
        let core = FakeCore::new(steps);
        let mut operation = core.operation();
        let result =
            operation.finish_requested_space(&before, &scope(), "work", &reference("target"));
        if matches!(case, "absent" | "malformed" | "lost-read") {
            assert_eq!(
                result.space,
                SpaceEvidence::Requested {
                    value: reference("target")
                }
            );
        } else {
            assert_eq!(
                result.space,
                SpaceEvidence::Known {
                    value: reference("target")
                }
            );
        }
        match case {
            "accepted" => assert_eq!(result.state, PresentationState::Complete),
            "absent" | "occupied" | "conflict" => {
                assert!(matches!(result.state, PresentationState::Incomplete { .. }))
            }
            "malformed" | "lost-read" => assert!(matches!(
                result.state,
                PresentationState::Unavailable { .. }
            )),
            _ => assert!(matches!(result.state, PresentationState::Uncertain { .. })),
        }
        assert!(!format!("{result:?}").contains("owner-private"));
        drop(operation);
        core.finish();
    }
    let core = FakeCore::new(vec![]);
    let mut operation = core.operation();
    let before = AssociationSnapshot::decode(EndpointId("core".into()), 0, Value::Null).unwrap();
    let mut remote = reference("target");
    remote.endpoint = EndpointId("other-core".into());
    let result = operation.finish_requested_space(&before, &scope(), "work", &remote);
    assert_eq!(result.space, SpaceEvidence::None);
    assert!(matches!(
        result.state,
        PresentationState::Unavailable { .. }
    ));
    assert!(operation.commands.is_empty());
    drop(operation);
    assert_eq!(core.finish().len(), 1);
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
fn pi_source_claims_are_current_only_while_their_helper_runs() {
    use streams::{integration::ForegroundConversation, runtime::hc::registered_foreground};
    let terminal = reference("t1");
    let native = |labels: Value| json!({"address":"t1","state":"live","pid":10,"labels":labels});
    let foreground = |labels: Value, processes: &Processes| {
        registered_foreground(&terminal, native(labels), processes)
    };
    let selected = pi_claim("selected", 40);
    let running = Processes::live([40]);
    assert_eq!(
        foreground(claim_labels(&[&selected]), &running),
        ForegroundConversation::Current(selected.conversation.clone())
    );
    assert_eq!(
        foreground(json!({}), &running),
        ForegroundConversation::NotApplicable
    );
    // A dead helper or a reused PID with another start time leaves a stale label.
    assert_eq!(
        foreground(claim_labels(&[&selected]), &Processes::live([])),
        ForegroundConversation::NotApplicable
    );
    let mut reused = selected.clone();
    reused.helper.started += 1;
    assert_eq!(
        foreground(claim_labels(&[&reused]), &running),
        ForegroundConversation::NotApplicable
    );
    let replacement = pi_claim("replacement", 41);
    assert_eq!(
        foreground(
            claim_labels(&[&selected, &replacement]),
            &Processes::live([41])
        ),
        ForegroundConversation::Current(replacement.conversation.clone())
    );
    // Two current claims on one terminal are for the user to resolve.
    assert!(matches!(
        foreground(
            claim_labels(&[&selected, &replacement]),
            &Processes::live([40, 41])
        ),
        ForegroundConversation::Unknown { .. }
    ));
    let gap = Processes {
        gaps: BTreeSet::from([40]),
        ..Processes::live([])
    };
    assert!(matches!(
        foreground(claim_labels(&[&selected]), &gap),
        ForegroundConversation::Unknown { .. }
    ));
    let key = streams::runtime::hc::labels::source_key(&selected.helper);
    let older = json!({"version":1,"conversation":selected.conversation,"helper":selected.helper});
    let mut extra = serde_json::to_value(&selected).unwrap();
    extra["sdk"] = json!(selected.helper);
    let mut wrong_harness = selected.clone();
    wrong_harness.conversation.harness = "claude".into();
    for unreadable in [
        "not-json".to_string(),
        older.to_string(),
        extra.to_string(),
        serde_json::to_string(&wrong_harness).unwrap(),
    ] {
        assert!(matches!(
            foreground(json!({key.clone():unreadable}), &running),
            ForegroundConversation::Unknown { .. }
        ));
    }
    assert!(matches!(
        registered_foreground(
            &terminal,
            json!({"address":"t1","state":"exited","pid":null,"labels":claim_labels(&[&selected])}),
            &running
        ),
        ForegroundConversation::Unknown { .. }
    ));
}

#[test]
fn source_registration_and_retirement_touch_only_the_helper_key() {
    use streams::runtime::hc::{labels::source_key, sources::SourceRegistration};
    let ours = pi_claim("session", 40);
    let label = source_key(&ours.helper);
    let value = serde_json::to_string(&ours).unwrap();
    let terminal = |labels| json!({"address":"t1","state":"live","pid":10,"labels":labels});
    let connect = |core: &FakeCore| {
        let socket = HcEndpoint::resolve(
            &BTreeMap::from([("HC_SOCKET".into(), core.socket.display().to_string())]),
            core.socket.parent().unwrap(),
        )
        .unwrap();
        HcClient::connect(socket, Some(&EndpointId("core".into())), 5.0, None).unwrap()
    };
    let register = |client: &mut HcClient| {
        SourceRegistration::register(
            client,
            "hint",
            ours.conversation.clone(),
            ours.helper.clone(),
            &std::sync::atomic::AtomicBool::new(false),
        )
    };
    // Another helper's label stays; competing claims are for readers to report.
    let other = claim_labels(&[&pi_claim("other", 41)]);
    let mut saved = other.clone();
    saved[&label] = json!(value);
    let core = FakeCore::new(vec![
        Step(
            "terminal.get",
            json!({"terminal":"hint"}),
            Reply::Value(terminal(other.clone())),
        ),
        Step(
            "terminal.label",
            json!({"terminal":"t1","labels":{label.clone():value.clone()}}),
            Reply::Value(terminal(saved)),
        ),
        Step(
            "terminal.label",
            json!({"terminal":"t1","labels":{label.clone():""}}),
            Reply::Value(terminal(other)),
        ),
    ]);
    let mut client = connect(&core);
    let Effect::Applied { value: source } = register(&mut client) else {
        panic!("registration failed")
    };
    assert_eq!(source.terminal(), "t1");
    assert!(matches!(source.retire(&mut client), Effect::Applied { .. }));
    drop(client);
    assert_eq!(core.finish().len(), 4);

    let core = FakeCore::new(vec![Step(
        "terminal.get",
        json!({"terminal":"hint"}),
        Reply::Value(json!({"address":"t1","state":"exited","pid":null,"labels":{}})),
    )]);
    let mut client = connect(&core);
    assert!(matches!(register(&mut client), Effect::NotApplied { .. }));
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
    let mut client = connect(&core);
    assert!(matches!(register(&mut client), Effect::Uncertain { .. }));
    drop(client);
    assert_eq!(core.finish().len(), 3);
}

#[test]
fn execution_inspects_only_registered_sources_on_the_selected_core() {
    use streams::{
        integration::{
            Access, ConversationId, ExecutionSnapshot, Presence, SessionOpenResult, classify_open,
        },
        runtime::hc::RegisteredSources,
    };
    let processes = Processes::live([40]);
    let claim = pi_claim("saved", 40);
    let conversation = claim.conversation.clone();
    let endpoint = EndpointId("core".into());
    let stale = pi_claim("saved", 41);
    let labels = claim_labels(&[&claim, &stale]);
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
    let support = HcOperation::opening_support();
    let wanted = vec![
        ("running".into(), conversation.clone()),
        (
            "new".into(),
            ConversationId {
                harness: "pi".into(),
                native_id: "other".into(),
            },
        ),
    ];
    let (launches, results) =
        classify_open(&support, &wanted, Access::Available { value: &execution }).unwrap();
    assert_eq!(launches, vec!["new"]);
    assert!(matches!(
        results["running"],
        SessionOpenResult::AlreadyRunning { .. }
    ));
    assert!(matches!(
        execution.stream_terminals("work"),
        Access::Unavailable { .. }
    ));
    let (launches, results) = classify_open(
        &support,
        &wanted,
        Access::Unavailable {
            reason: "unavailable".into(),
        },
    )
    .unwrap();
    assert!(launches.is_empty());
    assert_eq!(results.len(), 2);
    let unsupported = vec![(
        "deferred".into(),
        ConversationId {
            harness: "claude".into(),
            native_id: "saved".into(),
        },
    )];
    let (launches, results) = classify_open(
        &support,
        &unsupported,
        Access::Available { value: &execution },
    )
    .unwrap();
    assert!(launches.is_empty());
    assert!(
        matches!(&results["deferred"], SessionOpenResult::Skipped {reason} if reason.contains("unsupported"))
    );
    let presentation = |layout: Value, view_scope: WorkScope| {
        streams::runtime::hc::HcPresentation::new(
            view_scope,
            AssociationSnapshot::decode(endpoint.clone(), 0, Value::Null).unwrap(),
            PublishedLayout::decode(endpoint.clone(), layout).unwrap(),
            None,
        )
        .unwrap()
    };
    let empty_layout = presentation(json!({"spaces":[]}), scope());
    let placed_layout = presentation(
        json!({"spaces":[{"id":"space","tabs":[{"id":"tab","root":{"type":"block","content":{"terminal":"t1"}}}]}]}),
        scope(),
    );
    let execution = execution.for_scope(scope()).unwrap();
    assert_eq!(
        execution
            .unplaced_selected(&[conversation.clone(), conversation.clone()], &empty_layout)
            .unwrap(),
        vec![reference("t1")]
    );
    assert!(
        execution
            .unplaced_selected(std::slice::from_ref(&conversation), &placed_layout)
            .unwrap()
            .is_empty()
    );
    assert!(
        execution
            .unplaced_selected(
                &[ConversationId {
                    harness: "pi".into(),
                    native_id: "other".into()
                }],
                &empty_layout
            )
            .unwrap()
            .is_empty()
    );
    let other_scope = WorkScope {
        host: "other".into(),
        ..scope()
    };
    assert!(
        execution
            .unplaced_selected(
                std::slice::from_ref(&conversation),
                &presentation(json!({"spaces":[]}), other_scope)
            )
            .is_err()
    );
    // Two current claims on one terminal leave both conversations unknown and unplaced.
    let second = pi_claim("unselected", 43);
    let mixed =
        json!({"address":"t1","state":"live","pid":10,"labels":claim_labels(&[&claim, &second])});
    let shared = RegisteredSources::decode(
        &endpoint,
        json!({"terminals":[mixed]}),
        &Processes::live([40, 43]),
    )
    .unwrap();
    for conversation in [&conversation, &second.conversation] {
        assert!(matches!(
            shared.presence(conversation),
            Presence::Unknown { .. }
        ));
    }
    assert!(
        shared
            .for_scope(scope())
            .unwrap()
            .unplaced_selected(
                &[conversation.clone(), second.conversation.clone()],
                &empty_layout
            )
            .unwrap()
            .is_empty()
    );
    let mut pending = local.clone();
    pending["labels"]["herdr.space"] = json!("pending-space");
    let pending = RegisteredSources::decode(&endpoint, json!({"terminals":[pending]}), &processes)
        .unwrap()
        .for_scope(scope())
        .unwrap();
    assert!(
        pending
            .unplaced_selected(std::slice::from_ref(&conversation), &empty_layout)
            .unwrap()
            .is_empty()
    );
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
    let duplicated = RegisteredSources::decode(
        &endpoint,
        json!({"terminals":[local.clone(),local.clone()]}),
        &processes,
    )
    .unwrap();
    assert!(matches!(
        duplicated.presence(&conversation),
        Presence::Unknown { .. }
    ));
    let mut other_terminal = local.clone();
    other_terminal["address"] = json!("t4");
    let duplicate_route = RegisteredSources::decode(
        &endpoint,
        json!({"terminals":[local.clone(),other_terminal]}),
        &processes,
    )
    .unwrap();
    assert!(matches!(
        duplicate_route.presence(&conversation),
        Presence::Running { .. }
    ));
    assert_eq!(
        duplicate_route.verified_terminals(&conversation),
        vec![reference("t1"), reference("t4")]
    );
    let broken =
        json!({"address":"t3","state":"live","pid":10,"labels":{"streams.pi.bad":"broken"}});
    let partial = RegisteredSources::decode(
        &endpoint,
        json!({"terminals":[local.clone(),broken]}),
        &processes,
    )
    .unwrap();
    assert!(matches!(
        partial.presence(&conversation),
        Presence::Running { .. }
    ));
    let missing = ConversationId {
        harness: "pi".into(),
        native_id: "missing".into(),
    };
    assert!(matches!(
        partial.presence(&missing),
        Presence::Unknown { .. }
    ));
    let (launches, outcomes) = classify_open(
        &support,
        &[
            ("known".into(), conversation.clone()),
            ("unknown".into(), missing),
        ],
        Access::Available { value: &partial },
    )
    .unwrap();
    assert!(launches.is_empty());
    assert!(matches!(
        outcomes["known"],
        SessionOpenResult::AlreadyRunning { .. }
    ));
    assert!(matches!(
        outcomes["unknown"],
        SessionOpenResult::Skipped { .. }
    ));
    let mut mixed = local.clone();
    mixed["labels"]["streams.pi.bad"] = json!("broken");
    let mixed =
        RegisteredSources::decode(&endpoint, json!({"terminals":[mixed]}), &processes).unwrap();
    assert!(matches!(
        mixed.presence(&conversation),
        Presence::Unknown { .. }
    ));
    assert!(mixed.verified_terminals(&conversation).is_empty());
    let gap = Processes {
        gaps: BTreeSet::from([40]),
        ..Processes::live([])
    };
    let unobserved = RegisteredSources::decode(
        &endpoint,
        json!({"terminals":[{"address":"t3","state":"live","pid":10,"labels":claim_labels(&[&claim])}]}),
        &gap,
    )
    .unwrap();
    assert!(matches!(
        unobserved.presence(&conversation),
        Presence::Unknown { .. }
    ));
}

#[test]
fn work_routing_is_scoped_and_separately_unavailable_from_presence() {
    use streams::{
        integration::{Access, ConversationId, ExecutionSnapshot, Presence},
        runtime::hc::RegisteredSources,
    };
    struct NoProcesses;
    impl ProcessObserver for NoProcesses {
        fn alive(&self, _: &ProcessKey) -> std::io::Result<bool> {
            panic!("work labels are routing hints, not process identities")
        }
    }
    let label = |scope: WorkScope, stream: &str| {
        serde_json::to_string(&json!({"version":1,"scope":scope,"stream":stream})).unwrap()
    };
    let terminal =
        |address, labels| json!({"address":address,"state":"live","pid":null,"labels":labels});
    let mut other = scope();
    other.host = "other-host".into();
    let terminals = vec![
        terminal("t1", json!({"streams.work":label(scope(),"work")})),
        terminal("t2", json!({"streams.work":label(other,"work")})),
        terminal("t3", json!({"streams.work":label(scope(),"another")})),
        terminal("t4", json!({})),
        terminal("remote/t5", json!({"streams.work":"invalid"})),
        terminal("t6", json!({"streams.work":label(scope(),"work")})),
    ];
    let endpoint = EndpointId("core".into());
    let sources =
        RegisteredSources::decode(&endpoint, json!({"terminals":terminals}), &NoProcesses).unwrap();
    let work = sources.for_scope(scope()).unwrap();
    assert_eq!(
        work.stream_terminals("work"),
        Access::Available {
            value: vec![reference("t1"), reference("t6")]
        }
    );
    assert_eq!(
        work.stream_terminals("another"),
        Access::Available {
            value: vec![reference("t3")]
        }
    );
    assert_eq!(
        work.stream_terminals("empty"),
        Access::Available { value: vec![] }
    );
    let conversation = ConversationId {
        harness: "pi".into(),
        native_id: "not-registered".into(),
    };
    assert_eq!(work.presence(&conversation), Presence::Absent);
    let bad = RegisteredSources::decode(
        &endpoint,
        json!({"terminals":[terminal("t1",json!({"streams.work":"broken"}))]}),
        &NoProcesses,
    )
    .unwrap()
    .for_scope(scope())
    .unwrap();
    assert!(matches!(
        bad.stream_terminals("work"),
        Access::Unavailable { .. }
    ));
    assert_eq!(bad.presence(&conversation), Presence::Absent);
    for invalid in [
        json!({"version":2,"scope":scope(),"stream":"work"}),
        json!({"version":1,"scope":scope(),"stream":"work","extra":true}),
        json!({"version":1,"scope":scope(),"stream":""}),
    ] {
        let value = serde_json::to_string(&invalid).unwrap();
        let bad = RegisteredSources::decode(
            &endpoint,
            json!({"terminals":[terminal("t1",json!({"streams.work":value}))]}),
            &NoProcesses,
        )
        .unwrap()
        .for_scope(scope())
        .unwrap();
        assert!(matches!(
            bad.stream_terminals("work"),
            Access::Unavailable { .. }
        ));
    }
}

#[test]
fn hc_open_plan_uses_only_selected_work_and_positive_observations() {
    use std::{fs, sync::atomic::AtomicBool};
    use streams::{
        integration::{
            Access, ConversationId, ExecutionSnapshot, LaunchItem, SessionOpenResult, SpaceDecision,
        },
        model::{Session, Stream},
        runtime::{
            CheckoutRegistry, DirectoryBindings,
            conversations::ConversationRoots,
            hc::{HcPresentation, RegisteredSources},
            workspace::{SelectedConversation, WorkspaceRuntime},
        },
    };
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap();
    let registry = CheckoutRegistry::new(cwd.join("checkouts"), "host").unwrap();
    let locations = DirectoryBindings::new(cwd.join("bindings"), registry);
    let stream = Stream::new("work", "Work", None, "host").unwrap();
    let session = |native| {
        Session::new(
            Session::id_for("host", "pi", native),
            "host",
            "work",
            "pi",
            native,
            native,
        )
        .unwrap()
    };
    let running = session("running");
    let absent = session("absent");
    locations
        .remember("work", &cwd, None, true, None, None)
        .unwrap();
    let transcript = cwd.join("saved.jsonl");
    fs::write(&transcript, "{\"type\":\"session\",\"id\":\"absent\"}\n").unwrap();
    let roots = ConversationRoots::default();
    let environment = BTreeMap::from([
        ("HOME".into(), cwd.display().to_string()),
        ("PATH".into(), "/bin:/usr/bin".into()),
    ]);
    let runtime = WorkspaceRuntime::new("host", &locations, None, "catalog", &roots, &environment);
    let processes = Processes::live([40]);
    let endpoint = EndpointId("core".into());
    let claim = PiSourceClaim {
        conversation: (&running).into(),
        ..pi_claim("", 40)
    };
    let labels = claim_labels(&[&claim]);
    let source = json!({"address":"t1","state":"live","pid":10,"labels":labels});
    let work =
        RegisteredSources::decode(&endpoint, json!({"terminals":[source.clone()]}), &processes)
            .unwrap()
            .for_scope(scope())
            .unwrap();
    let present = HcPresentation::new(
        scope(),
        AssociationSnapshot::decode(endpoint.clone(), 0, Value::Null).unwrap(),
        PublishedLayout::decode(endpoint.clone(), json!({"spaces":[]})).unwrap(),
        None,
    )
    .unwrap();
    let foreign_scope = WorkScope {
        host: "other".into(),
        ..scope()
    };
    let foreign = HcPresentation::new(
        foreign_scope,
        AssociationSnapshot::decode(endpoint.clone(), 0, Value::Null).unwrap(),
        PublishedLayout::decode(endpoint.clone(), json!({"spaces":[]})).unwrap(),
        None,
    )
    .unwrap();
    assert!(
        runtime
            .prepare_hc_open(
                &AtomicBool::new(false),
                &stream,
                &[],
                None,
                Access::Available { value: &work },
                Access::Available { value: &foreign }
            )
            .is_err()
    );
    let selected = [
        SelectedConversation {
            session: &running,
            locator: None,
        },
        SelectedConversation {
            session: &absent,
            locator: Some(&transcript),
        },
    ];
    let (plan, outcomes) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &selected,
            None,
            Access::Available { value: &work },
            Access::Available { value: &present },
        )
        .unwrap();
    assert!(matches!(plan.space, SpaceDecision::Create { .. }));
    assert_eq!(plan.adopt, vec![reference("t1")]);
    assert!(
        matches!(&plan.launches[..],[LaunchItem::Conversation(launch)] if launch.session()==absent.id && launch.command().locator()==transcript)
    );
    assert!(matches!(
        outcomes[&running.id],
        SessionOpenResult::AlreadyRunning { .. }
    ));
    assert!(matches!(
        work.presence(&(&absent).into()),
        streams::integration::Presence::Absent
    ));
    let (plan, outcomes) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &selected,
            None,
            Access::Unavailable {
                reason: "/owner/private".into(),
            },
            Access::Available { value: &present },
        )
        .unwrap();
    assert!(plan.launches.is_empty() && plan.adopt.is_empty());
    assert_eq!(outcomes.len(), 2);
    assert!(!format!("{outcomes:?}").contains("/owner/private"));
    let (plan, _) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &selected,
            None,
            Access::Available { value: &work },
            Access::Unavailable {
                reason: "/owner/private".into(),
            },
        )
        .unwrap();
    assert!(
        matches!(&plan.space, SpaceDecision::Unavailable {reason} if !reason.contains("/owner/private"))
    );
    assert_eq!(plan.launches.len(), 1);
    assert!(plan.adopt.is_empty());
    let empty = RegisteredSources::decode(&endpoint, json!({"terminals":[]}), &processes)
        .unwrap()
        .for_scope(scope())
        .unwrap();
    let (plan, _) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &[],
            None,
            Access::Available { value: &empty },
            Access::Available { value: &present },
        )
        .unwrap();
    assert!(matches!(&plan.launches[..], [LaunchItem::Shell { .. }]));
    let (plan, _) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &[],
            None,
            Access::Available { value: &empty },
            Access::Unavailable {
                reason: "closed".into(),
            },
        )
        .unwrap();
    assert!(plan.launches.is_empty());
    let malformed=RegisteredSources::decode(&endpoint,json!({"terminals":[{"address":"t2","state":"live","pid":22,"labels":{"streams.work":"bad"}}]}),&processes).unwrap().for_scope(scope()).unwrap();
    let (plan, _) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &[],
            None,
            Access::Available { value: &malformed },
            Access::Available { value: &present },
        )
        .unwrap();
    assert!(plan.launches.is_empty());
    let routed=RegisteredSources::decode(&endpoint,json!({"terminals":[{"address":"t2","state":"live","pid":22,"labels":{"streams.work":serde_json::to_string(&json!({"version":1,"scope":scope(),"stream":"work"})).unwrap()}}]}),&processes).unwrap().for_scope(scope()).unwrap();
    let (plan, _) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &[],
            None,
            Access::Available { value: &routed },
            Access::Available { value: &present },
        )
        .unwrap();
    assert!(plan.launches.is_empty());
    assert!(matches!(plan.space, SpaceDecision::None));
    let no_locator = [SelectedConversation {
        session: &absent,
        locator: None,
    }];
    let (plan, outcomes) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &no_locator,
            None,
            Access::Available { value: &empty },
            Access::Available { value: &present },
        )
        .unwrap();
    assert!(plan.launches.is_empty());
    assert!(matches!(
        outcomes[&absent.id],
        SessionOpenResult::Skipped { .. }
    ));
    let missing = cwd.join("private-missing.jsonl");
    let invalid = [SelectedConversation {
        session: &absent,
        locator: Some(&missing),
    }];
    let (_, outcomes) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &invalid,
            None,
            Access::Available { value: &empty },
            Access::Available { value: &present },
        )
        .unwrap();
    assert!(
        matches!(&outcomes[&absent.id],SessionOpenResult::Skipped {reason} if !reason.contains("private-missing"))
    );
    use streams::runtime::hc::open::OpenContext;
    let selected = [SelectedConversation {
        session: &running,
        locator: None,
    }];
    let (plan, _) = runtime
        .prepare_hc_open(
            &AtomicBool::new(false),
            &stream,
            &selected,
            None,
            Access::Available { value: &work },
            Access::Available { value: &present },
        )
        .unwrap();
    let core = FakeCore::new(vec![]);
    let socket = HcEndpoint::resolve(
        &BTreeMap::from([("HC_SOCKET".into(), core.socket.display().to_string())]),
        &cwd,
    )
    .unwrap();
    let mut operation = core.operation();
    let effects = operation.execute_prepared_open(
        &plan,
        OpenContext {
            scope: &scope(),
            socket: &socket,
            environment: &environment,
            selected: &[],
            execution: Some(&work),
            presentation: Some(&present),
        },
    );
    assert!(matches!(
        effects.adoptions[0].result,
        Effect::NotApplied { .. }
    ));
    assert!(operation.commands.is_empty());
    drop(operation);
    assert_eq!(core.finish().len(), 1);
    let native = tempfile::Builder::new()
        .prefix("hc-adopt-open-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = native.path().join("s");
    let listener = UnixListener::bind(&path).unwrap();
    let socket = HcEndpoint::resolve(
        &BTreeMap::from([("HC_SOCKET".into(), path.display().to_string())]),
        &cwd,
    )
    .unwrap();
    let server = thread::spawn(move || {
        let mut reader = BufReader::new(listener.accept().unwrap().0);
        reader
            .get_ref()
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let hello: Value = serde_json::from_str(&line).unwrap();
        writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":hello["id"],"result":{"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","terminal.label","doc.get"]}})).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let label: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(label["method"], "terminal.label");
        assert_eq!(label["params"]["terminal"], "t1");
        assert_eq!(label["params"]["labels"].as_object().unwrap().len(), 2);
        writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":label["id"],"result":{"address":"t1","state":"live","labels":label["params"]["labels"]}})).unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let get: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(get["method"], "doc.get");
        writeln!(
            reader.get_mut(),
            "{}",
            json!({"jsonrpc":"2.0","id":get["id"],"result":{"version":0,"value":{"spaces":[]}}})
        )
        .unwrap();
        let mut rest = Vec::new();
        reader.read_to_end(&mut rest).unwrap();
        assert!(rest.is_empty());
    });
    let mut operation =
        HcOperation::new(HcClient::connect(socket.clone(), Some(&endpoint), 5.0, None).unwrap());
    let requested = [ConversationId::from(&running)];
    let effects = operation.execute_prepared_open(
        &plan,
        OpenContext {
            scope: &scope(),
            socket: &socket,
            environment: &environment,
            selected: &requested,
            execution: Some(&work),
            presentation: Some(&present),
        },
    );
    assert!(matches!(
        effects.adoptions[0].result,
        Effect::Applied { .. }
    ));
    assert!(matches!(
        effects.presentation.space,
        streams::integration::SpaceEvidence::Requested { .. }
    ));
    assert!(matches!(
        effects.presentation.state,
        streams::integration::PresentationState::Incomplete { .. }
    ));
    drop(operation);
    server.join().unwrap();
}

#[test]
fn hc_open_effects_keep_execution_and_placement_outcomes_independent() {
    use std::{fs, os::unix::fs::PermissionsExt};
    use streams::{
        integration::{
            LaunchItem, PreparedOpen, PresentationState, SpaceDecision, SpaceEvidence,
            VerifiedLaunch, ViewerAction,
        },
        model::Session,
        runtime::{
            conversations::ConversationRoots,
            hc::{HcPresentation, open::OpenContext},
            workspace::native_environment,
        },
    };
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let program = cwd.join("pi");
    fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let caller = BTreeMap::from([
        ("HOME".into(), cwd.display().to_string()),
        ("PATH".into(), cwd.display().to_string()),
    ]);
    let env = native_environment("work", &caller).unwrap();
    let mut launches = Vec::new();
    for id in ["one", "two", "three"] {
        let session = Session::new(
            Session::id_for("host", "pi", id),
            "host",
            "work",
            "pi",
            id,
            id,
        )
        .unwrap();
        let path = cwd.join(format!("{id}.jsonl"));
        fs::write(&path, format!("{{\"type\":\"session\",\"id\":\"{id}\"}}\n")).unwrap();
        launches.push(LaunchItem::Conversation(
            VerifiedLaunch::prepare(
                &session,
                &path,
                &ConversationRoots::default(),
                &cwd,
                env.clone(),
            )
            .unwrap(),
        ));
    }
    for case in [
        "published",
        "unpublished",
        "lost",
        "rejected",
        "reuse-lost",
        "no-presentation",
        "shell",
        "shell-invalid",
    ] {
        let native = tempfile::Builder::new()
            .prefix("hc-open-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = native.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let socket = HcEndpoint::resolve(
            &BTreeMap::from([("HC_SOCKET".into(), path.display().to_string())]),
            &cwd,
        )
        .unwrap();
        let reuse = case == "reuse-lost";
        let initial = if reuse {
            document(vec![row("target")])
        } else {
            Value::Null
        };
        let layout = if reuse {
            json!({"spaces":[{"id":"target"}]})
        } else {
            json!({"spaces":[]})
        };
        let presentation = HcPresentation::new(
            scope(),
            AssociationSnapshot::decode(EndpointId("core".into()), 3, initial).unwrap(),
            PublishedLayout::decode(EndpointId("core".into()), layout).unwrap(),
            None,
        )
        .unwrap();
        let plan = PreparedOpen {
            stream: "work".into(),
            space: if reuse {
                SpaceDecision::Reuse {
                    space: reference("target"),
                }
            } else if case == "no-presentation" {
                SpaceDecision::Unavailable {
                    reason: "closed".into(),
                }
            } else {
                SpaceDecision::Create {
                    title: "Work".into(),
                    cwd: cwd.clone(),
                }
            },
            launches: if case.starts_with("shell") {
                vec![LaunchItem::Shell { cwd: cwd.clone() }]
            } else {
                launches.clone()
            },
            adopt: vec![],
            focus: true,
        };
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(listener.accept().unwrap().0);
            reader
                .get_ref()
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let hello: Value = serde_json::from_str(&line).unwrap();
            writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":hello["id"],"result":{"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","terminal.run","doc.get","doc.set"]}})).unwrap();
            let count = if case.starts_with("shell") {
                1
            } else if matches!(case, "lost" | "rejected" | "reuse-lost") {
                2
            } else {
                3
            };
            let mut space = String::new();
            let mut tabs = BTreeSet::new();
            for i in 0..count {
                line.clear();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["method"], "terminal.run");
                let params = &request["params"];
                assert_eq!(params["no_parent"], true);
                assert_eq!(params["wait"], false);
                let current = params["labels"]["herdr.space"].as_str().unwrap();
                if i == 0 {
                    space = current.into();
                } else {
                    assert_eq!(current, space);
                }
                assert!(tabs.insert(params["labels"]["herdr.tab"].as_str().unwrap().to_owned()));
                if i == 1 && matches!(case, "lost" | "reuse-lost") {
                    return;
                }
                let argv = match case {
                    "shell" => json!(["/bin/sh"]),
                    "shell-invalid" => Value::Null,
                    _ => params["argv"].clone(),
                };
                if case.starts_with("shell") {
                    assert!(params.get("argv").is_none());
                }
                let response = if i == 1 && case == "rejected" {
                    json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32602,"message":"private","data":{"kind":"invalid_params"}}})
                } else {
                    json!({"jsonrpc":"2.0","id":request["id"],"result":{"address":format!("t{}",i+1),"state":"live","pid":100+i,"argv":argv,"cwd":params["cwd"],"parent":null,"labels":params["labels"]}})
                };
                writeln!(reader.get_mut(), "{}", response).unwrap();
            }
            if matches!(case, "published" | "unpublished" | "shell") {
                line.clear();
                reader.read_line(&mut line).unwrap();
                let get: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(get["method"], "doc.get");
                assert_eq!(get["params"], json!({"name":"herdr-app"}));
                let layout = if matches!(case, "published" | "shell") {
                    json!({"spaces":[{"id":space}]})
                } else {
                    json!({"spaces":[]})
                };
                writeln!(
                    reader.get_mut(),
                    "{}",
                    json!({"jsonrpc":"2.0","id":get["id"],"result":{"version":5,"value":layout}})
                )
                .unwrap();
                if matches!(case, "published" | "shell") {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    let set: Value = serde_json::from_str(&line).unwrap();
                    assert_eq!(set["method"], "doc.set");
                    assert_eq!(set["params"]["version"], 3);
                    assert_eq!(set["params"]["value"]["spaces"][&space]["stream"], "work");
                    writeln!(
                        reader.get_mut(),
                        "{}",
                        json!({"jsonrpc":"2.0","id":set["id"],"result":{"version":4}})
                    )
                    .unwrap();
                }
            }
            let mut rest = Vec::new();
            reader.read_to_end(&mut rest).unwrap();
            assert!(rest.is_empty(), "no retry or unexpected effect");
        });
        let client =
            HcClient::connect(socket.clone(), Some(&EndpointId("core".into())), 5.0, None).unwrap();
        let mut operation = HcOperation::new(client);
        let effects = operation.execute_prepared_open(
            &plan,
            OpenContext {
                scope: &scope(),
                socket: &socket,
                environment: &caller,
                selected: &[],
                execution: None,
                presentation: (case != "no-presentation").then_some(&presentation),
            },
        );
        if case == "shell-invalid" {
            assert!(matches!(effects.launches[0], Effect::Uncertain { .. }));
        } else {
            assert!(matches!(effects.launches[0], Effect::Applied { .. }));
        }
        if matches!(case, "lost" | "reuse-lost") {
            assert!(matches!(effects.launches[1], Effect::Uncertain { .. }));
            assert!(matches!(effects.launches[2], Effect::NotApplied { .. }));
        } else if case == "rejected" {
            assert!(matches!(effects.launches[1], Effect::NotApplied { .. }));
            assert!(matches!(effects.launches[2], Effect::NotApplied { .. }));
        } else if case != "shell-invalid" {
            assert!(
                effects
                    .launches
                    .iter()
                    .all(|effect| matches!(effect, Effect::Applied { .. }))
            );
        }
        match case {
            "published" | "shell" => {
                assert!(matches!(
                    effects.presentation.space,
                    SpaceEvidence::Known { .. }
                ));
                assert_eq!(effects.presentation.state, PresentationState::Complete);
                assert!(matches!(
                    &effects.viewer_actions[..],
                    [ViewerAction::SelectSpace { .. }]
                ));
            }
            "unpublished" => {
                assert!(matches!(
                    effects.presentation.space,
                    SpaceEvidence::Requested { .. }
                ));
                assert!(matches!(
                    effects.presentation.state,
                    PresentationState::Incomplete { .. }
                ));
                assert!(effects.viewer_actions.is_empty());
            }
            "lost" | "shell-invalid" => {
                assert!(matches!(
                    effects.presentation.space,
                    SpaceEvidence::Requested { .. }
                ));
                assert!(matches!(
                    effects.presentation.state,
                    PresentationState::Uncertain { .. }
                ));
            }
            "rejected" => {
                assert!(matches!(
                    effects.presentation.space,
                    SpaceEvidence::Requested { .. }
                ));
                assert!(matches!(
                    effects.presentation.state,
                    PresentationState::Incomplete { .. }
                ));
            }
            "reuse-lost" => {
                assert_eq!(
                    effects.presentation.space,
                    SpaceEvidence::Known {
                        value: reference("target")
                    }
                );
                assert_eq!(effects.presentation.state, PresentationState::Complete);
                assert!(matches!(
                    &effects.viewer_actions[..],
                    [ViewerAction::SelectSpace { .. }]
                ));
            }
            _ => {
                assert!(matches!(
                    effects.presentation.space,
                    SpaceEvidence::Requested { .. }
                ));
                assert!(matches!(
                    effects.presentation.state,
                    PresentationState::Unavailable { .. }
                ));
            }
        }
        drop(operation);
        server.join().unwrap();
    }
    let stale = row("gone");
    let present = HcPresentation::new(
        scope(),
        AssociationSnapshot::decode(EndpointId("core".into()), 4, document(vec![stale])).unwrap(),
        PublishedLayout::decode(EndpointId("core".into()), json!({"spaces":[]})).unwrap(),
        None,
    )
    .unwrap();
    let core = FakeCore::new(vec![Step(
        "doc.set",
        json!({"name":"streams","version":4,"value":document(vec![])}),
        Reply::Value(json!({"version":5})),
    )]);
    let socket = HcEndpoint::resolve(
        &BTreeMap::from([("HC_SOCKET".into(), core.socket.display().to_string())]),
        &cwd,
    )
    .unwrap();
    let mut operation = core.operation();
    let effects = operation.execute_prepared_open(
        &PreparedOpen {
            stream: "work".into(),
            space: SpaceDecision::None,
            launches: vec![],
            adopt: vec![],
            focus: true,
        },
        OpenContext {
            scope: &scope(),
            socket: &socket,
            environment: &caller,
            selected: &[],
            execution: None,
            presentation: Some(&present),
        },
    );
    assert_eq!(effects.presentation.space, SpaceEvidence::None);
    assert!(matches!(
        effects.presentation.state,
        PresentationState::Incomplete { .. }
    ));
    drop(operation);
    assert_eq!(core.finish().len(), 2);
}

#[test]
fn hc_adoption_writes_only_placement_labels_once() {
    use streams::{
        integration::ConversationId,
        runtime::hc::{HcPresentation, RegisteredSources},
    };
    let processes = Processes::live([40]);
    let conversation = ConversationId {
        harness: "pi".into(),
        native_id: "saved".into(),
    };
    let endpoint_id = EndpointId("core".into());
    let claim = PiSourceClaim {
        conversation: conversation.clone(),
        ..pi_claim("", 40)
    };
    let labels = claim_labels(&[&claim]);
    let work = RegisteredSources::decode(
        &endpoint_id,
        json!({"terminals":[{"address":"t1","state":"live","pid":10,"labels":labels}]}),
        &processes,
    )
    .unwrap()
    .for_scope(scope())
    .unwrap();
    let presentation = HcPresentation::new(
        scope(),
        AssociationSnapshot::decode(endpoint_id.clone(), 0, Value::Null).unwrap(),
        PublishedLayout::decode(endpoint_id.clone(), json!({"spaces":[]})).unwrap(),
        None,
    )
    .unwrap();
    for mode in ["confirmed", "rejected", "malformed", "lost"] {
        let native = tempfile::Builder::new()
            .prefix("hc-adopt-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = native.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let socket = HcEndpoint::resolve(
            &BTreeMap::from([("HC_SOCKET".into(), path.display().to_string())]),
            native.path(),
        )
        .unwrap();
        let mut prepared = work
            .prepare_adoptions(
                std::slice::from_ref(&conversation),
                &presentation,
                &reference("space"),
                &socket,
            )
            .unwrap();
        assert_eq!(prepared.len(), 1);
        let prepared = prepared.pop().unwrap();
        assert_eq!(prepared.terminal(), &reference("t1"));
        assert!(uuid::Uuid::parse_str(prepared.tab()).is_ok());
        let labels = json!({"herdr.space":"space","herdr.tab":prepared.tab()});
        let expected = json!({"terminal":"t1","labels":labels});
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(listener.accept().unwrap().0);
            reader
                .get_ref()
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let hello: Value = serde_json::from_str(&line).unwrap();
            writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":hello["id"],"result":{"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","terminal.label"]}})).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "terminal.label");
            assert_eq!(request["params"], expected);
            if mode == "lost" {
                return;
            }
            let reply = if mode == "confirmed" {
                json!({"address":"t1","state":"live","labels":labels})
            } else {
                json!({"address":"t1","state":"live","labels":{}})
            };
            let envelope = if mode == "rejected" {
                json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32602,"message":"private details","data":{"kind":"invalid_params"}}})
            } else {
                json!({"jsonrpc":"2.0","id":request["id"],"result":reply})
            };
            writeln!(reader.get_mut(), "{}", envelope).unwrap();
            let mut rest = Vec::new();
            reader.read_to_end(&mut rest).unwrap();
            assert!(rest.is_empty());
        });
        let client = HcClient::connect(socket, Some(&endpoint_id), 5.0, None).unwrap();
        let mut operation = HcOperation::new(client);
        let effect = operation.adopt_terminal(prepared);
        assert_eq!(effect.terminal, reference("t1"));
        if mode == "confirmed" {
            assert_eq!(
                effect.result,
                Effect::Applied {
                    value: reference("t1")
                }
            );
        } else if mode == "rejected" {
            assert!(matches!(effect.result, Effect::NotApplied { .. }));
        } else {
            assert!(matches!(effect.result, Effect::Uncertain { .. }));
        }
        drop(operation);
        server.join().unwrap();
    }
}

#[test]
fn prepared_hc_terminal_start_keeps_exact_inputs_and_never_replays() {
    use std::{fs, os::unix::fs::PermissionsExt};
    use streams::{
        integration::{LaunchItem, VerifiedLaunch},
        model::Session,
        runtime::{
            conversations::ConversationRoots,
            hc::launch::{PreparedTerminal, StartContext},
        },
    };
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path().canonicalize().unwrap();
    let bin = cwd.join("bin");
    fs::create_dir(&bin).unwrap();
    let program = bin.join("pi");
    fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let transcript = cwd.join("actual.jsonl");
    fs::write(&transcript, "{\"type\":\"session\",\"id\":\"saved\"}\n").unwrap();
    let session = Session::new("session", "host", "work", "pi", "saved", "Saved").unwrap();
    let approved = BTreeMap::from([
        ("HOME".into(), cwd.display().to_string()),
        ("PATH".into(), bin.display().to_string()),
        ("STREAMS_STREAM_ID".into(), "work".into()),
    ]);
    let item = LaunchItem::Conversation(
        VerifiedLaunch::prepare(
            &session,
            &transcript,
            &ConversationRoots::default(),
            &cwd,
            approved.clone(),
        )
        .unwrap(),
    );
    let caller = BTreeMap::from([
        ("HC_SESSION".into(), "chosen".into()),
        ("HC_TERMINAL_ID".into(), "caller".into()),
        ("XDG_RUNTIME_DIR".into(), cwd.display().to_string()),
        ("TOKEN".into(), "never-forward".into()),
    ]);
    for mode in [
        "confirmed",
        "finished",
        "rejected",
        "pending",
        "wrong-argv",
        "wrong-cwd",
        "has-parent",
        "malformed",
        "lost",
    ] {
        let native = tempfile::Builder::new()
            .prefix("hc-start-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = native.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let endpoint = HcEndpoint::resolve(
            &BTreeMap::from([("HC_SOCKET".into(), path.display().to_string())]),
            &cwd,
        )
        .unwrap();
        let placement = reference("new-space");
        let prepared = PreparedTerminal::prepare(
            &item,
            StartContext {
                scope: &scope(),
                stream: "work",
                space: &placement,
                socket: &endpoint,
                environment: &caller,
            },
        )
        .unwrap();
        let tab = prepared.tab().to_owned();
        assert!(uuid::Uuid::parse_str(&tab).is_ok());
        let mut environment = approved.clone();
        environment.insert("HC_SOCKET".into(), path.display().to_string());
        environment.insert("HC_SESSION".into(), "chosen".into());
        environment.insert("XDG_RUNTIME_DIR".into(), cwd.display().to_string());
        let labels = json!({"streams.work":serde_json::to_string(&json!({"version":1,"scope":scope(),"stream":"work"})).unwrap(), "herdr.space":"new-space", "herdr.tab":tab});
        let expected = json!({"argv":[program.display().to_string(),"--session",transcript.display().to_string()],"cwd":cwd,"env":environment,"labels":labels,"no_parent":true,"wait":false});
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(listener.accept().unwrap().0);
            reader
                .get_ref()
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let hello: Value = serde_json::from_str(&line).unwrap();
            writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":hello["id"],"result":{"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","terminal.run"]}})).unwrap();
            line.clear();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "terminal.run");
            assert_eq!(request["params"], expected);
            if mode == "lost" {
                return;
            }
            let mut reply = json!({"address":"t7","state":"live","pid":123,"labels":labels,"argv":expected["argv"],"cwd":expected["cwd"],"parent":null});
            match mode {
                "finished" => {
                    reply["state"] = json!("finished");
                    reply["reason"] = json!("exited");
                    reply["exit_code"] = json!(0);
                }
                "pending" => {
                    reply["pid"] = Value::Null;
                    reply["reason"] = json!("spawn_pending");
                }
                "wrong-argv" => reply["argv"] = json!(["different"]),
                "wrong-cwd" => reply["cwd"] = json!("/different"),
                "has-parent" => reply["parent"] = json!("t0"),
                "malformed" => reply["labels"] = json!({}),
                _ => {}
            };
            let envelope = if mode == "rejected" {
                json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32602,"message":"private details","data":{"kind":"invalid_params"}}})
            } else {
                json!({"jsonrpc":"2.0","id":request["id"],"result":reply})
            };
            writeln!(reader.get_mut(), "{}", envelope).unwrap();
            let mut remaining = Vec::new();
            reader.read_to_end(&mut remaining).unwrap();
            assert!(remaining.is_empty(), "no retry or additional native action");
        });
        let client =
            HcClient::connect(endpoint, Some(&EndpointId("core".into())), 5.0, None).unwrap();
        let mut operation = HcOperation::new(client);
        let result = operation.start_terminal(prepared);
        if mode == "confirmed" || mode == "finished" {
            assert_eq!(
                result,
                Effect::Applied {
                    value: reference("t7")
                }
            );
            assert_eq!(operation.commands[0].outcome, CommandOutcome::Acknowledged);
        } else if mode == "rejected" {
            assert!(matches!(result, Effect::NotApplied { .. }));
        } else {
            assert!(matches!(result, Effect::Uncertain { .. }));
            assert_eq!(operation.commands[0].outcome, CommandOutcome::Uncertain);
        }
        drop(operation);
        server.join().unwrap();
    }
    let core = FakeCore::new(vec![]);
    let different = HcEndpoint::resolve(
        &BTreeMap::from([(
            "HC_SOCKET".into(),
            cwd.join("different.sock").display().to_string(),
        )]),
        &cwd,
    )
    .unwrap();
    let placement = reference("space");
    let prepared = PreparedTerminal::prepare(
        &item,
        StartContext {
            scope: &scope(),
            stream: "work",
            space: &placement,
            socket: &different,
            environment: &caller,
        },
    )
    .unwrap();
    let mut operation = core.operation();
    assert!(matches!(
        operation.start_terminal(prepared),
        Effect::NotApplied { .. }
    ));
    assert!(operation.commands.is_empty());
    drop(operation);
    assert_eq!(core.finish().len(), 1);
    let core = FakeCore::new(vec![]);
    let endpoint = HcEndpoint::resolve(
        &BTreeMap::from([("HC_SOCKET".into(), core.socket.display().to_string())]),
        &cwd,
    )
    .unwrap();
    let prepared = PreparedTerminal::prepare(
        &item,
        StartContext {
            scope: &scope(),
            stream: "work",
            space: &placement,
            socket: &endpoint,
            environment: &caller,
        },
    )
    .unwrap();
    let mut operation = core.operation();
    assert!(matches!(
        operation.start_terminal(prepared),
        Effect::NotApplied { .. }
    ));
    assert!(operation.commands.is_empty());
    drop(operation);
    assert_eq!(core.finish().len(), 1);
}

#[test]
fn terminal_routing_works_when_host_processes_cannot_be_observed() {
    use streams::integration::{Access, ForegroundConversation, TerminalObservation};
    let scope = WorkScope {
        catalog: "catalog".into(),
        host: "host".into(),
    };
    let terminal = NativeRef {
        adapter: FrontendId::hc(),
        endpoint: EndpointId("core".into()),
        key: "t1".into(),
    };
    let space = NativeRef {
        key: "space".into(),
        ..terminal.clone()
    };
    let core = FakeCore::new(vec![
        Step(
            "doc.get",
            json!({"name":"streams"}),
            Reply::Value(
                json!({"version":1,"value":{"version":1,"spaces":{"space":SpaceAssociation {
                    scope: scope.clone(),
                    stream: "work".into(),
                    space,
                }}}}),
            ),
        ),
        Step(
            "doc.get",
            json!({"name":"herdr-app"}),
            Reply::Value(
                json!({"version":2,"value":{"spaces":[{"id":"space","tabs":[{"id":"tab","root":{"type":"block","content":{"terminal":"t1"}}}]}]}}),
            ),
        ),
    ]);
    let mut terminals = streams::runtime::hc::HcTerminals::new(
        core.operation(),
        Err("host processes cannot be observed".into()),
        scope,
    );
    assert_eq!(
        terminals.foreground_conversation(&terminal),
        ForegroundConversation::Unknown {
            reason: "host processes cannot be observed".into()
        }
    );
    assert_eq!(
        terminals.routed_stream(&terminal),
        Access::Available {
            value: Some("work".into())
        }
    );
    drop(terminals);
    assert_eq!(
        core.finish().len(),
        3,
        "no terminal read without an observer"
    );
}

#[test]
fn registered_foreground_reads_only_the_requested_terminal() {
    use streams::integration::ForegroundConversation;
    struct NoProcesses;
    impl ProcessObserver for NoProcesses {
        fn alive(&self, _: &ProcessKey) -> std::io::Result<bool> {
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
    let mut terminals = streams::runtime::hc::HcTerminals::new(
        core.operation(),
        Ok(&NoProcesses),
        WorkScope {
            catalog: "catalog".into(),
            host: "host".into(),
        },
    );
    use streams::integration::TerminalObservation;
    assert_eq!(
        terminals.foreground_conversation(&terminal),
        ForegroundConversation::NotApplicable
    );
    drop(terminals);
    assert_eq!(core.finish().len(), 2);
}
