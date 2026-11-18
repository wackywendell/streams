use std::{cell::RefCell, collections::BTreeMap};
use streams::{
    frontend::{EndpointId, FrontendId},
    integration::*,
    model::{Session, Status},
};

fn terminal(key: &str) -> NativeRef {
    NativeRef {
        adapter: FrontendId("test".into()),
        endpoint: EndpointId("owner-core".into()),
        key: key.into(),
    }
}
fn conversation(id: &str) -> ConversationId {
    ConversationId {
        harness: "pi".into(),
        native_id: id.into(),
    }
}
struct Facts {
    facts: BTreeMap<String, Presence>,
    reads: RefCell<Vec<String>>,
}
impl ExecutionSnapshot for Facts {
    fn presence(&self, conversation: &ConversationId) -> Presence {
        self.reads.borrow_mut().push(conversation.native_id.clone());
        self.facts
            .get(&conversation.native_id)
            .expect("only selected supported conversations are inspected")
            .clone()
    }
    fn verified_terminals(&self, _: &ConversationId) -> Vec<NativeRef> {
        panic!("classification does not request extra inspection")
    }
    fn stream_terminals(&self, _: &str) -> Access<Vec<NativeRef>> {
        panic!("classification does not select unrelated work")
    }
    fn foreground_conversation(&self, _: &NativeRef) -> ForegroundConversation {
        panic!("classification is not a foreground query")
    }
}
#[derive(Default)]
struct OnceNative {
    effects: Option<OpenEffects>,
    calls: usize,
    commands: Vec<Vec<String>>,
    directories: Vec<std::path::PathBuf>,
    environments: Vec<BTreeMap<String, String>>,
}
impl OnceNative {
    fn execute_open(&mut self, plan: &PreparedOpen) -> OpenEffects {
        self.calls += 1;
        for launch in &plan.launches {
            match launch {
                LaunchItem::Conversation(launch) => {
                    self.commands.push(launch.command().argv().to_vec());
                    self.directories.push(launch.cwd().into());
                    self.environments.push(launch.environment().clone());
                }
                LaunchItem::Shell { cwd } => self.directories.push(cwd.clone()),
            }
        }
        self.effects.take().expect("opening submits once")
    }
}

fn support(mode: OpeningMode) -> OpeningSupport {
    OpeningSupport {
        harnesses: NonEmpty::new("pi".into(), []),
        mode,
        launch_limit: LaunchLimit::Many,
        placement: Placement::Independent,
    }
}

#[test]
fn classification_reuses_running_work_and_only_absence_permits_a_replacement() {
    let facts = Facts {
        facts: BTreeMap::from([
            (
                "running".into(),
                Presence::Running {
                    locations: NonEmpty::new(terminal("agent"), []),
                },
            ),
            ("absent".into(), Presence::Absent),
            (
                "unknown".into(),
                Presence::Unknown {
                    reason: "incomplete process observation".into(),
                },
            ),
        ]),
        reads: RefCell::new(Vec::new()),
    };
    let requested = vec![
        ("one".into(), conversation("running")),
        ("two".into(), conversation("absent")),
        ("three".into(), conversation("unknown")),
        (
            "four".into(),
            ConversationId {
                harness: "codex".into(),
                native_id: "unsupported".into(),
            },
        ),
    ];
    let (launches, results) = classify_open(
        &support(OpeningMode::ReuseObserved),
        &requested,
        Access::Available { value: &facts },
    )
    .unwrap();
    assert_eq!(launches, vec!["two"]);
    assert!(matches!(
        results["one"],
        SessionOpenResult::AlreadyRunning { .. }
    ));
    assert!(matches!(
        results["three"],
        SessionOpenResult::Skipped { .. }
    ));
    assert!(matches!(results["four"], SessionOpenResult::Skipped { .. }));
    assert_eq!(*facts.reads.borrow(), vec!["running", "absent", "unknown"]);
}

#[test]
fn unavailable_and_unsupported_observation_cannot_become_absence() {
    for execution in [
        Access::<&dyn ExecutionSnapshot>::Unsupported,
        Access::Unavailable {
            reason: "owner unavailable".into(),
        },
    ] {
        let (launches, results) = classify_open(
            &support(OpeningMode::ReuseObserved),
            &[("one".into(), conversation("native"))],
            execution,
        )
        .unwrap();
        assert!(launches.is_empty());
        assert!(matches!(results["one"], SessionOpenResult::Skipped { .. }));
    }
}

#[test]
fn create_only_does_not_claim_observed_absence_and_launch_limit_precedes_inspection() {
    let facts = Facts {
        facts: BTreeMap::new(),
        reads: RefCell::new(Vec::new()),
    };
    let requested = [
        ("one".into(), conversation("one")),
        ("two".into(), conversation("two")),
    ];
    let (launches, results) = classify_open(
        &support(OpeningMode::CreateOnly),
        &requested,
        Access::Available { value: &facts },
    )
    .unwrap();
    assert_eq!(launches, vec!["one", "two"]);
    assert!(results.is_empty());
    assert!(facts.reads.borrow().is_empty());
    let mut limited = support(OpeningMode::ReuseObserved);
    limited.launch_limit = LaunchLimit::One;
    assert!(classify_open(&limited, &requested, Access::Available { value: &facts }).is_err());
    assert!(facts.reads.borrow().is_empty());
}

#[test]
fn duplicate_selection_fails_instead_of_planning_two_effects() {
    let facts = Facts {
        facts: BTreeMap::from([("one".into(), Presence::Absent)]),
        reads: RefCell::new(Vec::new()),
    };
    let requested = [
        ("one".into(), conversation("one")),
        ("one".into(), conversation("one")),
    ];
    assert!(
        classify_open(
            &support(OpeningMode::ReuseObserved),
            &requested,
            Access::Available { value: &facts }
        )
        .is_err()
    );
}

#[test]
fn prepared_application_preserves_partial_effects_exact_inputs_and_does_not_replay() {
    use streams::runtime::conversations::ConversationRoots;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let nested = root.join("nested");
    std::fs::create_dir(&nested).unwrap();
    let environment = BTreeMap::from([
        ("HOME".into(), root.to_str().unwrap().into()),
        ("STREAMS_STREAM_ID".into(), "work".into()),
    ]);
    let prepare = |id: &str, cwd: &std::path::Path| {
        let native = format!("native-{id}");
        let transcript = root.join(format!("{id}.jsonl"));
        std::fs::write(
            &transcript,
            include_str!("fixtures/runtime/pi.jsonl").replace("native-pi", &native),
        )
        .unwrap();
        let session = Session::new(id, "host", "work", "pi", native, id).unwrap();
        LaunchItem::Conversation(
            VerifiedLaunch::prepare(
                &session,
                &transcript,
                &ConversationRoots::default(),
                cwd,
                environment.clone(),
            )
            .unwrap(),
        )
    };
    let space = terminal("space");
    let adopted = terminal("adopted");
    let launched = terminal("launched");
    let presentation = PresentationResult {
        space: SpaceEvidence::Known {
            value: space.clone(),
        },
        state: PresentationState::Incomplete {
            reason: "binding failed".into(),
        },
    };
    let actions = vec![ViewerAction::SelectSpace {
        space: space.clone(),
    }];
    let adoption = AdoptionEffect {
        terminal: adopted.clone(),
        result: Effect::Uncertain {
            reason: "placement reply lost".into(),
        },
    };
    let mut native = OnceNative {
        effects: Some(OpenEffects {
            launches: vec![
                Effect::Applied {
                    value: launched.clone(),
                },
                Effect::Uncertain {
                    reason: "launch reply lost".into(),
                },
                Effect::NotApplied {
                    reason: "stopped after uncertain effect".into(),
                },
            ],
            adoptions: vec![adoption.clone()],
            presentation: presentation.clone(),
            viewer_actions: actions.clone(),
        }),
        ..OnceNative::default()
    };
    let plan = PreparedOpen {
        stream: "work".into(),
        space: SpaceDecision::Reuse { space },
        launches: vec![
            prepare("one", &root),
            prepare("two", &nested),
            prepare("three", &root),
        ],
        adopt: vec![adopted],
        focus: true,
    };
    let running = SessionOpenResult::AlreadyRunning {
        locations: NonEmpty::new(terminal("running"), []),
    };
    let effects = native.execute_open(&plan);
    let report = complete_open(
        plan,
        effects,
        BTreeMap::from([("live".into(), running.clone())]),
    );
    assert_eq!(native.calls, 1);
    assert_eq!(
        native.commands,
        ["one", "two", "three"].map(|id| vec![
            "pi".to_owned(),
            "--session".into(),
            root.join(format!("{id}.jsonl")).to_str().unwrap().into()
        ])
    );
    assert_eq!(native.directories, [root.clone(), nested, root]);
    assert_eq!(native.environments, vec![environment; 3]);
    assert_eq!(
        report.sessions["one"],
        SessionOpenResult::Launched { location: launched }
    );
    assert_eq!(
        report.sessions["two"],
        SessionOpenResult::Uncertain {
            reason: "launch reply lost".into()
        }
    );
    assert_eq!(
        report.sessions["three"],
        SessionOpenResult::Skipped {
            reason: "stopped after uncertain effect".into()
        }
    );
    assert_eq!(report.sessions["live"], running);
    assert_eq!(report.adoptions, vec![adoption]);
    assert_eq!(report.presentation, presentation);
    assert_eq!(report.viewer_actions, actions);
    assert!(report.initial_shell.is_none());
}

#[test]
fn an_initial_shell_keeps_its_own_uncertain_effect_without_a_fictitious_session() {
    let space = terminal("requested");
    let mut native = OnceNative {
        effects: Some(OpenEffects {
            launches: vec![Effect::Uncertain {
                reason: "shell reply lost".into(),
            }],
            adoptions: vec![],
            presentation: PresentationResult {
                space: SpaceEvidence::Requested { value: space },
                state: PresentationState::Uncertain {
                    reason: "inspect requested placement".into(),
                },
            },
            viewer_actions: vec![],
        }),
        ..OnceNative::default()
    };
    let plan = PreparedOpen {
        stream: "work".into(),
        space: SpaceDecision::Create {
            title: "Work".into(),
            cwd: "/verified".into(),
        },
        launches: vec![LaunchItem::Shell {
            cwd: "/verified".into(),
        }],
        adopt: vec![],
        focus: true,
    };
    let effects = native.execute_open(&plan);
    let report = complete_open(plan, effects, BTreeMap::new());
    assert_eq!(native.calls, 1);
    assert!(report.sessions.is_empty());
    assert_eq!(
        report.initial_shell,
        Some(Effect::Uncertain {
            reason: "shell reply lost".into()
        })
    );
    assert!(matches!(
        report.presentation.space,
        SpaceEvidence::Requested { .. }
    ));
}

#[test]
fn shared_views_map_only_addressed_saved_fields_and_require_nullable_wire_fields() {
    let mut session = Session::new("one", "host", "stream", "pi", "native", "Saved title").unwrap();
    session.status = Status::Waiting;
    session.recap = "Saved recap".into();
    session.state = "Saved state".into();
    session.remaining = "Saved next step".into();
    session.state_short = Some("Short state".into());
    let view = SessionView::from(&session);
    assert_eq!(view.title, session.title);
    assert_eq!(view.stream, session.stream_id);
    assert_eq!(view.status, session.status);
    assert_eq!(view.recap, session.recap);
    assert_eq!(view.state, session.state);
    assert_eq!(view.remaining, session.remaining);
    assert_eq!(
        SessionSummary::from(&session).state_short,
        session.state_short
    );
    let mut wire = serde_json::to_value(&view).unwrap();
    assert!(wire["recap_short"].is_null());
    assert_eq!(
        serde_json::from_value::<SessionView>(wire.clone()).unwrap(),
        view
    );
    wire.as_object_mut().unwrap().remove("recap_short");
    assert!(serde_json::from_value::<SessionView>(wire).is_err());
    assert!(serde_json::from_value::<NonEmpty<NativeRef>>(serde_json::json!([])).is_err());
    assert!(serde_json::from_value::<LinkKind>(serde_json::json!({"issue":null})).is_err());
}

#[test]
fn owner_core_and_adapter_are_part_of_exact_resource_identity() {
    let first = terminal("same-key");
    let mut other = first.clone();
    other.endpoint = EndpointId("other-core".into());
    assert_ne!(first, other);
    other = first.clone();
    other.adapter = FrontendId("other".into());
    assert_ne!(first, other);
    assert!(first.validate().is_ok());
    other.key.clear();
    assert!(other.validate().is_err());
}

#[test]
fn herdr_work_evidence_preserves_id_and_path_matching_without_foreground_authority() {
    use serde_json::json;
    use streams::frontend::herdr::HerdrNativeSnapshot;
    let directory = tempfile::tempdir().unwrap();
    let locator = directory.path().join("not-opened.jsonl");
    let scope = WorkScope {
        catalog: "catalog".into(),
        host: "host".into(),
    };
    let snapshot = HerdrNativeSnapshot::parse(&json!({
        "workspaces":[{"workspace_id":"w", "label":"Work", "number":1, "tokens":streams::runtime::native_markers::stream_tokens(&scope.catalog, &scope.host, "work")}],
        "tabs":[{"workspace_id":"w", "tab_id":"a", "label":"A", "number":1}, {"workspace_id":"w", "tab_id":"b", "label":"B", "number":2}, {"workspace_id":"w", "tab_id":"c", "label":"Shell", "number":3}],
        "panes":[{"workspace_id":"w", "tab_id":"a", "pane_id":"pa", "terminal_id":"ta"}, {"workspace_id":"w", "tab_id":"b", "pane_id":"pb", "terminal_id":"tb"}, {"workspace_id":"w", "tab_id":"c", "pane_id":"pc", "terminal_id":"tc"}],
        "layouts":[{"workspace_id":"w", "tab_id":"a", "panes":[{"pane_id":"pa"}]}, {"workspace_id":"w", "tab_id":"b", "panes":[{"pane_id":"pb"}]}, {"workspace_id":"w", "tab_id":"c", "panes":[{"pane_id":"pc"}]}],
        "agents":[
            {"workspace_id":"w", "tab_id":"a", "pane_id":"pa", "terminal_id":"ta", "agent":"pi", "agent_session":{"agent":"pi", "kind":"id", "value":"live"}},
            {"workspace_id":"w", "tab_id":"b", "pane_id":"pb", "terminal_id":"tb", "agent":"pi", "agent_session":{"agent":"pi", "kind":"path", "value":locator}}
        ]
    })).unwrap();
    let wanted = [
        WantedConversation {
            conversation: conversation("live"),
            locator: None,
        },
        WantedConversation {
            conversation: conversation("by-path"),
            locator: Some(locator.clone()),
        },
        WantedConversation {
            conversation: conversation("absent"),
            locator: None,
        },
    ];
    let endpoint = EndpointId("observed-owner".into());
    let evidence = snapshot.work_execution(endpoint.clone(), scope.clone(), "work", &wanted);
    for (native, key) in [("live", "pa"), ("by-path", "pb")] {
        let Presence::Running { locations } = evidence.presence(&conversation(native)) else {
            panic!("reported conversation is running")
        };
        assert_eq!(
            locations.as_slice(),
            &[NativeRef {
                adapter: FrontendId("herdr".into()),
                endpoint: endpoint.clone(),
                key: key.into()
            }]
        );
    }
    assert_eq!(evidence.presence(&conversation("absent")), Presence::Absent);
    assert!(matches!(
        evidence.presence(&conversation("unrequested")),
        Presence::Unknown { .. }
    ));
    let Access::Available {
        value: work_terminals,
    } = evidence.stream_terminals("work")
    else {
        panic!("Herdr work routing was inspected")
    };
    assert_eq!(work_terminals.len(), 3);
    let empty_query = snapshot.work_execution(endpoint.clone(), scope.clone(), "work", &[]);
    assert_eq!(
        empty_query.stream_terminals("work"),
        evidence.stream_terminals("work")
    );
    let foreign = snapshot.work_execution(
        endpoint,
        WorkScope {
            host: "other-host".into(),
            ..scope
        },
        "work",
        &wanted,
    );
    assert_eq!(
        foreign.stream_terminals("work"),
        Access::Available { value: vec![] }
    );
    assert!(matches!(
        evidence.stream_terminals("other"),
        Access::Unavailable { .. }
    ));
    assert!(matches!(
        evidence.foreground_conversation(&terminal("ta")),
        ForegroundConversation::Unknown { .. }
    ));
    assert!(!locator.exists());
}

#[test]
fn prepared_launch_preserves_the_exact_verified_command_cwd_and_approved_values() {
    use streams::runtime::conversations::ConversationRoots;
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let transcript = root.join("saved conversation.jsonl");
    std::fs::write(&transcript, include_str!("fixtures/runtime/pi.jsonl")).unwrap();
    let session =
        Session::new("one", "host", "stream", "pi", "native-pi", "Display title").unwrap();
    let environment = BTreeMap::from([
        ("HOME".into(), root.to_str().unwrap().into()),
        ("STREAMS_STREAM_ID".into(), "stream".into()),
    ]);
    let launch = VerifiedLaunch::prepare(
        &session,
        &transcript,
        &ConversationRoots::default(),
        &root,
        environment.clone(),
    )
    .unwrap();
    assert_eq!(launch.session(), session.id);
    assert_eq!(launch.conversation(), &ConversationId::from(&session));
    assert_eq!(
        launch.command().argv(),
        &["pi", "--session", transcript.to_str().unwrap()]
    );
    assert_eq!(launch.command().locator(), transcript);
    assert_eq!(launch.cwd(), root);
    assert_eq!(launch.environment(), &environment);
    assert!(
        VerifiedLaunch::prepare(
            &session,
            &transcript,
            &ConversationRoots::default(),
            &root.join("missing"),
            environment
        )
        .is_err()
    );
}

#[test]
fn saved_reference_resolution_checks_link_kind_and_retains_moved_artifact_provenance() {
    use streams::{
        application::{self, integration::Streams},
        artifacts::{ArtifactEntry, ArtifactKey},
        config::{Config, Strategy},
        model::{
            Record, Reference as ModelReference, ReferenceRole, ReferenceState, ReferenceTarget,
            ReferenceValue, Stream,
        },
        operations::membership::MoveIntent,
        runtime::{CheckoutRegistry, DirectoryBindings},
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let config = Config::new(
        &BTreeMap::from([("HOME".into(), root.to_str().unwrap().into())]),
        &root,
    )
    .unwrap();
    let records = application::initialize_records(&config, Strategy::None, None, "main").unwrap();
    let host = records.host().as_str();
    let mut stream = Stream::new("work", "Work", None, host).unwrap();
    stream.references.push(
        ModelReference::new(ReferenceValue {
            name: "Issue".into(),
            purpose: "Issue".into(),
            role: ReferenceRole::Dependency,
            state: ReferenceState::Active,
            target: ReferenceTarget::Issue {
                url: "https://example.test/issue/1".into(),
            },
        })
        .unwrap(),
    );
    stream.references.push(
        ModelReference::new(ReferenceValue {
            name: "Review".into(),
            purpose: "Review".into(),
            role: ReferenceRole::Deliverable,
            state: ReferenceState::Active,
            target: ReferenceTarget::Pr {
                url: "https://example.test/pull/2".into(),
            },
        })
        .unwrap(),
    );
    application::create_record(records.records(), &Record::Stream(stream)).unwrap();
    application::create_record(
        records.records(),
        &Record::Stream(Stream::new("next", "Next", None, host).unwrap()),
    )
    .unwrap();
    let mut session = Session::new(
        Session::id_for(host, "pi", "native"),
        host,
        "work",
        "pi",
        "native",
        "Producer",
    )
    .unwrap();
    session.artifacts.insert(
        "work".into(),
        vec![ArtifactEntry {
            name: "result.md".into(),
            description: "Result".into(),
        }],
    );
    application::create_record(records.records(), &Record::Session(session.clone())).unwrap();
    let registry = CheckoutRegistry::new(root.join("registry"), host).unwrap();
    let locations = DirectoryBindings::new(root.join("bindings"), registry.clone());
    let facade = Streams::new(&records);
    let issue = Reference::Link {
        stream: "work".into(),
        link: LinkKey {
            kind: LinkKind::Issue,
            url: "https://example.test/issue/1".into(),
        },
    };
    let resolved = facade
        .resolve_saved_reference(&issue, &registry, &locations)
        .unwrap();
    assert!(
        matches!(resolved, ResolvedTarget::Link {url} if url.as_url().as_str() == "https://example.test/issue/1")
    );
    let wrong_kind = Reference::Link {
        stream: "work".into(),
        link: LinkKey {
            kind: LinkKind::Pr,
            url: "https://example.test/issue/1".into(),
        },
    };
    assert!(
        facade
            .resolve_saved_reference(&wrong_kind, &registry, &locations)
            .is_err()
    );
    let key = ArtifactKey {
        host: host.into(),
        stream: "work".into(),
        session: session.id.clone(),
        name: "result.md".into(),
    };
    let moved = MoveIntent::new(host, "pi", "native", "next").unwrap();
    application::move_session_observed(records.records(), &moved).unwrap();
    let resolved = facade
        .resolve_saved_reference(
            &Reference::Artifact { key: key.clone() },
            &registry,
            &locations,
        )
        .unwrap();
    assert!(matches!(resolved, ResolvedTarget::Artifact {key:found} if found == key));
    let mut foreign = key;
    foreign.host = "other-host".into();
    assert!(
        facade
            .resolve_saved_reference(&Reference::Artifact { key: foreign }, &registry, &locations)
            .is_err()
    );
    let document = Reference::Document {
        stream: "work".into(),
        document: DocumentKey {
            repository: "repo".into(),
            path: "plan.md".into(),
        },
        viewing_session: session.id,
    };
    let error = facade
        .resolve_saved_reference(&document, &registry, &locations)
        .err()
        .unwrap();
    assert!(error.to_string().contains("current member"));
    assert!(
        !registry.path().exists(),
        "link/artifact resolution needs no checkout inventory"
    );
}

#[test]
fn admitted_session_reads_are_saved_only_and_scope_is_not_a_catalog_selector() {
    use std::sync::{Arc, Mutex};
    use streams::{
        application::{self, integration::Streams},
        config::{Config, Strategy},
        file_store::records::RecordIoEvent,
        model::{Record, Stream},
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().canonicalize().unwrap();
    let config = Config::new(
        &BTreeMap::from([("HOME".into(), root.to_str().unwrap().into())]),
        &root,
    )
    .unwrap();
    let records = application::initialize_records(&config, Strategy::None, None, "main").unwrap();
    let host = records.host().as_str();
    application::create_record(
        records.records(),
        &Record::Stream(Stream::new("work", "Work", None, host).unwrap()),
    )
    .unwrap();
    let session = Session::new(
        Session::id_for(host, "pi", "native"),
        host,
        "work",
        "pi",
        "native",
        "Saved title",
    )
    .unwrap();
    application::create_record(records.records(), &Record::Session(session.clone())).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let records = application::open_records_observed(&config, move |event| {
        observed.lock().unwrap().push(event)
    })
    .unwrap();
    let facade = Streams::new(&records);
    let mut wrong = facade.scope().clone();
    wrong.host = "other-host".into();
    events.lock().unwrap().clear();
    assert!(Streams::admit(&records, &wrong).is_err());
    assert!(events.lock().unwrap().is_empty());
    let settings = facade
        .companion_settings(&config, &ConversationId::from(&session))
        .unwrap();
    assert_eq!(settings.mode().unwrap(), streams::config::CardMode::Inherit);
    assert!(!settings.path().exists());
    settings.set(streams::config::CardMode::Manual).unwrap();
    assert_eq!(
        facade
            .companion_settings(&config, &ConversationId::from(&session))
            .unwrap()
            .mode()
            .unwrap(),
        streams::config::CardMode::Manual
    );
    assert!(
        events.lock().unwrap().is_empty(),
        "preferences do not read or mutate catalog records"
    );
    let reply = facade.read_session(&session.id).unwrap();
    assert_eq!(reply.view, SessionView::from(&session));
    assert!(reply.viewer_actions.is_empty());
    assert!(reply.warnings.is_empty());
    let events = events.lock().unwrap();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RecordIoEvent::List(_)))
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RecordIoEvent::Read(_)))
            .count(),
        2,
        "format metadata and one Session, no Stream or roster"
    );
}

#[test]
fn herdr_prepared_effects_keep_partial_launches_and_do_not_observe_or_retry() {
    use crate::support::herdr::{FakeHerdr, Reply};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use streams::{frontend::herdr::HerdrClient, runtime::conversations::ConversationRoots};
    let directory = tempfile::tempdir().unwrap();
    let starts = Arc::new(AtomicUsize::new(0));
    let observed = starts.clone();
    let fake = FakeHerdr::start(
        &directory.path().join("native.sock"),
        move |method, params| match method {
            "workspace.create" => Reply::Result(serde_json::json!({"type":"workspace_created",
            "workspace":{"workspace_id":"w","label":"Work","number":1},
            "tab":{"tab_id":"a","workspace_id":"w"}, "root_pane":{"pane_id":"pa","tab_id":"a","workspace_id":"w"}})),
            "workspace.report_metadata" => Reply::Result(serde_json::json!({"type":"ok"})),
            "tab.create" => Reply::Result(serde_json::json!({"type":"tab_created",
            "tab":{"tab_id":"b","workspace_id":"w"}, "root_pane":{"pane_id":"pb","tab_id":"b","workspace_id":"w"}})),
            "agent.start" => {
                if observed.fetch_add(1, Ordering::SeqCst) == 1 {
                    return Reply::Raw(vec![]);
                }
                Reply::Result(serde_json::json!({"type":"agent_started","agent":{
                "pane_id":params["pane_id"],"tab_id":"a","workspace_id":"w",
                "name":params["name"],"agent":"pi","interactive_ready":true}}))
            }
            _ => panic!("unexpected native call {method}"),
        },
    );
    let client = HerdrClient::connect(
        BTreeMap::from([(
            "HERDR_SOCKET_PATH".into(),
            fake.socket.to_str().unwrap().into(),
        )]),
        0.5,
    )
    .unwrap();
    let mut launches = vec![];
    for id in ["one", "two", "three"] {
        let session = Session::new(id, "host", "work", "pi", id, id).unwrap();
        let locator = directory.path().join(format!("{id}.jsonl"));
        std::fs::write(
            &locator,
            format!("{{\"type\":\"session\",\"id\":\"{id}\"}}\n"),
        )
        .unwrap();
        launches.push(LaunchItem::Conversation(
            VerifiedLaunch::prepare(
                &session,
                &locator,
                &ConversationRoots::default(),
                directory.path(),
                BTreeMap::from([("APPROVED".into(), "exact value".into())]),
            )
            .unwrap(),
        ));
    }
    let plan = PreparedOpen {
        stream: "work".into(),
        space: SpaceDecision::Create {
            title: "Work".into(),
            cwd: directory.path().canonicalize().unwrap(),
        },
        launches,
        adopt: vec![],
        focus: false,
    };
    let submitted = client.execute_prepared_open(
        &plan,
        &WorkScope {
            catalog: "catalog".into(),
            host: "host".into(),
        },
        &BTreeMap::from([("WRONG_FALLBACK".into(), "not for the agent".into())]),
        &AtomicBool::new(false),
    );
    assert!(submitted.error.is_some());
    let report = complete_open(plan, submitted.effects, BTreeMap::new());
    assert!(
        matches!(&report.sessions["one"],SessionOpenResult::Launched {location} if location.key=="pa")
    );
    assert!(matches!(
        &report.sessions["two"],
        SessionOpenResult::Uncertain { .. }
    ));
    assert!(matches!(
        &report.sessions["three"],
        SessionOpenResult::Skipped { .. }
    ));
    assert!(matches!(
        report.presentation.space,
        SpaceEvidence::Known { .. }
    ));
    assert_eq!(starts.load(Ordering::SeqCst), 2);
    assert_eq!(
        fake.methods(),
        [
            "ping",
            "workspace.create",
            "workspace.report_metadata",
            "agent.start",
            "tab.create",
            "agent.start"
        ]
    );
    let requests = fake.requests();
    assert_eq!(requests[1].1["focus"], false);
    assert_eq!(
        requests[1].1["env"],
        serde_json::json!({"APPROVED":"exact value"})
    );
    assert_eq!(requests[4].1["env"], requests[1].1["env"]);
}

#[test]
fn herdr_initial_shell_remains_applied_when_association_reply_is_lost() {
    use crate::support::herdr::{FakeHerdr, Reply};
    use std::sync::atomic::AtomicBool;
    use streams::frontend::herdr::HerdrClient;
    let directory = tempfile::tempdir().unwrap();
    let fake = FakeHerdr::start(
        &directory.path().join("native.sock"),
        |method, _| match method {
            "workspace.create" => Reply::Result(serde_json::json!({"type":"workspace_created",
            "workspace":{"workspace_id":"w","label":"Work","number":1},
            "tab":{"tab_id":"a","workspace_id":"w"},"root_pane":{"pane_id":"pa","tab_id":"a","workspace_id":"w"}})),
            "workspace.report_metadata" => Reply::Raw(vec![]),
            _ => panic!("unexpected {method}"),
        },
    );
    let client = HerdrClient::connect(
        BTreeMap::from([(
            "HERDR_SOCKET_PATH".into(),
            fake.socket.to_str().unwrap().into(),
        )]),
        0.5,
    )
    .unwrap();
    let plan = PreparedOpen {
        stream: "work".into(),
        space: SpaceDecision::Create {
            title: "Work".into(),
            cwd: directory.path().to_path_buf(),
        },
        launches: vec![LaunchItem::Shell {
            cwd: directory.path().to_path_buf(),
        }],
        adopt: vec![],
        focus: false,
    };
    let submitted = client.execute_prepared_open(
        &plan,
        &WorkScope {
            catalog: "catalog".into(),
            host: "host".into(),
        },
        &BTreeMap::new(),
        &AtomicBool::new(false),
    );
    assert!(submitted.error.is_some());
    let report = complete_open(plan, submitted.effects, BTreeMap::new());
    assert!(report.sessions.is_empty());
    assert!(matches!(report.initial_shell, Some(Effect::Applied { .. })));
    assert!(matches!(
        report.presentation.space,
        SpaceEvidence::Known { .. }
    ));
    assert!(matches!(
        report.presentation.state,
        PresentationState::Uncertain { .. }
    ));
    assert_eq!(
        fake.methods(),
        ["ping", "workspace.create", "workspace.report_metadata"]
    );
}
