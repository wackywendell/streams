use serde_json::{Value, json};
use streams::integration::*;

fn fixtures() -> Value {
    serde_json::from_str(include_str!("fixtures/integration_v1.json")).unwrap()
}

#[test]
fn owner_open_work_selects_admitted_sessions_and_keeps_paths_off_the_reply() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        thread,
        time::Duration,
    };
    use streams::runtime::{CheckoutRegistry, DirectoryBindings, state::RuntimeState};
    let fixture = SavedFixture::new();
    let root = fixture._directory.path().canonicalize().unwrap();
    let host = fixture.records.host().as_str();
    let registry = CheckoutRegistry::new(fixture.config.checkout_state_dir(), host).unwrap();
    let state = RuntimeState::new(
        fixture
            .config
            .runtime_state_dir_for(fixture.records.root())
            .unwrap(),
        host,
    )
    .unwrap();
    let locations = DirectoryBindings::new(state.root(), registry);
    let session = &fixture.sessions[0];
    locations
        .remember("work", &root, Some(&session.id), false, None, None)
        .unwrap();
    let transcript = root.join("private-session.jsonl");
    std::fs::write(&transcript, "{\"type\":\"session\",\"id\":\"one\"}\n").unwrap();
    locations
        .remember_transcript("work", &session.id, &transcript)
        .unwrap();
    let program = root.join("pi");
    std::fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let body = json!({"kind":"act","action":{"kind":"open_work","request":{"stream":"work","sessions":{"kind":"only","value":[session.id]},"cwd":null}}});
    for presentation in [true, false] {
        let socket_dir = tempfile::Builder::new()
            .prefix("hc-cli-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = socket_dir.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let owner_cwd = root.clone();
        let owner_program = program.clone();
        let owner_transcript = transcript.clone();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(listener.accept().unwrap().0);
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let read = |reader: &mut BufReader<std::os::unix::net::UnixStream>| {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                serde_json::from_str::<Value>(&line).unwrap()
            };
            let hello = read(&mut reader);
            writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":hello["id"],"result":{"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","terminal.list","terminal.run","doc.get","doc.set"]}})).unwrap();
            let list = read(&mut reader);
            assert_eq!(list["method"], "terminal.list");
            writeln!(
                reader.get_mut(),
                "{}",
                json!({"jsonrpc":"2.0","id":list["id"],"result":{"terminals":[]}})
            )
            .unwrap();
            let associations = read(&mut reader);
            assert_eq!(associations["params"]["name"], "streams");
            writeln!(
                reader.get_mut(),
                "{}",
                json!({"jsonrpc":"2.0","id":associations["id"],"result":{"version":0,"value":null}})
            )
            .unwrap();
            let layout = read(&mut reader);
            assert_eq!(layout["params"]["name"], "herdr-app");
            let value = if presentation {
                json!({"spaces":[]})
            } else {
                json!(42)
            };
            writeln!(
                reader.get_mut(),
                "{}",
                json!({"jsonrpc":"2.0","id":layout["id"],"result":{"version":1,"value":value}})
            )
            .unwrap();
            let run = read(&mut reader);
            assert_eq!(run["method"], "terminal.run");
            let params = &run["params"];
            assert_eq!(params["cwd"], owner_cwd.to_str().unwrap());
            assert_eq!(params["argv"][0], owner_program.to_str().unwrap());
            assert!(
                params["argv"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| value == owner_transcript.to_str().unwrap())
            );
            assert!(params["labels"].get("streams.work").is_some());
            let space = params["labels"]["herdr.space"].as_str().unwrap();
            writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":run["id"],"result":{"address":"t1","state":"live","pid":123,"argv":params["argv"],"cwd":params["cwd"],"parent":null,"labels":params["labels"]}})).unwrap();
            if presentation {
                let get = read(&mut reader);
                assert_eq!(get["params"]["name"], "herdr-app");
                writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":get["id"],"result":{"version":2,"value":{"spaces":[{"id":space}]}}})).unwrap();
                let set = read(&mut reader);
                assert_eq!(set["method"], "doc.set");
                assert_eq!(set["params"]["version"], 0);
                writeln!(
                    reader.get_mut(),
                    "{}",
                    json!({"jsonrpc":"2.0","id":set["id"],"result":{"version":1}})
                )
                .unwrap();
            }
            let mut rest = Vec::new();
            reader.read_to_end(&mut rest).unwrap();
            assert!(rest.is_empty());
        });
        let mut envelope = fixture.envelope(body.clone());
        envelope["endpoint"] = json!("core");
        let output = fixture
            .command()
            .env("HC_SOCKET", &path)
            .env("PATH", &root)
            .args([
                "integration",
                "request",
                "--request-json",
                &envelope.to_string(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let raw = String::from_utf8(output.stdout).unwrap();
        assert!(!raw.contains(root.to_str().unwrap()));
        assert!(!raw.contains("private-session.jsonl"));
        let reply: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(reply["verified_endpoint"], "core");
        let report = &reply["result"]["ok"]["value"];
        assert_eq!(report["sessions"][&session.id]["kind"], "launched");
        assert_eq!(report["sessions"].as_object().unwrap().len(), 1);
        assert_eq!(
            report["presentation"]["space"]["kind"],
            if presentation { "known" } else { "requested" }
        );
        assert_eq!(
            report["presentation"]["state"]["kind"],
            if presentation {
                "complete"
            } else {
                "unavailable"
            }
        );
        server.join().unwrap();
    }
    let mut path_request = fixture.envelope(body);
    path_request["body"]["action"]["request"]["cwd"] = json!("/private-owner-path");
    let output = fixture.invoke(&path_request.to_string(), false);
    assert!(output.status.success());
    let reply: String = String::from_utf8(output.stdout).unwrap();
    assert!(!reply.contains("/private-owner-path"));
    assert!(reply.contains("desktop opening cannot supply an owner directory"));
}

#[test]
fn hc_public_open_and_start_preserve_runtime_shapes_and_current_lookup() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        thread,
        time::Duration,
    };
    use streams::runtime::{CheckoutRegistry, DirectoryBindings, state::RuntimeState};
    let fixture = SavedFixture::new();
    let root = fixture._directory.path().canonicalize().unwrap();
    let host = fixture.records.host().as_str();
    let scope = WorkScope {
        catalog: fixture.records.source_id().into(),
        host: host.into(),
    };
    let registry = CheckoutRegistry::new(fixture.config.checkout_state_dir(), host).unwrap();
    let state = RuntimeState::new(
        fixture
            .config
            .runtime_state_dir_for(fixture.records.root())
            .unwrap(),
        host,
    )
    .unwrap();
    let locations = DirectoryBindings::new(state.root(), registry);
    let session = &fixture.sessions[0];
    locations
        .remember("work", &root, Some(&session.id), false, None, None)
        .unwrap();
    locations
        .remember("empty", &root, None, false, None, None)
        .unwrap();
    let transcript = root.join("one.jsonl");
    std::fs::write(&transcript, "{\"type\":\"session\",\"id\":\"one\"}\n").unwrap();
    locations
        .remember_transcript("work", &session.id, &transcript)
        .unwrap();
    let program = root.join("pi");
    std::fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

    for case in ["none", "requested", "known", "current-shell"] {
        let socket_dir = tempfile::Builder::new()
            .prefix("hc-public-open-")
            .tempdir_in("/tmp")
            .unwrap();
        let path = socket_dir.path().join("s");
        let listener = UnixListener::bind(&path).unwrap();
        let scope_for_server = scope.clone();
        let server = thread::spawn(move || {
            let mut reader = BufReader::new(listener.accept().unwrap().0);
            reader
                .get_ref()
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let read = |reader: &mut BufReader<std::os::unix::net::UnixStream>| {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                serde_json::from_str::<Value>(&line).unwrap()
            };
            let hello = read(&mut reader);
            writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":hello["id"],"result":{"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","terminal.list","terminal.run","doc.get","doc.set"]}})).unwrap();
            if case == "current-shell" {
                let associations = read(&mut reader);
                assert_eq!(associations["params"]["name"], "streams");
                writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":associations["id"],"result":{"version":2,"value":{"version":1,"spaces":{"space-work":{"scope":scope_for_server,"stream":"work","space":{"adapter":"hc","endpoint":"core","key":"space-work"}}}}}})).unwrap();
                let layout = read(&mut reader);
                assert_eq!(layout["params"]["name"], "herdr-app");
                writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":layout["id"],"result":{"version":3,"value":{"spaces":[{"id":"space-work","tabs":[{"id":"tab-1","root":{"type":"block","content":{"terminal":"t-caller"}}}]}]}}})).unwrap();
                let list = read(&mut reader);
                assert_eq!(list["method"], "terminal.list");
                writeln!(
                    reader.get_mut(),
                    "{}",
                    json!({"jsonrpc":"2.0","id":list["id"],"result":{"terminals":[]}})
                )
                .unwrap();
                let run = read(&mut reader);
                assert_eq!(run["method"], "terminal.run");
                let params = &run["params"];
                assert_eq!(params["labels"]["herdr.space"], "space-work");
                writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":run["id"],"result":{"address":"t1","state":"live","pid":123,"argv":params["argv"],"cwd":params["cwd"],"parent":null,"labels":params["labels"]}})).unwrap();
            } else {
                let list = read(&mut reader);
                assert_eq!(list["method"], "terminal.list");
                writeln!(
                    reader.get_mut(),
                    "{}",
                    json!({"jsonrpc":"2.0","id":list["id"],"result":{"terminals":[]}})
                )
                .unwrap();
                let associations = read(&mut reader);
                assert_eq!(associations["params"]["name"], "streams");
                writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":associations["id"],"result":{"version":0,"value":null}})).unwrap();
                let layout = read(&mut reader);
                assert_eq!(layout["params"]["name"], "herdr-app");
                writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":layout["id"],"result":{"version":1,"value":{"spaces":[]}}})).unwrap();
                if case != "none" {
                    let run = read(&mut reader);
                    assert_eq!(run["method"], "terminal.run");
                    let params = &run["params"];
                    let space = params["labels"]["herdr.space"].as_str().unwrap();
                    writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":run["id"],"result":{"address":"t1","state":"live","pid":123,"argv":params["argv"],"cwd":params["cwd"],"parent":null,"labels":params["labels"]}})).unwrap();
                    let get = read(&mut reader);
                    assert_eq!(get["params"]["name"], "herdr-app");
                    let published = if case == "known" {
                        json!({"spaces":[{"id":space}]})
                    } else {
                        json!({"spaces":[]})
                    };
                    writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":get["id"],"result":{"version":2,"value":published}})).unwrap();
                    if case == "known" {
                        let set = read(&mut reader);
                        assert_eq!(set["method"], "doc.set");
                        writeln!(
                            reader.get_mut(),
                            "{}",
                            json!({"jsonrpc":"2.0","id":set["id"],"result":{"version":1}})
                        )
                        .unwrap();
                    }
                }
            }
            let mut rest = Vec::new();
            reader.read_to_end(&mut rest).unwrap();
            assert!(rest.is_empty());
        });
        let mut command = fixture.command();
        command.env("HC_SOCKET", &path).env("PATH", &root);
        if case == "current-shell" {
            let target = format!("{host}/{}", session.id);
            command.env("HC_TERMINAL_ID", "t-caller").args([
                "open",
                "--current",
                "--session-key",
                &target,
                "--json",
            ]);
        } else {
            let target = format!(
                "{host}/{}",
                if case == "none" {
                    &fixture.sessions[1].id
                } else {
                    &session.id
                }
            );
            command.env("STREAMS_STREAM_ID", "work").args([
                "open",
                "--current",
                "--session-key",
                &target,
                "--frontend",
                "hc",
                "--json",
            ]);
        }
        let output = command.output().unwrap();
        assert_eq!(output.status.code(), Some(2));
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["runtime"]["backend"], "hc", "{case}: {value}");
        let report = &value["runtime"]["report"];
        let attention = value["runtime"]["needs_attention"].as_array().unwrap();
        match case {
            "none" => {
                assert_eq!(report["presentation"]["space"]["kind"], "none");
                assert!(
                    attention
                        .iter()
                        .any(|item| item.as_str().unwrap().contains("Session"))
                );
            }
            "requested" => {
                assert_eq!(report["presentation"]["space"]["kind"], "requested");
                assert!(attention.iter().any(|item| {
                    item.as_str()
                        .unwrap()
                        .contains("streams integration bind-space --space-ref")
                }));
            }
            "known" => {
                assert_eq!(report["presentation"]["space"]["kind"], "known");
                assert_eq!(report["presentation"]["state"]["kind"], "complete");
                assert!(
                    attention
                        .iter()
                        .any(|item| item.as_str().unwrap().contains("select space"))
                );
            }
            _ => {
                assert_eq!(value["stream_id"], "work");
                assert_eq!(
                    report["presentation"]["space"]["value"]["key"],
                    "space-work"
                );
            }
        }
        server.join().unwrap();
    }

    let missing_socket = root.join("missing.sock");
    let output = fixture
        .command()
        .env("HC_SOCKET", &missing_socket)
        .args([
            "start",
            "Saved with hc failure",
            "--frontend",
            "hc",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let saved: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(saved["save"]["local"], "persisted");
    assert!(saved["completion_error"]["message"].is_string());
}

fn saved_presentation(scope: &WorkScope) -> streams::runtime::hc::HcPresentation {
    use streams::{
        frontend::EndpointId,
        runtime::hc::{AssociationSnapshot, HcPresentation, PublishedLayout},
    };
    let endpoint = EndpointId("core".into());
    let before=AssociationSnapshot::decode(endpoint.clone(),7,json!({"version":1,"spaces":{"target":{"scope":scope,"stream":"work","space":{"adapter":"hc","endpoint":"core","key":"target"}}}})).unwrap();
    let layout=PublishedLayout::decode(endpoint,json!({"spaces":[{"id":"target","tabs":[{"id":"tab","root":{"type":"block","content":{"terminal":"t1"}}}]},{"id":"outside","tabs":[{"id":"other-tab","root":{"type":"block","content":{"terminal":"t2"}}}]}]})).unwrap();
    HcPresentation::new(scope.clone(), before, layout, None).unwrap()
}

enum PointOnlyExecution {
    Current(&'static str),
    NotApplicable,
    Unknown,
}
impl ExecutionSnapshot for PointOnlyExecution {
    fn presence(&self, _: &ConversationId) -> Presence {
        panic!("sidebar must not inspect global presence")
    }
    fn verified_terminals(&self, _: &ConversationId) -> Vec<NativeRef> {
        panic!("sidebar must not inspect global execution")
    }
    fn stream_terminals(&self, _: &str) -> Access<Vec<NativeRef>> {
        panic!("sidebar must not inspect global execution")
    }
    fn foreground_conversation(&self, terminal: &NativeRef) -> ForegroundConversation {
        assert_eq!(terminal.key, "t1", "inspect only the in-space candidate");
        match self {
            Self::Current(native) => ForegroundConversation::Current(ConversationId {
                harness: "pi".into(),
                native_id: (*native).into(),
            }),
            Self::NotApplicable => ForegroundConversation::NotApplicable,
            Self::Unknown => ForegroundConversation::Unknown {
                reason: "foreground evidence is unavailable".into(),
            },
        }
    }
}

struct SavedFixture {
    _directory: tempfile::TempDir,
    config: streams::config::Config,
    records: streams::application::OpenedRecords,
    sessions: Vec<streams::model::Session>,
}
impl SavedFixture {
    fn command(&self) -> std::process::Command {
        let root = self._directory.path().canonicalize().unwrap();
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_streams"));
        command
            .env_clear()
            .env("HOME", &root)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(root);
        command
    }
    fn envelope(&self, body: Value) -> Value {
        json!({"version":1,"scope":{"catalog":self.records.source_id(),"host":self.records.host().as_str()},"endpoint":"unconnected-core","body":body})
    }
    fn invoke(&self, request: &str, global_json: bool) -> std::process::Output {
        let mut command = self.command();
        if global_json {
            command.arg("--json");
        }
        command
            .args(["integration", "request", "--request-json", request])
            .output()
            .unwrap()
    }
    fn new() -> Self {
        use streams::{
            application,
            artifacts::ArtifactEntry,
            config::{Config, Strategy},
            model::{Record, Session, Stream},
        };
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let config = Config::new(
            &std::collections::BTreeMap::from([("HOME".into(), root.to_str().unwrap().into())]),
            &root,
        )
        .unwrap();
        let records =
            application::initialize_records(&config, Strategy::None, None, "main").unwrap();
        let host = records.host().as_str();
        for id in ["work", "next", "empty"] {
            let mut stream = Stream::new(id, id, None, host).unwrap();
            if id == "work" {
                stream.goal = Some("Saved full goal".into());
                stream.goal_short = Some("Saved short goal".into());
                stream.repositories.insert(
                    "source".into(),
                    streams::model::Repository::new("https://example.test/source.git").unwrap(),
                );
                stream.references = ["z.md", "a.md"]
                    .into_iter()
                    .map(|path| {
                        streams::model::Reference::new(
                            streams::model::ReferenceValue {
                                name: path.into(),
                                purpose: format!("Saved {path}"),
                                role: streams::model::ReferenceRole::Deliverable,
                                state: streams::model::ReferenceState::Active,
                                target: streams::model::ReferenceTarget::Document(
                                    streams::model::RepositoryDocument {
                                        repository_url: "https://example.test/source.git".into(),
                                        path: path.into(),
                                        r#ref: Some("saved-ref".into()),
                                        commit: Some("saved-commit".into()),
                                        source_session_id: None,
                                    },
                                ),
                            },
                            format!("rev-{path}"),
                        )
                        .unwrap()
                    })
                    .collect();
                stream.references.push(
                    streams::model::Reference::new(
                        streams::model::ReferenceValue {
                            name: "Saved issue".into(),
                            purpose: "Saved issue".into(),
                            role: streams::model::ReferenceRole::Dependency,
                            state: streams::model::ReferenceState::Active,
                            target: streams::model::ReferenceTarget::Issue {
                                url: "https://example.test/shared-issue".into(),
                            },
                        },
                        "rev-issue",
                    )
                    .unwrap(),
                );
                stream.references.push(
                    streams::model::Reference::new(
                        streams::model::ReferenceValue {
                            name: "Saved PR".into(),
                            purpose: "Saved PR".into(),
                            role: streams::model::ReferenceRole::Deliverable,
                            state: streams::model::ReferenceState::Active,
                            target: streams::model::ReferenceTarget::Pr {
                                url: "https://example.test/shared-pr".into(),
                            },
                        },
                        "rev-pr",
                    )
                    .unwrap(),
                );
            }
            application::create_record(records.records(), id, &Record::Stream(stream)).unwrap();
        }
        let mut sessions = vec![];
        for (native, title) in [("one", "Straße"), ("two", "STRASSE"), ("moved", "Moved")] {
            let mut session = Session::new(
                Session::id_for(host, "pi", native),
                host,
                "work",
                "pi",
                native,
                title,
            )
            .unwrap();
            session.recap = format!("Saved recap for {native}");
            session.artifacts.insert(
                "work".into(),
                vec![ArtifactEntry {
                    name: "result.md".into(),
                    description: "Saved output".into(),
                }],
            );
            application::create_record(
                records.records(),
                native,
                &Record::Session(session.clone()),
            )
            .unwrap();
            sessions.push(session);
        }
        application::move_session_observed(
            records.records(),
            "move",
            &streams::operations::membership::MoveIntent::new(host, "pi", "moved", "next").unwrap(),
        )
        .unwrap();
        Self {
            _directory: directory,
            config,
            records,
            sessions,
        }
    }
}

#[test]
fn work_selection_uses_exact_members_or_active_local_sessions() {
    use std::sync::{Arc, Mutex};
    use streams::{
        application::{self, integration::Streams},
        model::{Record, Session, Status},
        yaml_store::records::RecordIoEvent,
    };
    let fixture = SavedFixture::new();
    let host = fixture.records.host().as_str();
    let mut done = Session::new(
        Session::id_for(host, "pi", "done"),
        host,
        "work",
        "pi",
        "done",
        "Done",
    )
    .unwrap();
    done.status = Status::Done;
    application::create_record(
        fixture.records.records(),
        "done",
        &Record::Session(done.clone()),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let records = application::open_records_observed(&fixture.config, move |event| {
        observed.lock().unwrap().push(event)
    })
    .unwrap();
    events.lock().unwrap().clear();
    let facade = Streams::new(&records);
    let one = fixture.sessions[0].id.clone();
    let two = fixture.sessions[1].id.clone();
    let (_, selected) = facade
        .select_open(
            "work",
            &SessionSelection::Only {
                value: NonEmpty::new(two.clone(), [one.clone(), done.id.clone()]),
            },
        )
        .unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|session| session.id.as_str())
            .collect::<Vec<_>>(),
        vec![two.as_str(), one.as_str(), done.id.as_str()]
    );
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .all(|event| !matches!(event, RecordIoEvent::List(_) | RecordIoEvent::Written(..)))
    );
    let (_, active) = facade
        .select_open("work", &SessionSelection::AllEligible)
        .unwrap();
    let mut expected = vec![one.clone(), two.clone()];
    expected.sort();
    assert_eq!(
        active
            .into_iter()
            .map(|session| session.id)
            .collect::<Vec<_>>(),
        expected
    );
    assert!(
        facade
            .select_open(
                "work",
                &SessionSelection::Only {
                    value: NonEmpty::new(one.clone(), [one])
                }
            )
            .is_err()
    );
    assert!(
        facade
            .select_open(
                "work",
                &SessionSelection::Only {
                    value: NonEmpty::new(fixture.sessions[2].id.clone(), [])
                }
            )
            .is_err()
    );
    assert!(
        facade
            .select_open(
                "work",
                &SessionSelection::Only {
                    value: NonEmpty::new("missing".into(), [])
                }
            )
            .is_err()
    );
    assert!(
        facade
            .select_open("missing", &SessionSelection::AllEligible)
            .is_err()
    );
}

#[test]
fn saved_batch_shares_host_decodes_and_producer_lists_with_standalone_artifact_parity() {
    use std::{
        fs,
        sync::{Arc, Mutex},
    };
    use streams::{
        application::{self, integration::Streams},
        yaml_store::{encode_component, records::RecordIoEvent},
    };
    let fixture = SavedFixture::new();
    let host = fixture.records.host().as_str();
    let partition = fixture
        .records
        .root()
        .join("hosts")
        .join(encode_component(host));
    let producer = |id: &str| {
        partition
            .join("streams/work/artifacts")
            .join(encode_component(id))
    };
    for session in [&fixture.sessions[0], &fixture.sessions[2]] {
        fs::create_dir_all(producer(&session.id)).unwrap();
    }
    fs::create_dir_all(producer("leftover-without-record")).unwrap();
    let foreign = fixture.records.root().join("hosts/other/conversations");
    fs::create_dir_all(&foreign).unwrap();
    fs::write(foreign.join("broken.yaml"), "not a record").unwrap();
    fs::write(partition.join("streams/unrequested.yaml"), "not a record").unwrap();
    let events = Arc::new(Mutex::new(vec![]));
    let observed = events.clone();
    let records = application::open_records_observed(&fixture.config, move |event| {
        observed.lock().unwrap().push(event)
    })
    .unwrap();
    events.lock().unwrap().clear();
    let facade = Streams::new(&records);
    let native = saved_presentation(facade.scope());
    let replies = facade
        .read_views(
            &[
                ViewSelection::SavedStream {
                    stream: "work".into(),
                },
                ViewSelection::SavedStream {
                    stream: "next".into(),
                },
                ViewSelection::SavedStream {
                    stream: "work".into(),
                },
                ViewSelection::SavedSession {
                    session: fixture.sessions[2].id.clone(),
                },
                ViewSelection::Sidebar {
                    space: NativeRef {
                        adapter: streams::frontend::FrontendId::hc(),
                        endpoint: streams::frontend::EndpointId("core".into()),
                        key: "target".into(),
                    },
                    viewing: ViewingChoice::Pinned {
                        session: fixture.sessions[0].id.clone(),
                    },
                },
            ],
            Access::Available { value: &native },
            Access::Unsupported,
        )
        .unwrap();
    let WireResult::Ok(ModReadValue::Stream { value: work }) = &replies[0] else {
        panic!("{replies:?}");
    };
    assert_eq!(work.title, "work");
    assert_eq!(work.goal.as_deref(), Some("Saved full goal"));
    assert_eq!(work.goal_short.as_deref(), Some("Saved short goal"));
    assert_eq!(
        work.documents
            .iter()
            .map(|document| document.key.path.as_str())
            .collect::<Vec<_>>(),
        ["z.md", "a.md"]
    );
    assert_eq!(work.documents[0].description, "Saved z.md");
    assert_eq!(
        work.links
            .iter()
            .map(|link| link.key.kind)
            .collect::<Vec<_>>(),
        [LinkKind::Issue, LinkKind::Pr]
    );
    assert_eq!(work.links[0].description, "Saved issue");
    assert_eq!(work.links[1].description, "Saved PR");
    let mut expected_members = vec![
        fixture.sessions[0].id.clone(),
        fixture.sessions[1].id.clone(),
    ];
    expected_members.sort();
    assert_eq!(
        work.sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>(),
        expected_members
    );
    let flat: Vec<_> = work
        .artifacts
        .iter()
        .flat_map(|group| group.entries.iter().cloned())
        .collect();
    assert_eq!(
        flat,
        application::artifacts::list_stream_artifacts(fixture.records.artifacts(), host, "work")
            .unwrap()
    );
    assert_eq!(
        flat.len(),
        2,
        "missing directories and unattached leftovers contribute nothing"
    );
    assert!(
        flat.iter()
            .any(|entry| entry.key.session == fixture.sessions[2].id && entry.key.stream == "work")
    );
    assert!(
        matches!(&replies[3],WireResult::Ok(ModReadValue::Session {value}) if value.stream=="next")
    );
    assert!(
        matches!(&replies[4],WireResult::Ok(ModReadValue::Sidebar{value:SidebarContext::Associated{viewing_session:ViewingSession::Selected{session},..}}) if session==&fixture.sessions[0].id)
    );
    let events = events.lock().unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, RecordIoEvent::List(_)))
            .count(),
        3,
        "one host partition and two producer directories"
    );
    assert_eq!(events.iter().filter(|event|matches!(event,RecordIoEvent::Read(path) if path.parent()==Some(partition.join("conversations").as_path()))).count(),3,"each Session decoded once, including the addressed Session");
    assert!(!events.iter().any(|event|matches!(event,RecordIoEvent::Read(path) if path.components().any(|part|part.as_os_str()=="artifacts"))),"no artifact content reads");
}

#[test]
fn saved_batch_distinguishes_empty_unavailable_and_a_broken_host_partition() {
    use streams::application::integration::Streams;
    let fixture = SavedFixture::new();
    let facade = Streams::new(&fixture.records);
    let empty = facade.read_stream("empty").unwrap();
    assert!(empty.view.sessions.is_empty());
    assert!(empty.view.artifacts.is_empty());
    assert!(facade.read_stream("missing").is_err());
    let partition = fixture
        .records
        .root()
        .join("hosts")
        .join(streams::yaml_store::encode_component(
            fixture.records.host().as_str(),
        ))
        .join("conversations");
    std::fs::write(partition.join("broken.yaml"), "not a record").unwrap();
    let values = facade
        .read_saved_views(&[
            ViewSelection::SavedStream {
                stream: "work".into(),
            },
            ViewSelection::SavedSession {
                session: fixture.sessions[0].id.clone(),
            },
            ViewSelection::HostStreams,
        ])
        .unwrap();
    let WireResult::Ok(ModReadValue::HostStreams { value: listing }) = &values[2] else {
        panic!(
            "the host listing survives an unreadable Session: {:?}",
            values[2]
        );
    };
    assert_eq!(listing.streams.len(), 3, "every readable Stream is listed");
    assert_eq!(listing.unreadable.len(), 1);
    assert_eq!(
        listing.unreadable[0].path,
        partition
            .join("broken.yaml")
            .strip_prefix(fixture.records.root())
            .unwrap()
    );
    let WireResult::Ok(ModReadValue::Stream { value: view }) = &values[0] else {
        panic!(
            "a Stream view survives an unreadable Session: {:?}",
            values[0]
        );
    };
    assert_eq!(view.sessions.len(), 2, "its readable members are shown");
    assert_eq!(
        view.unreadable, listing.unreadable,
        "and the Sessions it cannot show are named as in the listing"
    );
    assert!(
        empty.view.unreadable.is_empty(),
        "a clean partition names nothing"
    );
    assert!(
        matches!(&values[1], WireResult::Ok(ModReadValue::Session { .. })),
        "an addressed read remains independent from unrelated corrupt Sessions"
    );
}

#[test]
fn saved_cli_owns_stdout_and_needs_no_native_endpoint_with_or_without_global_json() {
    let fixture = SavedFixture::new();
    let request=fixture.envelope(json!({"kind":"read","selections":[
        {"kind":"saved_stream","stream":"work"},{"kind":"saved_session","session":fixture.sessions[0].id},
        {"kind":"saved_stream","stream":"empty"},{"kind":"saved_session","session":"missing"}]}));
    for global in [false, true] {
        let output = fixture.invoke(&request.to_string(), global);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(
            text.lines().count(),
            1,
            "one JSON reply, not a global --json wrapper"
        );
        let reply = ModReply::decode(&text).unwrap();
        assert_eq!(reply.scope.catalog, fixture.records.source_id());
        assert_eq!(reply.endpoint.0, "unconnected-core");
        assert!(reply.verified_endpoint.is_none());
        assert!(reply.viewer_projection.is_empty());
        let WireResult::Ok(ModResult::Read {
            values,
            notification_key,
            herdr_config,
        }) = reply.result
        else {
            panic!("read reply");
        };
        assert!(herdr_config.tab_naming);
        assert_eq!(
            herdr_config.viewer_mode,
            streams::config::ViewerMode::Editor
        );
        assert_eq!(
            herdr_config.viewer_placement,
            streams::config::ViewerPlacement::PaneRight
        );
        assert_eq!(notification_key, reply.scope.notification_key());
        assert!(
            matches!(&values[0],WireResult::Ok(ModReadValue::Stream {value}) if value.sessions.len()==2)
        );
        assert!(
            matches!(&values[1],WireResult::Ok(ModReadValue::Session {value}) if value.recap==fixture.sessions[0].recap)
        );
        assert!(
            matches!(&values[2],WireResult::Ok(ModReadValue::Stream {value}) if value.sessions.is_empty())
        );
        assert!(matches!(&values[3],WireResult::Error(WireError {kind,..}) if kind=="unavailable"));
    }
}

#[test]
fn saved_cli_refuses_wrong_scope_and_disabled_native_requests_without_effects() {
    let fixture = SavedFixture::new();
    let body = json!({"kind":"read","selections":[{"kind":"saved_stream","stream":"work"}]});
    for field in ["catalog", "host"] {
        let mut request = fixture.envelope(body.clone());
        request["scope"][field] = json!("wrong");
        let output = fixture.invoke(&request.to_string(), true);
        assert!(output.status.success());
        let reply = ModReply::decode(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
        assert!(
            matches!(reply.result,WireResult::Error(WireError {kind,..}) if kind=="unavailable")
        );
        assert!(reply.verified_endpoint.is_none());
    }
    for body in [
        fixtures()["requests"][1]["body"].clone(),
        fixtures()["requests"][3]["body"].clone(),
        fixtures()["requests"][5]["body"].clone(),
    ] {
        let output = fixture.invoke(&fixture.envelope(body).to_string(), false);
        assert!(output.status.success());
        let reply = ModReply::decode(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
        match reply.result {
            WireResult::Error(error) => assert_eq!(error.kind, "unavailable"),
            WireResult::Ok(ModResult::Read { values, .. }) => assert!(
                values
                    .iter()
                    .all(|value| matches!(value, WireResult::Error(_)))
            ),
            WireResult::Ok(ModResult::ReferenceOpened { value }) => assert!(matches!(
                value.result,
                ReferenceOpenResult::NotApplied { .. }
            )),
            _ => panic!("no native action succeeded without an endpoint"),
        }
        assert!(reply.verified_endpoint.is_none());
        assert!(reply.viewer_projection.is_empty());
    }
}

#[test]
fn describe_verifies_the_configured_core_without_caller_context_or_writes() {
    use std::{
        io::{BufRead, BufReader, Read, Write},
        os::unix::net::UnixListener,
        thread,
    };
    let fixture = SavedFixture::new();
    for global in [false, true] {
        let directory = tempfile::Builder::new()
            .prefix("hc-describe-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(request["method"], "hello");
            assert_eq!(request["params"], json!({"generation":1,"framed":false}));
            writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":request["id"],"result":{"generation":1,"framed":false,"core":{"id":"permanent-core","name":"Not authority"},"methods":["hello"]}})).unwrap();
            let mut extra = Vec::new();
            reader.read_to_end(&mut extra).unwrap();
            assert!(extra.is_empty(), "describe performs no native writes");
        });
        let mut command = fixture.command();
        if global {
            command.current_dir(directory.path()).env("HC_SOCKET", "s");
        } else {
            command.env("HC_SOCKET", &socket);
        }
        if global {
            command.arg("--json");
        }
        let output = command
            .args(["integration", "describe", "--frontend", "hc"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let text = std::str::from_utf8(&output.stdout).unwrap();
        assert_eq!(text.lines().count(), 1);
        let scope = streams::application::integration::Streams::new(&fixture.records)
            .scope()
            .clone();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            json!({"version":1,"scope":scope,"endpoint":"permanent-core","notification_key":scope.notification_key()})
        );
        server.join().unwrap();
    }
    for frontend in ["auto", "herdr", "cmux"] {
        let output = fixture
            .command()
            .args(["--json", "integration", "describe", "--frontend", frontend])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[test]
fn hc_resume_checks_registered_presence_before_replacing_the_process() {
    use std::{
        fs,
        io::{BufRead, BufReader, Read, Write},
        os::unix::{fs::PermissionsExt, net::UnixListener},
        thread,
    };
    use streams::{
        frontend::hc::processes::OwnerProcesses,
        integration::ConversationId,
        runtime::{
            CheckoutRegistry, DirectoryBindings,
            hc::{PiSourceClaim, labels::source_key},
            state::RuntimeState,
        },
    };
    let fixture = SavedFixture::new();
    let session = &fixture.sessions[0];
    let root = fixture._directory.path().canonicalize().unwrap();
    let transcript = root.join("exact.jsonl");
    fs::write(&transcript, "{\"type\":\"session\",\"id\":\"one\"}\n").unwrap();
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let program = bin.join("pi");
    fs::write(&program, "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$1\" \"$2\" \"$STREAMS_STREAM_ID\" \"$HC_TERMINAL_ID\"\n").unwrap();
    fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
    let host = fixture.records.host().as_str();
    let registry = CheckoutRegistry::new(fixture.config.checkout_state_dir(), host).unwrap();
    let state = RuntimeState::new(
        fixture
            .config
            .runtime_state_dir_for(fixture.records.root())
            .unwrap(),
        host,
    )
    .unwrap();
    let locations = DirectoryBindings::new(state.root(), registry);
    locations
        .remember("work", &root, Some(&session.id), false, None, None)
        .unwrap();
    locations
        .remember_transcript("work", &session.id, &transcript)
        .unwrap();
    let observer = OwnerProcesses::new().unwrap();
    let claim = PiSourceClaim {
        version: PiSourceClaim::VERSION,
        conversation: ConversationId::from(session),
        helper: observer.observe(std::process::id()).unwrap(),
    };
    let label = source_key(&claim.helper);
    let other_claim = PiSourceClaim {
        conversation: ConversationId {
            harness: "pi".into(),
            native_id: "other".into(),
        },
        ..claim.clone()
    };
    for (labels, frontend, expected) in [
        (json!({}), "hc", "launched"),
        (
            json!({label.clone():serde_json::to_string(&other_claim).unwrap()}),
            "auto",
            "launched",
        ),
        (
            json!({label.clone():serde_json::to_string(&claim).unwrap()}),
            "hc",
            "already observed running",
        ),
        (
            json!({"streams.pi.invalid":"broken"}),
            "hc",
            "hc Pi presence is unavailable",
        ),
    ] {
        let directory = tempfile::Builder::new()
            .prefix("hc-resume-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            use std::os::fd::AsRawFd;
            let mut fd = libc::pollfd {
                fd: listener.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            assert_eq!(
                unsafe { libc::poll(&mut fd, 1, 5000) },
                1,
                "resume did not connect"
            );
            let mut reader = BufReader::new(listener.accept().unwrap().0);
            reader
                .get_ref()
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            for (method, reply) in [
                (
                    "hello",
                    json!({"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","terminal.list"]}),
                ),
                (
                    "terminal.list",
                    json!({"terminals":[{"address":"t1","state":"live","pid":10,"labels":labels}]}),
                ),
            ] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["method"], method);
                writeln!(
                    reader.get_mut(),
                    "{}",
                    json!({"jsonrpc":"2.0","id":request["id"],"result":reply})
                )
                .unwrap();
            }
            let mut extra = Vec::new();
            reader.read_to_end(&mut extra).unwrap();
            assert!(
                extra.is_empty(),
                "resume presence inspection makes no native mutations"
            );
        });
        let output = fixture
            .command()
            .env("HC_SOCKET", &socket)
            .env("HC_TERMINAL_ID", "caller")
            .env("STREAMS_STREAM_ID", "work")
            .env("PATH", &bin)
            .args([
                "resume",
                "--current",
                "--session-key",
                &format!("{host}/{}", session.id),
                "--frontend",
                frontend,
            ])
            .output()
            .unwrap();
        assert!(
            server.join().is_ok(),
            "resume failed before completing hc inspection: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        if expected == "launched" {
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                String::from_utf8_lossy(&output.stdout),
                format!("--session|{}|work|caller", transcript.display())
            );
        } else {
            assert!(!output.status.success());
            assert!(
                String::from_utf8_lossy(&output.stderr).contains(expected),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stdout.is_empty());
        }
    }
    let unavailable = fixture
        .command()
        .env("HC_SOCKET", root.join("missing.sock"))
        .env("HC_TERMINAL_ID", "caller")
        .env("STREAMS_STREAM_ID", "work")
        .env("PATH", &bin)
        .args([
            "resume",
            "--current",
            "--session-key",
            &format!("{host}/{}", session.id),
            "--frontend",
            "hc",
        ])
        .output()
        .unwrap();
    assert!(!unavailable.status.success());
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("hc Pi presence is unavailable"));
    assert!(unavailable.stdout.is_empty());

    for (harness, native, header, argument) in [
        (
            "codex",
            "codex-one",
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-one\"}}\n",
            "resume",
        ),
        (
            "claude",
            "claude-one",
            "{\"sessionId\":\"claude-one\"}\n",
            "--resume",
        ),
    ] {
        let other = streams::model::Session::new(
            streams::model::Session::id_for(host, harness, native),
            host,
            "work",
            harness,
            native,
            native,
        )
        .unwrap();
        streams::application::create_record(
            fixture.records.records(),
            native,
            &streams::model::Record::Session(other.clone()),
        )
        .unwrap();
        let path = root.join(format!("{native}.jsonl"));
        fs::write(&path, header).unwrap();
        locations
            .remember("work", &root, Some(&other.id), false, None, None)
            .unwrap();
        locations
            .remember_transcript("work", &other.id, &path)
            .unwrap();
        let executable = bin.join(harness);
        fs::write(&executable, "#!/bin/sh\nprintf '%s|%s|%s|%s' \"$1\" \"$2\" \"$STREAMS_STREAM_ID\" \"$HC_TERMINAL_ID\"\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        let output = fixture
            .command()
            .env("HC_SOCKET", root.join("missing.sock"))
            .env("HC_TERMINAL_ID", "caller")
            .env("STREAMS_STREAM_ID", "work")
            .env("PATH", &bin)
            .args([
                "resume",
                "--current",
                "--session-key",
                &format!("{host}/{}", other.id),
                "--frontend",
                "hc",
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("{argument}|{native}|work|caller")
        );
    }
}

#[test]
fn document_cli_resolves_saved_keys_and_pipes_only_owner_bytes_to_the_secure_pager() {
    use std::{fs, os::unix::fs::PermissionsExt, process::Command};
    use streams::runtime::{CheckoutRegistry, DirectoryBindings};
    let fixture = SavedFixture::new();
    let root = fixture._directory.path().canonicalize().unwrap();
    let repository = root.join("repo");
    fs::create_dir(&repository).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repository)
            .status()
            .unwrap()
            .success()
    );
    fs::write(repository.join("z.md"), "Owner working document\n").unwrap();
    let host = fixture.records.host().as_str();
    let url = "https://example.test/source.git";
    let registry = CheckoutRegistry::new(fixture.config.checkout_state_dir(), host).unwrap();
    let checkout = registry.register(&repository, Some(url)).unwrap();
    let state = streams::runtime::state::RuntimeState::new(
        fixture
            .config
            .runtime_state_dir_for(fixture.records.root())
            .unwrap(),
        host,
    )
    .unwrap();
    let locations = DirectoryBindings::new(state.root(), registry);
    locations
        .remember(
            "work",
            &repository,
            Some(&fixture.sessions[0].id),
            false,
            Some(url),
            Some(&checkout.id),
        )
        .unwrap();
    let scope = streams::application::integration::Streams::new(&fixture.records)
        .scope()
        .clone();
    let reference = Reference::Document {
        stream: "work".into(),
        document: DocumentKey {
            repository: "source".into(),
            path: "z.md".into(),
        },
        viewing_session: fixture.sessions[0].id.clone(),
    };
    let bin = root.join("bin");
    fs::create_dir(&bin).unwrap();
    let pager = bin.join("less");
    let marker = root.join("pager-invocations");
    fs::write(&pager,"#!/bin/sh\nset -eu\n[ \"$LESSSECURE\" = 1 ]\n[ -z \"$LESS\" ]\n[ -z \"${LESSOPEN+x}\" ]\n[ -z \"${LESSCLOSE+x}\" ]\n[ \"$#\" = 1 ]\n[ \"$1\" = -- ]\nprintf 'called\\n' >> \"$FAKE_LESS_MARKER\"\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done\nprintf 'document viewer done\\n'\n").unwrap();
    fs::set_permissions(&pager, fs::Permissions::from_mode(0o700)).unwrap();
    let command = |scope: &WorkScope| {
        let mut command = fixture.command();
        command
            .args([
                "integration",
                "view-document",
                "--scope-json",
                &serde_json::to_string(scope).unwrap(),
                "--reference-json",
                &serde_json::to_string(&reference).unwrap(),
            ])
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    bin.display(),
                    std::env::var("PATH").unwrap_or_default()
                ),
            )
            .env("FAKE_LESS_MARKER", &marker)
            .env("LESS", "unsafe")
            .env("LESSOPEN", "unsafe")
            .env("LESSCLOSE", "unsafe");
        command
    };
    let mut terminal = crate::support::pty::Pty::spawn(command(&scope), false);
    terminal.until(|terminal| terminal.screen().contains("document viewer done"));
    assert!(terminal.screen().contains("Owner working document"));
    assert!(terminal.finish().success());
    assert_eq!(fs::read_to_string(&marker).unwrap(), "called\n");
    let mut wrong = scope.clone();
    wrong.host = "wrong-host".into();
    let mut terminal = crate::support::pty::Pty::spawn(command(&wrong), false);
    terminal.until(|terminal| terminal.screen().contains("does not match"));
    drop(terminal);
    streams::application::move_session_observed(
        fixture.records.records(),
        "move-viewer",
        &streams::operations::membership::MoveIntent::new(host, "pi", "one", "next").unwrap(),
    )
    .unwrap();
    let mut terminal = crate::support::pty::Pty::spawn(command(&scope), false);
    terminal.until(|terminal| terminal.screen().contains("not a current member"));
    drop(terminal);
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        "called\n",
        "invalid scope/context never starts the pager"
    );
}

#[test]
fn document_cli_rejects_json_mode_non_document_references_and_client_paths() {
    let fixture = SavedFixture::new();
    let scope = fixture.envelope(json!({}))["scope"].to_string();
    for (global, reference) in [
        (
            true,
            json!({"kind":"document","stream":"work","document":{"repository":"source","path":"z.md"},"viewing_session":fixture.sessions[0].id}),
        ),
        (
            false,
            json!({"kind":"document","path":"/unchecked/client/path"}),
        ),
        (
            false,
            json!({"kind":"link","stream":"work","link":{"kind":"issue","url":"https://example.test/shared"}}),
        ),
    ] {
        let mut command = fixture.command();
        if global {
            command.arg("--json");
        }
        let output = command
            .args([
                "integration",
                "view-document",
                "--scope-json",
                &scope,
                "--reference-json",
                &reference.to_string(),
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        if global {
            assert!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .contains("does not support --json")
            );
        } else {
            assert!(output.stdout.is_empty());
            assert!(!output.stderr.is_empty());
        }
    }
}

#[test]
fn describe_and_request_do_not_expose_private_configuration_failures() {
    let fixture = SavedFixture::new();
    let marker = "private-owner-configuration-content";
    std::fs::write(
        fixture.config.config_path(),
        json!({(marker):{}}).to_string(),
    )
    .unwrap();
    for global in [false, true] {
        let mut command = fixture.command();
        if global {
            command.arg("--json");
        }
        let output = command
            .args(["integration", "describe", "--frontend", "hc"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(marker));
        let output = fixture.invoke(
            &fixture
                .envelope(
                    json!({"kind":"read","selections":[{"kind":"saved_stream","stream":"work"}]}),
                )
                .to_string(),
            global,
        );
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        assert!(!String::from_utf8_lossy(&output.stdout).contains(marker));
        let reply = ModReply::decode(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
        assert!(matches!(reply.result, WireResult::Error(_)));
    }
}

#[test]
fn link_actions_revalidate_saved_kind_and_url_without_native_or_owner_path_disclosure() {
    use streams::{
        application,
        model::{
            Record, Reference, ReferenceRole, ReferenceState, ReferenceTarget, ReferenceValue,
            Stream, TitleDefaults,
        },
        operations::references::ReferenceChange,
    };
    let fixture = SavedFixture::new();
    let mut stream =
        Stream::new("link-only", "Links", None, fixture.records.host().as_str()).unwrap();
    let urls = [
        "https://example.test/issue-only",
        "https://owner-secret:secret@example.test/",
        "javascript:alert(1)",
        "file:///owner-private/file",
    ];
    stream.references.push(
        Reference::new(
            ReferenceValue {
                name: "Issue".into(),
                purpose: "Issue".into(),
                role: ReferenceRole::Dependency,
                state: ReferenceState::Active,
                target: ReferenceTarget::Issue {
                    url: urls[0].into(),
                },
            },
            "rev-1",
        )
        .unwrap(),
    );
    application::create_record(
        fixture.records.records(),
        "create-links",
        &Record::Stream(stream.clone()),
    )
    .unwrap();
    let socket = fixture._directory.path().join("no-native.socket");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let invoke = |stream: &str, kind: &str, url: &str, context: Value, global: bool| {
        let request=fixture.envelope(json!({"kind":"act","action":{"kind":"open_reference","reference":{"kind":"link","stream":stream,"link":{"kind":kind,"url":url}},"context":context,"at":null}}));
        let mut command = fixture.command();
        command.env("HC_SOCKET", &socket);
        if global {
            command.arg("--json");
        }
        let output = command
            .args([
                "integration",
                "request",
                "--request-json",
                &request.to_string(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["verified_endpoint"], Value::Null);
        assert_eq!(value["endpoint"], "unconnected-core");
        assert_eq!(value["viewer_projection"], json!([]));
        assert_eq!(value["result"]["ok"]["kind"], "reference_opened");
        assert_eq!(value["result"]["ok"]["value"]["known"], json!([]));
        assert!(
            matches!(listener.accept(),Err(error) if error.kind()==std::io::ErrorKind::WouldBlock),
            "links require no native connection"
        );
        value["result"]["ok"]["value"]["result"].clone()
    };
    for global in [false, true] {
        for (stream, kind, url, allowed) in [
            ("work", "issue", "https://example.test/shared-issue", true),
            ("work", "pr", "https://example.test/shared-pr", true),
            ("link-only", "issue", urls[0], true),
            ("link-only", "pr", urls[0], false),
            ("missing", "issue", urls[0], false),
            ("work", "issue", "https://example.test/removed", false),
            ("link-only", "issue", urls[1], false),
            ("link-only", "issue", urls[2], false),
            ("link-only", "issue", urls[3], false),
        ] {
            let result = invoke(stream, kind, url, json!({"kind":"explicit"}), global);
            if allowed {
                assert_eq!(
                    result,
                    json!({"kind":"needs_viewer","value":[{"kind":"open_url","url":url}]})
                );
            } else {
                assert_eq!(result["kind"], "not_applied");
                assert!(!result.to_string().contains(url));
            }
        }
        let result = invoke(
            "work",
            "issue",
            "https://example.test/shared-issue",
            json!({"kind":"follow_frame","terminal":{"adapter":"hc","endpoint":"core","key":"t1"},"expected_session":fixture.sessions[0].id}),
            global,
        );
        assert_eq!(
            result,
            json!({"kind":"needs_viewer","value":[{"kind":"open_url","url":"https://example.test/shared-issue"}]}),
            "saved links open directly without foreground process-group re-verification"
        );
    }
    application::save_user_references(
        fixture.records.records(),
        "remove-painted-link",
        &stream,
        vec![ReferenceChange::UserDelete {
            key: streams::model::ResourceKey::Url {
                url: urls[0].into(),
            },
        }],
        TitleDefaults::default(),
    )
    .unwrap();
    assert_eq!(
        invoke(
            "link-only",
            "issue",
            urls[0],
            json!({"kind":"explicit"}),
            false
        )["kind"],
        "not_applied",
        "activation rereads after a painted reference was removed"
    );
}

#[test]
fn shared_sidebar_policy_uses_only_point_foreground_and_keeps_inactive_pinned_members() {
    use streams::{
        application,
        model::{Record, Session, Status},
    };
    let fixture = SavedFixture::new();
    let facade = application::integration::Streams::new(&fixture.records);
    let native = saved_presentation(facade.scope());
    let mut inactive = Session::new(
        Session::id_for(&facade.scope().host, "pi", "inactive"),
        &facade.scope().host,
        "work",
        "pi",
        "inactive",
        "Inactive",
    )
    .unwrap();
    inactive.status = Status::Done;
    application::create_record(
        fixture.records.records(),
        "inactive-member",
        &Record::Session(inactive.clone()),
    )
    .unwrap();
    let resource = |key: &str| NativeRef {
        adapter: streams::frontend::FrontendId::hc(),
        endpoint: streams::frontend::EndpointId("core".into()),
        key: key.into(),
    };
    for (execution, expected) in [
        (PointOnlyExecution::Current("one"), "selected"),
        (PointOnlyExecution::Current("moved"), "none"),
        (PointOnlyExecution::Current("unenrolled"), "none"),
        (PointOnlyExecution::NotApplicable, "none"),
        (PointOnlyExecution::Unknown, "unavailable"),
    ] {
        let selections = [
            ViewingChoice::Follow {
                last_agent: Some(resource("t1")),
            },
            ViewingChoice::Follow { last_agent: None },
            ViewingChoice::Follow {
                last_agent: Some(resource("t2")),
            },
            ViewingChoice::Pinned {
                session: inactive.id.clone(),
            },
            ViewingChoice::Pinned {
                session: fixture.sessions[2].id.clone(),
            },
        ]
        .into_iter()
        .map(|viewing| ViewSelection::Sidebar {
            space: resource("target"),
            viewing,
        })
        .collect::<Vec<_>>();
        let replies = facade
            .read_views(
                &selections,
                Access::Available { value: &native },
                Access::Available { value: &execution },
            )
            .unwrap();
        for (index, expected) in [expected, "none", "none", "selected", "unavailable"]
            .iter()
            .enumerate()
        {
            let WireResult::Ok(ModReadValue::Sidebar {
                value:
                    SidebarContext::Associated {
                        stream,
                        viewing_session,
                    },
            }) = &replies[index]
            else {
                panic!("{:?}", replies[index])
            };
            assert_eq!(stream.id, "work");
            assert_eq!(
                serde_json::to_value(viewing_session).unwrap()["kind"],
                *expected
            );
        }
        assert!(
            matches!(&replies[3],WireResult::Ok(ModReadValue::Sidebar{value:SidebarContext::Associated{viewing_session:ViewingSession::Selected{session},..}}) if session==&inactive.id)
        );
    }
}

#[test]
fn native_sidebar_reads_use_one_context_snapshot_and_keep_saved_data_when_follow_is_unavailable() {
    use crate::integration_hc::{FakeCore, Reply, Step};
    let fixture = SavedFixture::new();
    let scope = streams::application::integration::Streams::new(&fixture.records)
        .scope()
        .clone();
    let resource = |key: &str| NativeRef {
        adapter: streams::frontend::FrontendId::hc(),
        endpoint: streams::frontend::EndpointId("core".into()),
        key: key.into(),
    };
    let row = |key: &str| streams::runtime::hc::SpaceAssociation {
        scope: scope.clone(),
        stream: "work".into(),
        space: resource(key),
    };
    let mut foreign = row("foreign");
    foreign.scope.host = "another-host".into();
    let associations = json!({"version":1,"spaces":{"target":row("target"),"stale":row("stale"),"foreign":foreign}});
    let layout = json!({"spaces":[{"id":"target","tabs":[{"id":"tab","root":{"type":"block","content":{"terminal":"t1"}}}]},{"id":"outside","tabs":[{"id":"other-tab","root":{"type":"block","content":{"terminal":"t2"}}}]},{"id":"empty"},{"id":"foreign"}]});
    let sidebar = |space: NativeRef, viewing: Value| json!({"kind":"sidebar","space":space,"viewing":viewing});
    let follow = |candidate: Option<NativeRef>| json!({"kind":"follow","last_agent":candidate});
    let pinned = |id: &str| json!({"kind":"pinned","session":id});
    let mut wrong = resource("target");
    wrong.endpoint.0 = "wrong-core".into();
    let mut request=fixture.envelope(json!({"kind":"read","selections":[{"kind":"saved_stream","stream":"work"},sidebar(resource("target"),follow(None)),sidebar(resource("target"),pinned(&fixture.sessions[0].id)),sidebar(resource("target"),pinned(&fixture.sessions[2].id)),sidebar(resource("target"),pinned("missing")),sidebar(resource("target"),follow(Some(resource("t1")))),sidebar(resource("target"),follow(Some(resource("t2")))),sidebar(resource("empty"),follow(None)),sidebar(resource("stale"),follow(None)),sidebar(resource("foreign"),follow(None)),sidebar(wrong,follow(None)),{"kind":"frame","terminal":resource("t1")}] }));
    request["endpoint"] = json!("core");
    for global_json in [false, true] {
        let core = FakeCore::new(vec![
            Step(
                "doc.get",
                json!({"name":"streams"}),
                Reply::Value(json!({"version":7,"value":associations})),
            ),
            Step(
                "doc.get",
                json!({"name":"herdr-app"}),
                Reply::Value(json!({"version":90,"value":layout})),
            ),
            Step(
                "terminal.get",
                json!({"terminal":"t1"}),
                Reply::Reject("unavailable"),
            ),
        ]);
        let mut command = fixture.command();
        command.env("HC_SOCKET", &core.socket);
        if global_json {
            command.arg("--json");
        }
        let output = command
            .args([
                "integration",
                "request",
                "--request-json",
                &request.to_string(),
            ])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let reply = ModReply::decode(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
        assert_eq!(
            reply.verified_endpoint,
            Some(streams::frontend::EndpointId("core".into()))
        );
        assert!(reply.viewer_projection.is_empty());
        assert!(reply.warnings.is_empty());
        let WireResult::Ok(ModResult::Read { values, .. }) = reply.result else {
            panic!("read reply")
        };
        let values = serde_json::to_value(values).unwrap();
        assert_eq!(values[0]["ok"]["kind"], "stream");
        let associated = &values[1]["ok"]["value"];
        assert_eq!(associated["kind"], "associated");
        assert_eq!(associated["stream"]["goal"], "Saved full goal");
        assert_eq!(associated["viewing_session"]["kind"], "none");
        assert_eq!(
            values[2]["ok"]["value"]["viewing_session"],
            json!({"kind":"selected","session":fixture.sessions[0].id})
        );
        for index in [3, 4, 5] {
            assert_eq!(values[index]["ok"]["value"]["kind"], "associated");
            assert_eq!(
                values[index]["ok"]["value"]["viewing_session"]["kind"],
                "unavailable"
            );
            assert_eq!(values[index]["ok"]["value"]["stream"], associated["stream"]);
        }
        assert_eq!(values[6]["ok"]["value"]["viewing_session"]["kind"], "none");
        for index in [7, 9] {
            assert_eq!(values[index]["ok"]["value"]["kind"], "unassociated");
        }
        for index in [8, 10] {
            assert_eq!(values[index]["ok"]["value"]["kind"], "unavailable");
        }
        assert_eq!(
            values[11]["ok"]["context"]["kind"], "unavailable",
            "frame execution reports unavailable when terminal.get fails"
        );
        assert_eq!(
            core.finish().len(),
            4,
            "only the in-space Follow candidate is read"
        );
    }
}

#[test]
fn native_sidebar_follows_a_registered_foreground_conversation_without_a_process_census() {
    use crate::integration_hc::{FakeCore, Reply, Step};
    use streams::{
        frontend::{EndpointId, FrontendId, hc::processes::OwnerProcesses},
        runtime::hc::{PiSourceClaim, labels::source_key},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let fixture = SavedFixture::new();
    let child = Child(
        std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap(),
    );
    let observer = OwnerProcesses::new().unwrap();
    let helper = observer.observe(child.0.id()).unwrap();
    let endpoint = EndpointId("core".into());
    let scope = streams::application::integration::Streams::new(&fixture.records)
        .scope()
        .clone();
    let resource = |key: &str| NativeRef {
        adapter: FrontendId::hc(),
        endpoint: endpoint.clone(),
        key: key.into(),
    };
    let claim = PiSourceClaim {
        version: PiSourceClaim::VERSION,
        conversation: ConversationId {
            harness: "pi".into(),
            native_id: fixture.sessions[0].harness_session_id.clone(),
        },
        helper: helper.clone(),
    };
    let label = source_key(&claim.helper);
    let association = streams::runtime::hc::SpaceAssociation {
        scope: scope.clone(),
        stream: "work".into(),
        space: resource("target"),
    };
    let core = FakeCore::new(vec![
        Step(
            "doc.get",
            json!({"name":"streams"}),
            Reply::Value(
                json!({"version":1,"value":{"version":1,"spaces":{"target":association}}}),
            ),
        ),
        Step(
            "doc.get",
            json!({"name":"herdr-app"}),
            Reply::Value(
                json!({"version":2,"value":{"spaces":[{"id":"target","tabs":[{"id":"tab","root":{"type":"block","content":{"terminal":"t1"}}}]}]}}),
            ),
        ),
        Step(
            "terminal.get",
            json!({"terminal":"t1"}),
            Reply::Value(
                json!({"address":"t1","state":"live","pid":10,"labels":{label:serde_json::to_string(&claim).unwrap()}}),
            ),
        ),
    ]);
    let selection = json!({"kind":"sidebar","space":resource("target"),"viewing":{"kind":"follow","last_agent":resource("t1")}});
    let mut request =
        fixture.envelope(json!({"kind":"read","selections":[selection.clone(),selection]}));
    request["endpoint"] = json!("core");
    let output = fixture
        .command()
        .env("HC_SOCKET", &core.socket)
        .args([
            "integration",
            "request",
            "--request-json",
            &request.to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply = ModReply::decode(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::Read { values, .. }) = reply.result else {
        panic!("read reply")
    };
    for value in serde_json::to_value(values).unwrap().as_array().unwrap() {
        assert_eq!(
            value["ok"]["value"]["viewing_session"],
            json!({"kind":"selected","session":fixture.sessions[0].id})
        );
    }
    assert_eq!(
        core.finish().len(),
        4,
        "one point read for the repeated candidate"
    );
}

#[test]
fn unavailable_native_sidebar_context_does_not_break_independent_saved_reads_or_claim_empty_context()
 {
    use crate::integration_hc::{FakeCore, Reply, Step};
    let fixture = SavedFixture::new();
    for case in ["wrong-endpoint", "bad-associations", "bad-layout"] {
        let steps = match case {
            "wrong-endpoint" => vec![],
            "bad-associations" => vec![Step(
                "doc.get",
                json!({"name":"streams"}),
                Reply::Value(json!({"version":1,"value":{"version":2,"spaces":{}}})),
            )],
            _ => vec![
                Step(
                    "doc.get",
                    json!({"name":"streams"}),
                    Reply::Value(json!({"version":0,"value":null})),
                ),
                Step(
                    "doc.get",
                    json!({"name":"herdr-app"}),
                    Reply::Value(json!({"version":1,"value":{"spaces":null}})),
                ),
            ],
        };
        let core = FakeCore::new(steps);
        let mut request=fixture.envelope(json!({"kind":"read","selections":[{"kind":"saved_stream","stream":"work"},{"kind":"sidebar","space":{"adapter":"hc","endpoint":"core","key":"target"},"viewing":{"kind":"follow","last_agent":null}}]}));
        if case != "wrong-endpoint" {
            request["endpoint"] = json!("core");
        }
        let output = fixture
            .command()
            .env("HC_SOCKET", &core.socket)
            .args([
                "integration",
                "request",
                "--request-json",
                &request.to_string(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            reply["verified_endpoint"].is_null(),
            case == "wrong-endpoint"
        );
        assert_eq!(reply["result"]["ok"]["values"][0]["ok"]["kind"], "stream");
        assert_eq!(
            reply["result"]["ok"]["values"][1]["error"]["kind"],
            "unavailable"
        );
        assert_eq!(
            core.finish().len(),
            match case {
                "wrong-endpoint" => 1,
                "bad-associations" => 2,
                _ => 3,
            }
        );
    }
}

#[test]
fn bind_space_uses_only_the_saved_stream_and_one_verified_native_write() {
    use crate::integration_hc::{FakeCore, Reply, Step};
    let fixture = SavedFixture::new();
    let scope = streams::application::integration::Streams::new(&fixture.records)
        .scope()
        .clone();
    let space = NativeRef {
        adapter: streams::frontend::FrontendId::hc(),
        endpoint: streams::frontend::EndpointId("core".into()),
        key: "target".into(),
    };
    let wanted = streams::runtime::hc::SpaceAssociation {
        scope: scope.clone(),
        stream: "work".into(),
        space: space.clone(),
    };
    let partition = fixture
        .records
        .root()
        .join("hosts")
        .join(streams::yaml_store::encode_component(&scope.host))
        .join("conversations");
    std::fs::write(partition.join("unrelated-broken.yaml"), "not a record").unwrap();
    for case in ["accepted", "conflict", "lost", "unpublished"] {
        let layout = if case == "unpublished" {
            json!({})
        } else {
            json!({"spaces":[{"id":"target"}]})
        };
        let mut steps = vec![
            Step(
                "doc.get",
                json!({"name":"streams"}),
                Reply::Value(json!({"version":7,"value":null})),
            ),
            Step(
                "doc.get",
                json!({"name":"herdr-app"}),
                Reply::Value(json!({"version":90,"value":layout})),
            ),
        ];
        if case != "unpublished" {
            let reply = match case {
                "accepted" => Reply::Value(json!({"version":8})),
                "conflict" => Reply::Reject("conflict"),
                _ => Reply::Lost,
            };
            steps.push(Step("doc.set",json!({"name":"streams","version":7,"value":{"version":1,"spaces":{"target":wanted}}}),reply));
        }
        let core = FakeCore::new(steps);
        let output = fixture
            .command()
            .env("HC_SOCKET", &core.socket)
            .args([
                "--json",
                "integration",
                "bind-space",
                "--space-ref",
                &serde_json::to_string(&space).unwrap(),
                "--scope-json",
                &serde_json::to_string(&scope).unwrap(),
                "--stream",
                "work",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(if case == "accepted" { 0 } else { 2 }),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            result["requested_space"],
            serde_json::to_value(&space).unwrap()
        );
        assert_eq!(
            result["association"]["kind"],
            match case {
                "accepted" => "applied",
                "lost" => "uncertain",
                _ => "not_applied",
            }
        );
        assert_eq!(
            core.finish().len(),
            if case == "unpublished" { 3 } else { 4 }
        );
    }
    let core = FakeCore::new(vec![]);
    let mut wrong = space.clone();
    wrong.endpoint.0 = "wrong-core".into();
    let output = fixture
        .command()
        .env("HC_SOCKET", &core.socket)
        .args([
            "--json",
            "integration",
            "bind-space",
            "--space-ref",
            &serde_json::to_string(&wrong).unwrap(),
            "--scope-json",
            &serde_json::to_string(&scope).unwrap(),
            "--stream",
            "work",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(core.finish().len(), 1, "wrong endpoint stops at hello");
    for (stream, host) in [("missing", scope.host.as_str()), ("work", "wrong-host")] {
        let mut wrong = scope.clone();
        wrong.host = host.into();
        let output = fixture
            .command()
            .env("HC_SOCKET", "/missing/native/socket")
            .args([
                "--json",
                "integration",
                "bind-space",
                "--space-ref",
                &serde_json::to_string(&space).unwrap(),
                "--scope-json",
                &serde_json::to_string(&wrong).unwrap(),
                "--stream",
                stream,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("hc socket:"),
            "saved admission precedes native connection"
        );
    }
}

#[test]
fn invalid_cli_envelopes_have_stderr_and_no_fabricated_json_reply() {
    let fixture = SavedFixture::new();
    for text in [
        "not JSON".to_owned(),
        " ".repeat(MOD_REQUEST_BYTES + 1),
        "{}".into(),
    ] {
        for global in [false, true] {
            let output = fixture.invoke(&text, global);
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!output.stderr.is_empty());
        }
    }
}

#[test]
fn session_only_batch_has_no_directory_inventory_or_artifact_reads() {
    use std::sync::{Arc, Mutex};
    use streams::{
        application::{self, integration::Streams},
        yaml_store::records::RecordIoEvent,
    };
    let fixture = SavedFixture::new();
    let events = Arc::new(Mutex::new(vec![]));
    let observed = events.clone();
    let records = application::open_records_observed(&fixture.config, move |event| {
        observed.lock().unwrap().push(event)
    })
    .unwrap();
    events.lock().unwrap().clear();
    let selection = ViewSelection::SavedSession {
        session: fixture.sessions[0].id.clone(),
    };
    let values = Streams::new(&records)
        .read_saved_views(&[selection.clone(), selection])
        .unwrap();
    assert!(
        values
            .iter()
            .all(|value| matches!(value, WireResult::Ok(ModReadValue::Session { .. })))
    );
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
        "one format check and one addressed Session for both selections"
    );
}

#[test]
fn malformed_or_non_directory_producers_fail_only_the_affected_stream() {
    use streams::application::integration::Streams;
    for directory in [false, true] {
        let fixture = SavedFixture::new();
        let parent = fixture
            .records
            .root()
            .join("hosts")
            .join(streams::yaml_store::encode_component(
                fixture.records.host().as_str(),
            ))
            .join("streams/work/artifacts");
        std::fs::create_dir_all(&parent).unwrap();
        if directory {
            std::fs::create_dir(parent.join("invalid%")).unwrap();
        } else {
            std::fs::write(parent.join("not-a-directory"), "ignored content").unwrap();
        }
        let values = Streams::new(&fixture.records)
            .read_saved_views(&[
                ViewSelection::SavedStream {
                    stream: "work".into(),
                },
                ViewSelection::SavedStream {
                    stream: "empty".into(),
                },
            ])
            .unwrap();
        assert!(matches!(&values[0], WireResult::Error(_)));
        assert!(matches!(
            &values[1],
            WireResult::Ok(ModReadValue::Stream { .. })
        ));
    }
}

#[test]
fn an_unreadable_producer_directory_is_unavailable_not_empty() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let fixture = SavedFixture::new();
    let directory = fixture
        .records
        .root()
        .join("hosts")
        .join(streams::yaml_store::encode_component(
            fixture.records.host().as_str(),
        ))
        .join("streams/work/artifacts");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o0)).unwrap();
    let result =
        streams::application::integration::Streams::new(&fixture.records).read_stream("work");
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert!(result.is_err());
}

#[test]
fn oversized_cli_reply_fails_before_any_stdout_is_written() {
    use streams::{
        application,
        model::{Record, Session},
    };
    let fixture = SavedFixture::new();
    let host = fixture.records.host().as_str();
    let mut session = Session::new(
        Session::id_for(host, "pi", "large"),
        host,
        "work",
        "pi",
        "large",
        "Large saved record",
    )
    .unwrap();
    session.recap = "x".repeat(600_000);
    application::create_record(
        fixture.records.records(),
        "large",
        &Record::Session(session.clone()),
    )
    .unwrap();
    let selection = json!({"kind":"saved_session","session":session.id});
    let request = fixture.envelope(json!({"kind":"read","selections":vec![selection;9]}));
    let output = fixture.invoke(&request.to_string(), true);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("exceeds 4 MiB"));
}

#[test]
fn reply_limit_includes_the_cli_newline() {
    let mut reply = ModReply::decode(&fixtures()["replies"][0].to_string()).unwrap();
    let size = reply.encode().unwrap().len();
    reply
        .warnings
        .push("x".repeat(MOD_REPLY_BYTES - 1 - size - 2));
    let encoded = reply.encode().unwrap();
    assert_eq!(encoded.len() + 1, MOD_REPLY_BYTES);
    assert!(ModReply::decode(&(encoded + "\n")).is_ok());
    reply.warnings[0].push('x');
    assert!(reply.encode().is_err());
}

#[test]
fn version_one_fixtures_have_exact_request_and_reply_shapes() {
    let fixture = fixtures();
    for data in fixture["requests"].as_array().unwrap() {
        let decoded = ModRequest::decode(&data.to_string()).unwrap();
        assert_eq!(serde_json::to_value(decoded).unwrap(), *data);
    }
    for data in fixture["replies"].as_array().unwrap() {
        let decoded = ModReply::decode(&data.to_string()).unwrap();
        assert_eq!(serde_json::to_value(&decoded).unwrap(), *data);
        assert_eq!(
            serde_json::from_str::<Value>(&decoded.encode().unwrap()).unwrap(),
            *data
        );
    }
    assert_eq!(
        WorkScope {
            catalog: "catalog".into(),
            host: "host".into()
        }
        .notification_key(),
        "streams.saved.ac2a86c465bdcd578f259810f69d9a156bc4e03eb8d37cb0d4cbec57fb42a6b5"
    );
}

#[test]
fn wire_rejects_unknown_omitted_duplicate_positional_and_multiple_result_fields() {
    let fixture = fixtures();
    let request = &fixture["requests"][0];
    for broken in [
        {
            let mut data = request.clone();
            data["version"] = json!(2);
            data
        },
        {
            let mut data = request.clone();
            data["extra"] = json!(true);
            data
        },
        {
            let mut data = request.clone();
            data["scope"] = json!(["catalog", "host"]);
            data
        },
        {
            let mut data = request.clone();
            data["body"]["selections"] = json!([]);
            data
        },
        {
            let mut data = request.clone();
            data["body"]["selections"][0]["extra"] = json!(true);
            data
        },
    ] {
        assert!(ModRequest::decode(&broken.to_string()).is_err(), "{broken}");
    }
    let encoded = request.to_string();
    assert!(
        ModRequest::decode(&encoded.replacen("\"version\":1", "\"version\":1,\"version\":1", 1))
            .is_err()
    );
    assert!(ModRequest::decode(&(encoded + " {} ")).is_err());
    let mut action = fixture["requests"][2].clone();
    action["body"]["action"]
        .as_object_mut()
        .unwrap()
        .remove("at");
    assert!(ModRequest::decode(&action.to_string()).is_err());
    for field in ["verified_endpoint", "viewer_projection", "warnings"] {
        let mut data = fixture["replies"][0].clone();
        data.as_object_mut().unwrap().remove(field);
        assert!(ModReply::decode(&data.to_string()).is_err(), "{field}");
    }
    let mut data = fixture["replies"][0].clone();
    data["result"]["error"] = json!({"kind":"unavailable","message":"second branch"});
    assert!(ModReply::decode(&data.to_string()).is_err());
}

#[test]
fn request_and_reply_byte_limits_fail_without_partial_output() {
    assert!(ModRequest::decode(&" ".repeat(MOD_REQUEST_BYTES + 1)).is_err());
    let mut reply = ModReply::decode(&fixtures()["replies"][0].to_string()).unwrap();
    reply.warnings.push("x".repeat(MOD_REPLY_BYTES));
    assert!(reply.encode().is_err());
}

#[test]
fn frame_reads_and_reference_opening_resolve_enrolled_sessions_and_viewer_terminals() {
    use crate::integration_hc::{FakeCore, Reply, Step};
    use streams::{
        frontend::{EndpointId, FrontendId, hc::processes::OwnerProcesses},
        runtime::hc::{PiSourceClaim, labels::source_key},
    };
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let fixture = SavedFixture::new();
    let child = Child(
        std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap(),
    );
    let observer = OwnerProcesses::new().unwrap();
    let helper = observer.observe(child.0.id()).unwrap();
    let endpoint = EndpointId("core".into());
    let scope = streams::application::integration::Streams::new(&fixture.records)
        .scope()
        .clone();
    let resource = |key: &str| NativeRef {
        adapter: FrontendId::hc(),
        endpoint: endpoint.clone(),
        key: key.into(),
    };
    let enrolled_claim = PiSourceClaim {
        version: PiSourceClaim::VERSION,
        conversation: ConversationId {
            harness: "pi".into(),
            native_id: fixture.sessions[0].harness_session_id.clone(),
        },
        helper: helper.clone(),
    };
    let unenrolled_claim = PiSourceClaim {
        conversation: ConversationId {
            harness: "pi".into(),
            native_id: "unenrolled-native".into(),
        },
        ..enrolled_claim.clone()
    };
    let label = source_key(&enrolled_claim.helper);
    let core = FakeCore::new(vec![
        Step(
            "terminal.get",
            json!({"terminal":"t1"}),
            Reply::Value(json!({
                "address":"t1",
                "state":"live",
                "pid":10,
                "labels":{label.clone():serde_json::to_string(&enrolled_claim).unwrap()}
            })),
        ),
        Step(
            "terminal.get",
            json!({"terminal":"t2"}),
            Reply::Value(json!({
                "address":"t2",
                "state":"live",
                "pid":10,
                "labels":{label:serde_json::to_string(&unenrolled_claim).unwrap()}
            })),
        ),
        Step(
            "terminal.get",
            json!({"terminal":"t3"}),
            Reply::Value(json!({
                "address":"t3",
                "state":"live",
                "pid":10,
                "labels":{}
            })),
        ),
    ]);
    let mut request = fixture.envelope(json!({
        "kind":"read",
        "selections":[
            {"kind":"frame","terminal":resource("t1")},
            {"kind":"frame","terminal":resource("t2")},
            {"kind":"frame","terminal":resource("t3")}
        ]
    }));
    request["endpoint"] = json!("core");
    let output = fixture
        .command()
        .env("HC_SOCKET", &core.socket)
        .args([
            "integration",
            "request",
            "--request-json",
            &request.to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply = ModReply::decode(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::Read { values, .. }) = reply.result else {
        panic!("read reply")
    };
    assert!(matches!(
        &values[0],
        WireResult::Ok(ModReadValue::Frame {
            context: FrameContext::Enrolled {
                session,
                companion: CompanionVisibility::Shown,
            }
        }) if session.id == fixture.sessions[0].id
    ));
    assert_eq!(
        serde_json::to_value(&values[1]).unwrap(),
        json!({"ok":{"kind":"frame","context":{"kind":"unenrolled"}}})
    );
    assert_eq!(
        serde_json::to_value(&values[2]).unwrap(),
        json!({"ok":{"kind":"frame","context":{"kind":"not_applicable"}}})
    );
    assert_eq!(core.finish().len(), 4);

    // Corrupting the enrolled Session YAML yields FrameContext::Unavailable (never Unenrolled).
    let session_file = fixture
        .records
        .root()
        .join("hosts")
        .join(streams::yaml_store::encode_component(&scope.host))
        .join("conversations")
        .join(format!(
            "{}.yaml",
            streams::yaml_store::encode_component(&fixture.sessions[0].id)
        ));
    let saved_yaml = std::fs::read(&session_file).unwrap();
    std::fs::write(&session_file, b":::corrupt yaml:::").unwrap();
    let corrupt_core = FakeCore::new(vec![Step(
        "terminal.get",
        json!({"terminal":"t1"}),
        Reply::Value(json!({
            "address":"t1",
            "state":"live",
            "pid":10,
            "labels":{(source_key(&helper)):serde_json::to_string(&enrolled_claim).unwrap()}
        })),
    )]);
    let mut corrupt_req = fixture.envelope(json!({
        "kind":"read",
        "selections":[{"kind":"frame","terminal":resource("t1")}]
    }));
    corrupt_req["endpoint"] = json!("core");
    let corrupt_out = fixture
        .command()
        .env("HC_SOCKET", &corrupt_core.socket)
        .args([
            "integration",
            "request",
            "--request-json",
            &corrupt_req.to_string(),
        ])
        .output()
        .unwrap();
    std::fs::write(&session_file, &saved_yaml).unwrap();
    assert!(corrupt_out.status.success());
    let corrupt_reply =
        ModReply::decode(std::str::from_utf8(&corrupt_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::Read {
        values: corrupt_values,
        ..
    }) = corrupt_reply.result
    else {
        panic!("corrupt read reply")
    };
    assert!(matches!(
        &corrupt_values[0],
        WireResult::Ok(ModReadValue::Frame {
            context: FrameContext::Unavailable { .. }
        })
    ));
    assert_eq!(corrupt_core.finish().len(), 2);

    let artifact_request = {
        let mut req = fixture.envelope(json!({
            "kind":"act",
            "action":{
                "kind":"open_reference",
                "reference":{
                    "kind":"artifact",
                    "key":{
                        "host":scope.host,
                        "stream":"work",
                        "session":fixture.sessions[0].id,
                        "name":"result.md"
                    }
                },
                "context":{"kind":"explicit"},
                "at":null
            }
        }));
        req["endpoint"] = json!("core");
        req
    };
    streams::application::artifacts::write_artifact(
        fixture.records.artifacts(),
        &scope.host,
        "write-result-md",
        &streams::application::artifacts::WriteArtifact {
            session_id: fixture.sessions[0].id.clone(),
            stream_id: Some("work".into()),
            name: "result.md".into(),
            description: "Saved output".into(),
            content: Some("Saved artifact body".into()),
        },
    )
    .unwrap();
    let output = fixture
        .command()
        .args([
            "integration",
            "request",
            "--request-json",
            &artifact_request.to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reply = ModReply::decode(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened { value }) = reply.result else {
        panic!("reference opened")
    };
    assert!(value.known.is_empty());
    match &value.result {
        ReferenceOpenResult::LaunchCommand {
            title,
            args,
            placement,
        } => {
            assert_eq!(title, "result.md");
            assert_eq!(args[0], "artifact");
            assert_eq!(args[1], "view");
            assert_eq!(*placement, streams::config::ViewerPlacement::PaneRight);
        }
        other => panic!("expected LaunchCommand, got {other:?}"),
    }

    // Floating placement + terminal mode returns NotApplied.
    assert!(
        fixture
            .command()
            .args(["config", "set", "herdr.viewer.placement", "floating"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let floating_out = fixture
        .command()
        .args([
            "integration",
            "request",
            "--request-json",
            &artifact_request.to_string(),
        ])
        .output()
        .unwrap();
    let floating_reply =
        ModReply::decode(std::str::from_utf8(&floating_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened {
        value: floating_val,
    }) = floating_reply.result
    else {
        panic!("floating reply")
    };
    assert!(matches!(
        floating_val.result,
        ReferenceOpenResult::NotApplied { .. }
    ));

    // Native mode + floating placement returns ShowContent with Markdown content and zero host paths.
    assert!(
        fixture
            .command()
            .args(["config", "set", "herdr.viewer.mode", "native"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let native_out = fixture
        .command()
        .args([
            "integration",
            "request",
            "--request-json",
            &artifact_request.to_string(),
        ])
        .output()
        .unwrap();
    let native_reply = ModReply::decode(std::str::from_utf8(&native_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened { value: native_val }) = native_reply.result
    else {
        panic!("native reply")
    };
    match &native_val.result {
        ReferenceOpenResult::ShowContent {
            title,
            filename,
            content,
            truncated,
            placement,
        } => {
            assert_eq!(title, "result.md");
            assert_eq!(filename, "result.md");
            assert_eq!(content, "Saved artifact body");
            assert!(!truncated);
            assert_eq!(*placement, streams::config::ViewerPlacement::Floating);
        }
        other => panic!("expected ShowContent, got {other:?}"),
    }

    // Missing executable in EDITOR fails cleanly with NotApplied for Document references.
    assert!(
        fixture
            .command()
            .args(["config", "set", "herdr.viewer.mode", "editor"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        fixture
            .command()
            .args(["config", "set", "herdr.viewer.placement", "pane-right"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let root = fixture._directory.path().canonicalize().unwrap();
    let repository = root.join("repo");
    std::fs::create_dir(&repository).unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .arg(&repository)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(repository.join("z.md"), "Owner working document\n").unwrap();
    let doc_url = "https://example.test/source.git";
    let registry =
        streams::runtime::CheckoutRegistry::new(fixture.config.checkout_state_dir(), &scope.host)
            .unwrap();
    let checkout = registry.register(&repository, Some(doc_url)).unwrap();
    let state = streams::runtime::state::RuntimeState::new(
        fixture
            .config
            .runtime_state_dir_for(fixture.records.root())
            .unwrap(),
        &scope.host,
    )
    .unwrap();
    let locations = streams::runtime::DirectoryBindings::new(state.root(), registry);
    locations
        .remember(
            "work",
            &repository,
            Some(&fixture.sessions[0].id),
            false,
            Some(doc_url),
            Some(&checkout.id),
        )
        .unwrap();

    let doc_request = {
        let mut req = fixture.envelope(json!({
            "kind":"act",
            "action":{
                "kind":"open_reference",
                "reference":{
                    "kind":"document",
                    "stream":"work",
                    "document":{
                        "repository":"source",
                        "path":"z.md"
                    },
                    "viewing_session":fixture.sessions[0].id
                },
                "context":{"kind":"explicit"},
                "at":null
            }
        }));
        req["endpoint"] = json!("core");
        req
    };
    let bad_editor_out = fixture
        .command()
        .env("EDITOR", "/nonexistent/editor --wait")
        .args([
            "integration",
            "request",
            "--request-json",
            &doc_request.to_string(),
        ])
        .output()
        .unwrap();
    let bad_editor_reply =
        ModReply::decode(std::str::from_utf8(&bad_editor_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened { value: bad_val }) = bad_editor_reply.result
    else {
        panic!("bad editor reply")
    };
    assert!(matches!(
        bad_val.result,
        ReferenceOpenResult::NotApplied { .. }
    ));

    // Unset EDITOR falls back to less; valid EDITOR with flags selects editor mode.
    let unset_editor_out = fixture
        .command()
        .env_remove("EDITOR")
        .args([
            "integration",
            "request",
            "--request-json",
            &doc_request.to_string(),
        ])
        .output()
        .unwrap();
    let unset_editor_reply =
        ModReply::decode(std::str::from_utf8(&unset_editor_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened { value: unset_val }) = unset_editor_reply.result
    else {
        panic!("unset editor reply")
    };
    match &unset_val.result {
        ReferenceOpenResult::LaunchCommand { args, .. } => {
            assert_eq!(args[0], "integration");
            assert_eq!(args[1], "view-document");
            assert_eq!(args[2], "--mode");
            assert_eq!(args[3], "less");
        }
        other => panic!("expected less fallback LaunchCommand, got {other:?}"),
    }
    let sh_editor_out = fixture
        .command()
        .env("EDITOR", "sh -e")
        .args([
            "integration",
            "request",
            "--request-json",
            &doc_request.to_string(),
        ])
        .output()
        .unwrap();
    let sh_editor_reply =
        ModReply::decode(std::str::from_utf8(&sh_editor_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened { value: sh_val }) = sh_editor_reply.result
    else {
        panic!("editor reply")
    };
    match &sh_val.result {
        ReferenceOpenResult::LaunchCommand { args, .. } => {
            assert_eq!(args[3], "editor");
        }
        other => panic!("expected editor LaunchCommand, got {other:?}"),
    }

    // Native mode caps both Markdown and text at 400 KiB and rejects NUL bytes.
    assert!(
        fixture
            .command()
            .args(["config", "set", "herdr.viewer.mode", "native"])
            .output()
            .unwrap()
            .status
            .success()
    );
    std::fs::write(repository.join("z.md"), "a".repeat(450_000)).unwrap();
    let big_out = fixture
        .command()
        .args([
            "integration",
            "request",
            "--request-json",
            &doc_request.to_string(),
        ])
        .output()
        .unwrap();
    let big_reply = ModReply::decode(std::str::from_utf8(&big_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened { value: big_val }) = big_reply.result else {
        panic!("big reply")
    };
    match &big_val.result {
        ReferenceOpenResult::ShowContent {
            filename,
            content,
            truncated,
            ..
        } => {
            assert_eq!(filename, "z.md");
            assert!(*truncated);
            assert!(content.ends_with("… (file truncated)"));
            assert!(content.len() <= 400_100);
        }
        other => panic!("expected ShowContent, got {other:?}"),
    }
    std::fs::write(repository.join("z.md"), b"abc\0def").unwrap();
    let bin_out = fixture
        .command()
        .args([
            "integration",
            "request",
            "--request-json",
            &doc_request.to_string(),
        ])
        .output()
        .unwrap();
    let bin_reply = ModReply::decode(std::str::from_utf8(&bin_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened { value: bin_val }) = bin_reply.result else {
        panic!("bin reply")
    };
    assert!(matches!(
        bin_val.result,
        ReferenceOpenResult::NotApplied { .. }
    ));

    // Explicit mode/placement overrides on OpenReference override config defaults (for reopening truncated native files).
    std::fs::write(repository.join("z.md"), "hello override\n").unwrap();
    assert!(
        fixture
            .command()
            .args(["config", "set", "herdr.viewer.placement", "floating"])
            .output()
            .unwrap()
            .status
            .success()
    );
    let override_request = {
        let mut req = fixture.envelope(json!({
            "kind": "act",
            "action": {
                "kind": "open_reference",
                "reference": {
                    "kind": "document",
                    "stream": "work",
                    "document": { "repository": "source", "path": "z.md" },
                    "viewing_session": fixture.sessions[0].id
                },
                "context": { "kind": "explicit" },
                "at": null,
                "mode": "editor",
                "placement": "pane-right"
            }
        }));
        req["endpoint"] = json!("core");
        req
    };
    let override_out = fixture
        .command()
        .env("EDITOR", "sh -e")
        .args([
            "integration",
            "request",
            "--request-json",
            &override_request.to_string(),
        ])
        .output()
        .unwrap();
    let override_reply =
        ModReply::decode(std::str::from_utf8(&override_out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::ReferenceOpened {
        value: override_val,
    }) = override_reply.result
    else {
        panic!("override reply")
    };
    match &override_val.result {
        ReferenceOpenResult::LaunchCommand {
            args, placement, ..
        } => {
            assert_eq!(args[3], "editor");
            assert_eq!(*placement, streams::config::ViewerPlacement::PaneRight);
        }
        other => panic!("expected override LaunchCommand, got {other:?}"),
    }
}

#[test]
fn host_streams_lists_all_streams_with_active_session_counts_and_recency() {
    use serde_json::json;
    use std::collections::BTreeMap;
    use streams::{
        application,
        integration::{ModReadValue, ModReply, ModResult, WireResult},
        model::{Record, Status, Stream},
    };

    let fixture = SavedFixture::new();
    let host = fixture.records.host().as_str();
    // Streams with saved artifacts also have a `<stream-id>/` directory in `hosts/<host>/streams/`.
    let artifact_dir = fixture
        .records
        .root()
        .join("hosts")
        .join(streams::yaml_store::encode_component(host))
        .join("streams")
        .join("work")
        .join("artifacts");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    let mut closed = Stream::new("closed", "Closed", None, host).unwrap();
    closed.status = Status::Closed;
    application::create_record(fixture.records.records(), "close", &Record::Stream(closed))
        .unwrap();

    let mut req = fixture.envelope(json!({
        "kind": "read",
        "selections": [{ "kind": "host_streams" }]
    }));
    req["endpoint"] = json!("core");
    let out = fixture
        .command()
        .args(["integration", "request", "--request-json", &req.to_string()])
        .output()
        .unwrap();
    assert!(out.status.success());
    let reply = ModReply::decode(std::str::from_utf8(&out.stdout).unwrap()).unwrap();
    let WireResult::Ok(ModResult::Read { values, .. }) = reply.result else {
        panic!("read reply")
    };
    assert_eq!(values.len(), 1);
    let WireResult::Ok(ModReadValue::HostStreams { value }) = &values[0] else {
        panic!("host streams value")
    };
    assert!(value.unreadable.is_empty());
    let by_id: BTreeMap<_, _> = value
        .streams
        .iter()
        .map(|s| {
            (
                s.id.as_str(),
                (
                    s.status,
                    s.active_sessions,
                    s.total_sessions,
                    s.last_updated_ms,
                ),
            )
        })
        .collect();
    assert_eq!(by_id.len(), 4);
    assert_eq!(by_id["work"].0, Status::Active);
    assert_eq!((by_id["work"].1, by_id["work"].2), (2, 2));
    assert_eq!((by_id["next"].1, by_id["next"].2), (1, 1));
    assert_eq!((by_id["empty"].1, by_id["empty"].2), (0, 0));
    assert_eq!(by_id["closed"].0, Status::Closed);
    assert!(value.streams.iter().all(|s| s.last_updated_ms > 0));
    assert!(
        value
            .streams
            .windows(2)
            .all(|w| w[0].last_updated_ms >= w[1].last_updated_ms)
    );

    // Parked sessions remain eligible for Streams::select_open alongside Active sessions.
    let mut parked_session = fixture.sessions[0].clone();
    parked_session.status = Status::Parked;
    application::save_user_edit(
        fixture.records.records(),
        "park-for-select-open",
        &Record::Session(fixture.sessions[0].clone()),
        &json!({"status": "parked"}).as_object().unwrap().clone(),
        streams::model::TitleDefaults::default(),
    )
    .unwrap();
    let (_, selected_open) = application::integration::Streams::new(&fixture.records)
        .select_open("work", &SessionSelection::AllEligible)
        .unwrap();
    assert!(
        selected_open
            .iter()
            .any(|s| s.id == fixture.sessions[0].id && s.status == Status::Parked)
    );

    // HostStreams and StreamView agree on projected Closed status under decay_days.
    let future_now = value
        .streams
        .iter()
        .map(|s| s.last_updated_ms)
        .max()
        .unwrap_or(0)
        + 20u64 * 86_400_000;
    let decayed_facade =
        application::integration::Streams::new(&fixture.records).with_lifecycle(14, future_now);
    let decayed_replies = decayed_facade
        .read_views(
            &[
                ViewSelection::HostStreams {},
                ViewSelection::SavedStream {
                    stream: "work".into(),
                },
            ],
            Access::Unavailable {
                reason: "none".into(),
            },
            Access::Unavailable {
                reason: "none".into(),
            },
        )
        .unwrap();
    let WireResult::Ok(ModReadValue::HostStreams {
        value: decayed_list,
    }) = &decayed_replies[0]
    else {
        panic!("expected HostStreams");
    };
    let WireResult::Ok(ModReadValue::Stream {
        value: decayed_view,
    }) = &decayed_replies[1]
    else {
        panic!("expected StreamView");
    };
    assert_eq!(
        decayed_list
            .streams
            .iter()
            .find(|s| s.id == "work")
            .unwrap()
            .status,
        Status::Closed
    );
    assert_eq!(decayed_view.status, Status::Closed);
}

#[test]
fn host_listings_show_readable_streams_and_name_unreadable_records() {
    use streams::{application, integration::HostListing, model::RecordKind};

    let fixture = SavedFixture::new();
    let host = streams::yaml_store::encode_component(fixture.records.host().as_str());
    let corrupt = |relative: &str| {
        let path = fixture.records.root().join(relative);
        let mut value: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["record"]["archived"] = json!(false);
        std::fs::write(&path, value.to_string()).unwrap();
    };
    let read = || {
        application::integration::Streams::new(&fixture.records)
            .read_views(
                &[
                    ViewSelection::HostStreams {},
                    ViewSelection::SavedStream {
                        stream: "work".into(),
                    },
                ],
                Access::Unavailable {
                    reason: "none".into(),
                },
                Access::Unavailable {
                    reason: "none".into(),
                },
            )
            .unwrap()
    };
    let listing = |value: &WireResult<ModReadValue>| match value {
        WireResult::Ok(ModReadValue::HostStreams { value }) => value.clone(),
        other => panic!("expected a host listing, got {other:?}"),
    };
    let rows = |listing: &HostListing| {
        let mut rows: Vec<_> = listing
            .streams
            .iter()
            .map(|summary| (summary.id.clone(), summary.total_sessions))
            .collect();
        rows.sort();
        rows
    };
    let unreadable = |listing: &HostListing| {
        for record in &listing.unreadable {
            assert!(
                record.cause.contains("unknown field `archived`"),
                "{record}"
            );
        }
        listing
            .unreadable
            .iter()
            .map(|record| (record.kind, record.path.to_str().unwrap().to_owned()))
            .collect::<Vec<_>>()
    };
    let list = || {
        let output = fixture.command().args(["list", "--json"]).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };

    let stream = format!("hosts/{host}/streams/next.yaml");
    corrupt(&stream);
    let views = read();
    let host_listing = listing(&views[0]);
    assert_eq!(
        rows(&host_listing),
        [("empty".into(), 0), ("work".into(), 2)]
    );
    assert_eq!(
        unreadable(&host_listing),
        [(RecordKind::Stream, stream.clone())]
    );
    assert_eq!(
        host_listing.unreadable[0].to_string(),
        format!(
            "Stream record {stream} is unreadable: {}",
            host_listing.unreadable[0].cause
        )
    );
    assert!(matches!(
        views[1],
        WireResult::Ok(ModReadValue::Stream { .. })
    ));
    let listed = list();
    assert_eq!(
        listed["streams"]
            .as_array()
            .unwrap()
            .iter()
            .map(|stream| stream["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["empty", "work"]
    );
    assert_eq!(
        listed["unreadable"],
        serde_json::to_value(&host_listing.unreadable).unwrap()
    );
    let plain = fixture.command().arg("list").output().unwrap();
    assert!(plain.status.success());
    let stdout = String::from_utf8(plain.stdout).unwrap();
    assert!(
        stdout.contains("/work  ") && !stdout.contains("/next  "),
        "{stdout}"
    );
    let stderr = String::from_utf8(plain.stderr).unwrap();
    assert!(
        stderr.contains(&host_listing.unreadable[0].to_string()),
        "{stderr}"
    );
    assert_eq!(
        host_listing.unreadable[0].host,
        fixture.records.host().as_str()
    );
    assert_eq!(
        host_listing.unreadable[0].repair,
        "Run /streams reset in a conversation on that Stream, or delete or fix the file."
    );
    assert!(
        stderr.contains(&host_listing.unreadable[0].repair),
        "{stderr}"
    );
    assert!(!stderr.contains("another host"), "{stderr}");

    // An unreadable Session's Stream is unknown: the listing names it and counts it nowhere,
    // and a view of a Stream it might belong to shows its readable members and names it.
    let session = format!(
        "hosts/{host}/conversations/{}.yaml",
        streams::yaml_store::encode_component(&fixture.sessions[0].id)
    );
    corrupt(&session);
    let views = read();
    let host_listing = listing(&views[0]);
    assert_eq!(
        rows(&host_listing),
        [("empty".into(), 0), ("work".into(), 1)]
    );
    assert_eq!(
        unreadable(&host_listing),
        [
            (RecordKind::Session, session.clone()),
            (RecordKind::Stream, stream)
        ]
    );
    assert_eq!(
        host_listing.unreadable[0].repair,
        "Run /streams reset in that conversation, or delete or fix the file."
    );
    match &views[1] {
        WireResult::Ok(ModReadValue::Stream { value }) => {
            assert_eq!(value.sessions.len(), 1, "its readable member is shown");
            assert_eq!(
                value.unreadable,
                host_listing.unreadable[..1],
                "the Session it might hold is named"
            );
        }
        other => panic!("expected a Stream view, got {other:?}"),
    }
    assert_eq!(
        list()["unreadable"],
        serde_json::to_value(&host_listing.unreadable).unwrap()
    );

    // Another host's replica records are listed by `streams list`, with their own host and a
    // repair for their kind, but never in this host's listing.
    let other = fixture.records.root().join("hosts/other");
    std::fs::create_dir_all(other.join("streams")).unwrap();
    std::fs::write(other.join("host.yaml"), "not a record").unwrap();
    std::fs::write(other.join("streams/x.yaml"), "not a record").unwrap();
    assert_eq!(listing(&read()[0]), host_listing);
    let foreign: Vec<_> = list()["unreadable"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|record| record["host"] == "other")
        .map(|record| {
            (
                record["kind"].as_str().unwrap().to_owned(),
                record["path"].as_str().unwrap().to_owned(),
                record["repair"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        foreign,
        [
            (
                "host".into(),
                "hosts/other/host.yaml".into(),
                "Delete or fix the file.".into()
            ),
            (
                "stream".into(),
                "hosts/other/streams/x.yaml".into(),
                "Run /streams reset in a conversation on that Stream, or delete or fix the file."
                    .into()
            ),
        ]
    );
    let plain = fixture.command().arg("list").output().unwrap();
    assert!(plain.status.success());
    let stderr = String::from_utf8(plain.stderr).unwrap();
    assert!(
        stderr.contains("Host record hosts/other/host.yaml is unreadable")
            && stderr.contains("/streams reset repairs only this host's records"),
        "{stderr}"
    );
}
