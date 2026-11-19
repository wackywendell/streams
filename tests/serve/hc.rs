use super::*;
use std::{
    collections::BTreeMap,
    os::unix::net::{UnixListener, UnixStream},
};
use streams::{
    frontend::hc::processes::{OwnerProcesses, ProcessObserver},
    runtime::hc::{PiSourceClaim, labels::source_key},
};

fn request(reader: &mut BufReader<UnixStream>, method: &str) -> Value {
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut line = String::new();
    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
    let value: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(value["method"], method);
    value
}
fn reply(reader: &mut BufReader<UnixStream>, request: &Value, result: Value) {
    writeln!(
        reader.get_mut(),
        "{}",
        json!({"jsonrpc":"2.0","id":request["id"],"result":result})
    )
    .unwrap();
}
fn hello(reader: &mut BufReader<UnixStream>) {
    let frame = request(reader, "hello");
    reply(
        reader,
        &frame,
        json!({"generation":1,"framed":false,"core":{"id":"core"},"methods":["hello","terminal.get","terminal.label"]}),
    );
}
fn context() -> Value {
    json!({"conversation":{"harness":"pi","native_id":"pi-session"},"session_file":null})
}
fn terminal(labels: Value) -> Value {
    json!({"address":"t1","state":"live","pid":std::process::id(),"labels":labels})
}
fn register_source(reader: &mut BufReader<UnixStream>) -> String {
    hello(reader);
    let get = request(reader, "terminal.get");
    assert_eq!(get["params"], json!({"terminal":"t1"}));
    reply(reader, &get, terminal(json!({"other":"keep"})));
    let write = request(reader, "terminal.label");
    let labels = write["params"]["labels"].as_object().unwrap();
    assert_eq!(labels.len(), 1);
    let (key, value) = labels.iter().next().unwrap();
    let claim: PiSourceClaim = serde_json::from_str(value.as_str().unwrap()).unwrap();
    assert_eq!(claim.conversation.native_id, "pi-session");
    assert_eq!(source_key(&claim.helper), *key);
    assert!(OwnerProcesses::new().unwrap().alive(&claim.helper).unwrap());
    let mut saved = BTreeMap::from([("other".to_string(), "keep".to_string())]);
    saved.insert(key.clone(), value.as_str().unwrap().to_string());
    reply(reader, &write, terminal(json!(saved)));
    key.clone()
}
fn start(f: &Fixture, socket: &std::path::Path) -> Transport {
    f.ok(&["config", "set", "frontend.mode", "auto"], None);
    let mut command = f.command();
    command.env("HC_SOCKET", socket).env("HC_TERMINAL_ID", "t1");
    let child = Transport::start(command, "pi-session");
    assert_eq!(
        child.next(),
        json!({"type":"ready","protocol":2,"source":streams::child_protocol::SOURCE_DIGEST})
    );
    child
}

#[test]
fn sdk_locator_is_verified_after_enrollment_and_on_a_later_read() {
    use streams::{
        config::Config,
        runtime::{CheckoutRegistry, DirectoryBindings, state::RuntimeState},
    };
    let f = Fixture::new();
    let (saved, _) = f.capture();
    let stream = saved["stream"]["id"].as_str().unwrap();
    let enrollment = json!({"version":2,"catalog_source":saved["catalog_source"],"harness":"pi","harness_session_id":"pi-session","actor":"user","cwd":f.directory.path(),"stream_id":stream,"session":{"title":"Session"}});
    let file = f.directory.path().join("session.jsonl");
    let mut pi = context();
    pi["session_file"] = json!(file);
    let mut child = Transport::start(f.command(), "pi-session");
    assert_eq!(
        child.next(),
        json!({"type":"ready","protocol":2,"source":streams::child_protocol::SOURCE_DIGEST})
    );
    child.send(json!({"type":"request","id":1,"request":{"kind":"enroll","request":enrollment},"pi_context":pi}));
    let first = child.response(1);
    assert_eq!(first["outcome"]["status"], "success", "{first}");
    let session = &first["outcome"]["result"]["session"];
    let host = session["host_id"].as_str().unwrap();
    let session_id = session["id"].as_str().unwrap();
    let home = f.directory.path();
    let environment = BTreeMap::from([
        ("HOME".into(), home.display().to_string()),
        (
            "XDG_CONFIG_HOME".into(),
            home.join("config").display().to_string(),
        ),
        (
            "XDG_DATA_HOME".into(),
            home.join("data").display().to_string(),
        ),
        (
            "XDG_STATE_HOME".into(),
            home.join("state").display().to_string(),
        ),
    ]);
    let config = Config::new(&environment, home).unwrap();
    let state = RuntimeState::new(config.runtime_state_dir_for(&f.root()).unwrap(), host).unwrap();
    let bindings = DirectoryBindings::new(
        state.root(),
        CheckoutRegistry::new(config.checkout_state_dir(), host).unwrap(),
    );
    assert_eq!(bindings.transcript_file(stream, session_id).unwrap(), None);
    fs::write(&file, "{\"type\":\"session\",\"id\":\"wrong\"}\n").unwrap();
    child.send(json!({"type":"request","id":2,"request":{"kind":"enroll","request":enrollment},"pi_context":pi}));
    let warning = child.next();
    assert_eq!(
        warning,
        json!({"type":"warning","message":"Pi session file could not be verified"})
    );
    let wrong = child.response(2);
    assert_eq!(wrong["outcome"]["status"], "success", "{wrong}");
    assert_eq!(wrong["outcome"]["result"]["session"]["id"], session_id);
    assert_eq!(bindings.transcript_file(stream, session_id).unwrap(), None);
    fs::write(&file, "{\"type\":\"session\",\"id\":\"pi-session\"}\n").unwrap();
    child.send(json!({"type":"request","id":3,"request":{"kind":"list_streams"},"pi_context":pi}));
    assert_eq!(child.response(3)["outcome"]["status"], "success");
    child.send(json!({"type":"request","id":4,"request":{"kind":"enroll","request":enrollment},"pi_context":context()}));
    assert_eq!(child.response(4)["outcome"]["status"], "success");
    assert_eq!(
        bindings.transcript_file(stream, session_id).unwrap(),
        Some(file.canonicalize().unwrap())
    );
    let replacement = home.join("replacement.jsonl");
    fs::write(&replacement, "{\"type\":\"session\",\"id\":\"wrong\"}\n").unwrap();
    pi["session_file"] = json!(replacement);
    child.send(json!({"type":"request","id":5,"request":{"kind":"enroll","request":enrollment},"pi_context":pi}));
    assert_eq!(
        child.next(),
        json!({"type":"warning","message":"Pi session file could not be verified"})
    );
    assert_eq!(child.response(5)["outcome"]["status"], "success");
    assert_eq!(
        bindings.transcript_file(stream, session_id).unwrap(),
        Some(file.canonicalize().unwrap())
    );
    fs::write(
        &replacement,
        "{\"type\":\"session\",\"id\":\"pi-session\"}\n",
    )
    .unwrap();
    child.send(json!({"type":"request","id":6,"request":{"kind":"list_streams"},"pi_context":pi}));
    assert_eq!(child.response(6)["outcome"]["status"], "success");
    child.send(json!({"type":"request","id":7,"request":{"kind":"enroll","request":enrollment},"pi_context":context()}));
    assert_eq!(child.response(7)["outcome"]["status"], "success");
    assert_eq!(
        bindings.transcript_file(stream, session_id).unwrap(),
        Some(replacement.canonicalize().unwrap())
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn helper_registers_before_enrollment_and_retires_only_its_own_source_on_eof() {
    let f = Fixture::new();
    let directory = tempfile::Builder::new()
        .prefix("hc-source-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = directory.path().join("s");
    let listener = UnixListener::bind(&socket).unwrap();
    let (observed, observed_rx) = mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let mut reader = BufReader::new(listener.accept().unwrap().0);
        let key = register_source(&mut reader);
        observed.send(key.clone()).unwrap();
        drop(reader);
        let mut reader = BufReader::new(listener.accept().unwrap().0);
        hello(&mut reader);
        let retire = request(&mut reader, "terminal.label");
        assert_eq!(retire["params"], json!({"terminal":"t1","labels":{key:""}}));
        reply(
            &mut reader,
            &retire,
            terminal(json!({"other":"keep","streams.pi.replacement":"keep"})),
        );
    });
    let mut child = start(&f, &socket);
    child.send(json!({"type":"request","id":1,"request":{"kind":"session_started"},"pi_context":context()}));
    let outcome = child.response(1)["outcome"].clone();
    assert_eq!(outcome["result"]["backend"], "hc");
    assert_eq!(
        outcome["result"]["report"]["native"]["reason"],
        "not_enrolled"
    );
    assert!(
        !observed_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .is_empty()
    );
    child.send(
        json!({"type":"request","id":2,"request":{"kind":"list_streams"},"pi_context":context()}),
    );
    assert_eq!(child.response(2)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
    server.join().unwrap();
}

#[test]
fn retirement_timeout_exits_without_retrying_the_label_write() {
    let f = Fixture::new();
    let directory = tempfile::Builder::new()
        .prefix("hc-source-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = directory.path().join("s");
    let listener = UnixListener::bind(&socket).unwrap();
    let (entered, waiting) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let mut first = BufReader::new(listener.accept().unwrap().0);
        register_source(&mut first);
        drop(first);
        let mut second = BufReader::new(listener.accept().unwrap().0);
        request(&mut second, "hello");
        entered.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(10)).unwrap();
        let mut remainder = Vec::new();
        second.read_to_end(&mut remainder).unwrap();
        assert!(
            remainder.is_empty(),
            "unconfirmed retirement must not write or retry"
        );
    });
    let mut child = start(&f, &socket);
    child.send(json!({"type":"request","id":1,"request":{"kind":"session_started"},"pi_context":context()}));
    assert_eq!(child.response(1)["outcome"]["status"], "success");
    let started = std::time::Instant::now();
    let (code, stderr) = child.finish();
    let elapsed = started.elapsed();
    waiting.recv_timeout(Duration::from_secs(1)).unwrap();
    release.send(()).unwrap();
    server.join().unwrap();
    assert_ne!(code, 0);
    assert!(elapsed < Duration::from_secs(10));
    assert!(
        stderr.contains("timed out") || stderr.contains("did not stop promptly"),
        "{stderr}"
    );
}

#[test]
fn forced_helper_death_does_not_attempt_retirement() {
    let f = Fixture::new();
    let directory = tempfile::Builder::new()
        .prefix("hc-source-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = directory.path().join("s");
    let listener = UnixListener::bind(&socket).unwrap();
    let (release, released) = mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let mut reader = BufReader::new(listener.accept().unwrap().0);
        register_source(&mut reader);
        drop(reader);
        released.recv_timeout(Duration::from_secs(5)).unwrap();
        listener.set_nonblocking(true).unwrap();
        assert!(
            matches!(listener.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock)
        );
    });
    let mut child = start(&f, &socket);
    child.send(json!({"type":"request","id":1,"request":{"kind":"session_started"},"pi_context":context()}));
    assert_eq!(child.response(1)["outcome"]["status"], "success");
    child.child.kill().unwrap();
    child.child.wait().unwrap();
    release.send(()).unwrap();
    server.join().unwrap();
}

#[test]
fn cancelled_registration_never_writes_a_source_label() {
    let f = Fixture::new();
    let directory = tempfile::Builder::new()
        .prefix("hc-source-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = directory.path().join("s");
    let listener = UnixListener::bind(&socket).unwrap();
    let (entered, waiting) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let server = thread::spawn(move || {
        let mut reader = BufReader::new(listener.accept().unwrap().0);
        hello(&mut reader);
        let get = request(&mut reader, "terminal.get");
        entered.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(5)).unwrap();
        reply(&mut reader, &get, terminal(json!({})));
        let mut remainder = Vec::new();
        reader.read_to_end(&mut remainder).unwrap();
        assert!(
            remainder.is_empty(),
            "cancelled registration must not write or retry"
        );
    });
    let mut child = start(&f, &socket);
    child.send(json!({"type":"request","id":1,"request":{"kind":"session_started"},"pi_context":context()}));
    waiting.recv_timeout(Duration::from_secs(5)).unwrap();
    child.send(json!({"type":"cancel","id":1}));
    // This read-only response remains independent even when its locator work waits behind hc.
    let file = f.directory.path().join("session.jsonl");
    fs::write(&file, "{\"type\":\"session\",\"id\":\"pi-session\"}\n").unwrap();
    let mut pi = context();
    pi["session_file"] = json!(file);
    child.send(json!({"type":"request","id":2,"request":{"kind":"list_streams"},"pi_context":pi}));
    assert_eq!(child.response(2)["outcome"]["status"], "success");
    release.send(()).unwrap();
    assert_eq!(child.response(1)["outcome"]["status"], "error");
    assert_eq!(child.finish(), (0, String::new()));
    server.join().unwrap();
}

#[test]
fn registration_failure_does_not_gate_catalog_or_retry() {
    let f = Fixture::new();
    let directory = tempfile::Builder::new()
        .prefix("hc-source-")
        .tempdir_in("/tmp")
        .unwrap();
    let mut child = start(&f, &directory.path().join("missing"));
    child.send(json!({"type":"request","id":1,"request":{"kind":"session_started"},"pi_context":context()}));
    assert_eq!(child.response(1)["outcome"]["status"], "error");
    child.send(json!({"type":"request","id":2,"request":{"kind":"attach"},"pi_context":context()}));
    assert!(
        child.response(2)["outcome"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("reload")
    );
    child.send(
        json!({"type":"request","id":3,"request":{"kind":"list_streams"},"pi_context":context()}),
    );
    assert_eq!(child.response(3)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn hc_enrollment_and_apply_bind_space_route_terminal_project_name_and_emit_save_notices() {
    use streams::integration::WorkScope;
    let f = Fixture::new();
    let (saved, _) = f.capture();
    let stream_id = saved["stream"]["id"].as_str().unwrap().to_string();
    let catalog_source = saved["catalog_source"].as_str().unwrap().to_string();
    let host_id = saved["host_id"].as_str().unwrap().to_string();
    let scope = WorkScope {
        catalog: catalog_source.clone(),
        host: host_id,
    };
    let expected_work = streams::runtime::hc::labels::work_value(&scope, &stream_id).unwrap();
    let expected_notice_key = scope.notification_key();

    let directory = tempfile::Builder::new()
        .prefix("hc-enroll-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = directory.path().join("s");
    let listener = UnixListener::bind(&socket).unwrap();
    let stream_for_server = stream_id.clone();
    let work_for_server = expected_work.clone();
    let notice_key_for_server = expected_notice_key.clone();

    let server = thread::spawn(move || {
        let respond_hello = |reader: &mut BufReader<UnixStream>| {
            let frame = request(reader, "hello");
            reply(
                reader,
                &frame,
                json!({
                    "generation": 1,
                    "framed": false,
                    "core": { "id": "core" },
                    "methods": ["hello", "terminal.get", "terminal.label", "doc.get", "doc.set"]
                }),
            );
        };
        let layout_doc = || {
            json!({
                "name": "herdr-app",
                "version": 1,
                "updated_at": 1,
                "value": {
                    "spaces": [{
                        "id": "s1",
                        "name": "Space 1",
                        "tabs": [{
                            "id": "tab1",
                            "name": "Tab 1",
                            "root": {
                                "id": "p1",
                                "type": "block",
                                "content": { "terminal": "t1" }
                            }
                        }]
                    }]
                }
            })
        };

        // 1. SessionStarted (before enrollment): registers source only
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        let source_label_key = register_source(&mut conn);
        drop(conn);

        // 2. Context read: checks caller space association (unassociated) and walks parent chain
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let doc_streams = request(&mut conn, "doc.get");
        assert_eq!(doc_streams["params"], json!({"name":"streams"}));
        reply(
            &mut conn,
            &doc_streams,
            json!({"name":"streams","version":0,"updated_at":0,"value":null}),
        );
        let doc_layout = request(&mut conn, "doc.get");
        assert_eq!(doc_layout["params"], json!({"name":"herdr-app"}));
        reply(&mut conn, &doc_layout, layout_doc());
        let get_caller = request(&mut conn, "terminal.get");
        assert_eq!(get_caller["params"], json!({"terminal":"t1"}));
        reply(&mut conn, &get_caller, terminal(json!({})));
        drop(conn);

        // 3. Enroll follow-on: bind_session + notify_saved share ONE connection.
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let doc_streams = request(&mut conn, "doc.get");
        reply(
            &mut conn,
            &doc_streams,
            json!({"name":"streams","version":0,"updated_at":0,"value":null}),
        );
        let doc_layout = request(&mut conn, "doc.get");
        reply(&mut conn, &doc_layout, layout_doc());
        let doc_set = request(&mut conn, "doc.set");
        assert_eq!(doc_set["params"]["name"], "streams");
        assert_eq!(doc_set["params"]["version"], 0);
        assert_eq!(
            doc_set["params"]["value"]["spaces"]["s1"]["stream"],
            stream_for_server
        );
        reply(
            &mut conn,
            &doc_set,
            json!({"name":"streams","version":1,"updated_at":2,"value":doc_set["params"]["value"]}),
        );
        let set_work = request(&mut conn, "terminal.label");
        assert_eq!(
            set_work["params"],
            json!({"terminal":"t1","labels":{"streams.work":work_for_server}})
        );
        reply(
            &mut conn,
            &set_work,
            terminal(json!({"streams.work":work_for_server})),
        );
        let set_notice = request(&mut conn, "terminal.label");
        let first_token = set_notice["params"]["labels"][&notice_key_for_server]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!first_token.is_empty());
        reply(
            &mut conn,
            &set_notice,
            terminal(json!({(&notice_key_for_server): first_token})),
        );
        drop(conn);

        // 4. Apply follow-on: notify_saved ONLY (no space binding), and rejected label write -> warning frame, save still succeeds!
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let set_notice2 = request(&mut conn, "terminal.label");
        assert!(
            set_notice2["params"]["labels"]
                .get(&notice_key_for_server)
                .is_some()
        );
        writeln!(
            conn.get_mut(),
            "{}",
            json!({"jsonrpc":"2.0","id":set_notice2["id"],"error":{"code":-32602,"message":"simulated notice rejection","data":{"kind":"invalid_params"}}})
        )
        .unwrap();
        drop(conn);

        // 5. Retire on EOF
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let retire = request(&mut conn, "terminal.label");
        assert_eq!(
            retire["params"],
            json!({"terminal":"t1","labels":{source_label_key:""}})
        );
        reply(&mut conn, &retire, terminal(json!({})));
    });

    let mut child = start(&f, &socket);
    child.send(json!({"type":"request","id":1,"request":{"kind":"session_started"},"pi_context":context()}));
    let view1 = child.next();
    assert_eq!(
        view1,
        json!({"type":"view","view":{"desired_pi_name":null}})
    );
    let resp1 = child.next();
    assert_eq!(resp1["id"], 1);
    assert_eq!(
        resp1["outcome"]["result"]["report"]["native"]["reason"],
        "not_enrolled"
    );

    child
        .send(json!({"type":"request","id":2,"request":{"kind":"context"},"pi_context":context()}));
    let resp2 = child.response(2);
    assert_eq!(resp2["outcome"]["status"], "success");

    let enrollment = json!({
        "version": 2,
        "catalog_source": catalog_source,
        "harness": "pi",
        "harness_session_id": "pi-session",
        "actor": "user",
        "cwd": f.directory.path(),
        "stream_id": stream_id,
        "session": { "title": "Initial Pi Title" }
    });
    child.send(json!({"type":"request","id":3,"request":{"kind":"enroll","request":enrollment},"pi_context":context()}));
    let view3 = child.next();
    assert_eq!(
        view3,
        json!({"type":"view","view":{"desired_pi_name":"Initial Pi Title"}})
    );
    let resp3 = child.next();
    assert_eq!(resp3["id"], 3);
    assert_eq!(resp3["outcome"]["status"], "success");
    let enrolled_session = resp3["outcome"]["result"]["session"].clone();

    let apply_req = json!({
        "version": 2,
        "catalog_source": catalog_source,
        "actor": "user",
        "kind": "session",
        "before": enrolled_session,
        "changes": { "title": "Renamed Pi Title" },
        "inputs": [],
        "title_default": "automatic"
    });
    child.send(json!({"type":"request","id":4,"request":{"kind":"apply","request":apply_req},"pi_context":context()}));
    let view4 = child.next();
    assert_eq!(
        view4,
        json!({"type":"view","view":{"desired_pi_name":"Renamed Pi Title"}})
    );
    let warn4 = child.next();
    assert_eq!(warn4["type"], "warning", "{warn4}");
    let resp4 = child.next();
    assert_eq!(resp4["id"], 4);
    assert_eq!(resp4["outcome"]["status"], "success", "{resp4}");

    assert_eq!(child.finish(), (0, String::new()));
    server.join().unwrap();
}

#[test]
fn hc_caller_stream_inherits_from_parent_terminal() {
    use streams::integration::WorkScope;
    let f = Fixture::new();
    let (saved, _) = f.capture();
    let stream_id = saved["stream"]["id"].as_str().unwrap().to_string();
    let catalog_source = saved["catalog_source"].as_str().unwrap().to_string();
    let host_id = saved["host_id"].as_str().unwrap().to_string();
    let scope = WorkScope {
        catalog: catalog_source,
        host: host_id,
    };
    let work = streams::runtime::hc::labels::work_value(&scope, &stream_id).unwrap();

    let directory = tempfile::Builder::new()
        .prefix("hc-parent-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = directory.path().join("s");
    let listener = UnixListener::bind(&socket).unwrap();

    let server = thread::spawn(move || {
        let respond_hello = |reader: &mut BufReader<UnixStream>| {
            let frame = request(reader, "hello");
            reply(
                reader,
                &frame,
                json!({
                    "generation": 1,
                    "framed": false,
                    "core": { "id": "core" },
                    "methods": ["hello", "terminal.get", "terminal.label", "doc.get", "doc.set"]
                }),
            );
        };

        // Context read: t1 is in unassociated space s2, parent t_parent has streams.work.
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let doc_streams = request(&mut conn, "doc.get");
        reply(
            &mut conn,
            &doc_streams,
            json!({"name":"streams","version":0,"updated_at":0,"value":null}),
        );
        let doc_layout = request(&mut conn, "doc.get");
        reply(
            &mut conn,
            &doc_layout,
            json!({
                "name": "herdr-app",
                "version": 1,
                "updated_at": 1,
                "value": {
                    "spaces": [{
                        "id": "s2",
                        "name": "Space 2",
                        "tabs": [{
                            "id": "tab2",
                            "name": "Tab 2",
                            "root": { "id": "p2", "type": "block", "content": { "terminal": "t1" } }
                        }]
                    }]
                }
            }),
        );
        let get_child = request(&mut conn, "terminal.get");
        assert_eq!(get_child["params"], json!({"terminal":"t1"}));
        reply(
            &mut conn,
            &get_child,
            json!({"address":"t1","state":"live","pid":std::process::id(),"parent":"t_parent","labels":{}}),
        );
        let get_parent = request(&mut conn, "terminal.get");
        assert_eq!(get_parent["params"], json!({"terminal":"t_parent"}));
        reply(
            &mut conn,
            &get_parent,
            json!({"address":"t_parent","state":"live","pid":std::process::id(),"labels":{"streams.work":work}}),
        );
        drop(conn);
    });

    let mut child = start(&f, &socket);
    child
        .send(json!({"type":"request","id":1,"request":{"kind":"context"},"pi_context":context()}));
    let resp = child.response(1);
    assert_eq!(resp["outcome"]["status"], "success", "{resp}");
    assert_eq!(resp["outcome"]["result"]["stream"]["id"], stream_id);

    assert_eq!(child.finish(), (0, String::new()));
    server.join().unwrap();
}

#[test]
fn hc_lifecycle_emits_save_notice_when_changed_and_none_on_noop() {
    use streams::integration::WorkScope;
    let f = Fixture::new();
    let (saved, _) = f.capture();
    let stream_id = saved["stream"]["id"].as_str().unwrap().to_string();
    let catalog_source = saved["catalog_source"].as_str().unwrap().to_string();
    let host_id = saved["host_id"].as_str().unwrap().to_string();
    let scope = WorkScope {
        catalog: catalog_source.clone(),
        host: host_id,
    };
    let expected_work = streams::runtime::hc::labels::work_value(&scope, &stream_id).unwrap();
    let expected_notice_key = scope.notification_key();

    let directory = tempfile::Builder::new()
        .prefix("hc-life-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = directory.path().join("s");
    let listener = UnixListener::bind(&socket).unwrap();
    let work_for_server = expected_work;
    let notice_key_for_server = expected_notice_key.clone();

    let server = thread::spawn(move || {
        let respond_hello = |reader: &mut BufReader<UnixStream>| {
            let frame = request(reader, "hello");
            reply(
                reader,
                &frame,
                json!({
                    "generation": 1,
                    "framed": false,
                    "core": { "id": "core" },
                    "methods": ["hello", "terminal.get", "terminal.label", "doc.get", "doc.set"]
                }),
            );
        };

        // 1. SessionStarted (before enrollment): registers source only
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        let source_label_key = register_source(&mut conn);
        drop(conn);

        // 2. Enroll follow-on: bind_session + notify_saved on ONE connection.
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let doc_streams = request(&mut conn, "doc.get");
        reply(
            &mut conn,
            &doc_streams,
            json!({"name":"streams","version":0,"updated_at":0,"value":null}),
        );
        let doc_layout = request(&mut conn, "doc.get");
        reply(
            &mut conn,
            &doc_layout,
            json!({
                "name": "herdr-app",
                "version": 1,
                "updated_at": 1,
                "value": { "spaces": [{ "id": "s1", "name": "S1", "tabs": [{ "id": "tab1", "name": "T1", "root": { "id": "p1", "type": "block", "content": { "terminal": "t1" } } }] }] }
            }),
        );
        let doc_set = request(&mut conn, "doc.set");
        reply(
            &mut conn,
            &doc_set,
            json!({"name":"streams","version":1,"updated_at":2,"value":doc_set["params"]["value"]}),
        );
        let set_work = request(&mut conn, "terminal.label");
        reply(
            &mut conn,
            &set_work,
            terminal(json!({"streams.work": work_for_server})),
        );
        let set_notice = request(&mut conn, "terminal.label");
        let tok = set_notice["params"]["labels"][&notice_key_for_server]
            .as_str()
            .unwrap()
            .to_string();
        reply(
            &mut conn,
            &set_notice,
            terminal(json!({(&notice_key_for_server): tok})),
        );
        drop(conn);

        // 3. Lifecycle park (active -> parked) triggers after_save notice WITHOUT space binding!
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let set_notice2 = request(&mut conn, "terminal.label");
        let tok2 = set_notice2["params"]["labels"][&notice_key_for_server]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(tok2, tok);
        reply(
            &mut conn,
            &set_notice2,
            terminal(json!({(&notice_key_for_server): tok2})),
        );
        drop(conn);

        // 4. Repeat lifecycle park (no-op) does NOT connect to hc or emit a notice.
        // 5. Lifecycle resume (parked -> active) emits a new save notice; next is EOF retire.
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let set_notice3 = request(&mut conn, "terminal.label");
        let tok3 = set_notice3["params"]["labels"][&notice_key_for_server]
            .as_str()
            .unwrap()
            .to_string();
        assert_ne!(tok3, tok2);
        reply(
            &mut conn,
            &set_notice3,
            terminal(json!({(&notice_key_for_server): tok3})),
        );
        drop(conn);

        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let retire = request(&mut conn, "terminal.label");
        assert_eq!(
            retire["params"],
            json!({"terminal":"t1","labels":{source_label_key:""}})
        );
        reply(&mut conn, &retire, terminal(json!({})));
    });

    let mut child = start(&f, &socket);
    child.send(json!({"type":"request","id":1,"request":{"kind":"session_started"},"pi_context":context()}));
    let _view1 = child.next();
    let resp1 = child.next();
    assert_eq!(resp1["id"], 1);

    let enrollment = json!({
        "version": 2,
        "catalog_source": catalog_source,
        "harness": "pi",
        "harness_session_id": "pi-session",
        "actor": "user",
        "cwd": f.directory.path(),
        "stream_id": stream_id,
        "session": { "title": "Life Pi" }
    });
    child.send(json!({"type":"request","id":2,"request":{"kind":"enroll","request":enrollment},"pi_context":context()}));
    let _view2 = child.next();
    let resp2 = child.next();
    assert_eq!(resp2["id"], 2);
    assert_eq!(resp2["outcome"]["status"], "success");

    // First park changes status and emits a view + save notice.
    child.send(json!({"type":"request","id":3,"request":{"kind":"lifecycle","action":"park"},"pi_context":context()}));
    let view3 = child.next();
    assert_eq!(view3["type"], "view");
    let resp3 = child.next();
    assert_eq!(resp3["id"], 3);
    assert_eq!(resp3["outcome"]["status"], "success");
    assert_eq!(resp3["outcome"]["result"]["session_changed"], true);

    // Second park is a no-op and emits NO view/notice.
    child.send(json!({"type":"request","id":4,"request":{"kind":"lifecycle","action":"park"},"pi_context":context()}));
    let resp4 = child.next();
    assert_eq!(resp4["id"], 4);
    assert_eq!(resp4["outcome"]["status"], "success");
    assert_eq!(resp4["outcome"]["result"]["session_changed"], false);

    // Resume (parked -> active) emits view + save notice.
    child.send(json!({"type":"request","id":5,"request":{"kind":"lifecycle","action":"resume"},"pi_context":context()}));
    let view5 = child.next();
    assert_eq!(view5["type"], "view");
    let resp5 = child.next();
    assert_eq!(resp5["id"], 5);
    assert_eq!(resp5["outcome"]["status"], "success");
    assert_eq!(resp5["outcome"]["result"]["session_changed"], true);

    assert_eq!(child.finish(), (0, String::new()));
    server.join().unwrap();
}

#[test]
fn hc_noop_apply_skips_notice_association_and_vcs_while_writing_apply_notifies_without_vcs() {
    use std::os::unix::fs::PermissionsExt;
    use streams::integration::WorkScope;

    let f = Fixture::new();
    let (saved, _) = f.capture();
    let stream_id = saved["stream"]["id"].as_str().unwrap().to_string();
    let catalog_source = saved["catalog_source"].as_str().unwrap().to_string();
    let host_id = saved["host_id"].as_str().unwrap().to_string();
    let scope = WorkScope {
        catalog: catalog_source.clone(),
        host: host_id,
    };
    let expected_work = streams::runtime::hc::labels::work_value(&scope, &stream_id).unwrap();
    let expected_notice_key = scope.notification_key();

    let directory = tempfile::Builder::new()
        .prefix("hc-apply-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = directory.path().join("s");
    let listener = UnixListener::bind(&socket).unwrap();

    // Fake `git` and `jj` on PATH that record any invocation once `trap_armed` exists.
    let trap_bin = directory.path().join("bin");
    fs::create_dir_all(&trap_bin).unwrap();
    let trap_armed = directory.path().join("trap_armed");
    let vcs_log = directory.path().join("vcs.log");
    for tool in ["git", "jj"] {
        let script = trap_bin.join(tool);
        fs::write(
            &script,
            format!(
                "#!/bin/sh\nif [ -f \"{}\" ]; then echo \"{} $*\" >> \"{}\"; exit 99; fi\nexit 1\n",
                trap_armed.display(),
                tool,
                vcs_log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let work_for_server = expected_work;
    let notice_key_for_server = expected_notice_key;

    let server = thread::spawn(move || {
        let respond_hello = |reader: &mut BufReader<UnixStream>| {
            let frame = request(reader, "hello");
            reply(
                reader,
                &frame,
                json!({
                    "generation": 1,
                    "framed": false,
                    "core": { "id": "core" },
                    "methods": ["hello", "terminal.get", "terminal.label", "doc.get", "doc.set"]
                }),
            );
        };

        // 1. SessionStarted: registers source.
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        let source_label_key = register_source(&mut conn);
        drop(conn);

        // 2. Enroll follow-on: associate_caller + notify_saved on ONE connection.
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let doc_streams = request(&mut conn, "doc.get");
        reply(
            &mut conn,
            &doc_streams,
            json!({"name":"streams","version":0,"updated_at":0,"value":null}),
        );
        let doc_layout = request(&mut conn, "doc.get");
        reply(
            &mut conn,
            &doc_layout,
            json!({
                "name": "herdr-app",
                "version": 1,
                "updated_at": 1,
                "value": { "spaces": [{ "id": "s1", "name": "S1", "tabs": [{ "id": "tab1", "name": "T1", "root": { "id": "p1", "type": "block", "content": { "terminal": "t1" } } }] }] }
            }),
        );
        let doc_set = request(&mut conn, "doc.set");
        reply(
            &mut conn,
            &doc_set,
            json!({"name":"streams","version":1,"updated_at":2,"value":doc_set["params"]["value"]}),
        );
        let set_work = request(&mut conn, "terminal.label");
        reply(
            &mut conn,
            &set_work,
            terminal(json!({"streams.work": work_for_server})),
        );
        let set_notice = request(&mut conn, "terminal.label");
        let tok1 = set_notice["params"]["labels"][&notice_key_for_server]
            .as_str()
            .unwrap()
            .to_string();
        reply(
            &mut conn,
            &set_notice,
            terminal(json!({(&notice_key_for_server): tok1})),
        );
        drop(conn);

        // 3. No-op Apply makes ZERO hc connections.
        // 4. Writing Apply sends ONLY notify_saved (no doc.get/doc.set/streams.work).
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let set_notice2 = request(&mut conn, "terminal.label");
        let labels = set_notice2["params"]["labels"].as_object().unwrap();
        assert_eq!(labels.len(), 1);
        let tok2 = labels[&notice_key_for_server].as_str().unwrap().to_string();
        assert_ne!(tok2, tok1);
        reply(
            &mut conn,
            &set_notice2,
            terminal(json!({(&notice_key_for_server): tok2})),
        );
        drop(conn);

        // 5. Repeat of the writing Apply is a no-op and makes ZERO hc connections.
        // 6. Retire on EOF.
        let mut conn = BufReader::new(listener.accept().unwrap().0);
        respond_hello(&mut conn);
        let retire = request(&mut conn, "terminal.label");
        assert_eq!(
            retire["params"],
            json!({"terminal":"t1","labels":{source_label_key:""}})
        );
        reply(&mut conn, &retire, terminal(json!({})));
    });

    f.ok(&["config", "set", "frontend.mode", "auto"], None);
    let mut command = f.command();
    let existing_path = std::env::var("PATH").unwrap_or_default();
    command
        .env("HC_SOCKET", &socket)
        .env("HC_TERMINAL_ID", "t1")
        .env("PATH", format!("{}:{existing_path}", trap_bin.display()));
    let mut child = Transport::start(command, "pi-session");
    assert_eq!(
        child.next(),
        json!({"type":"ready","protocol":2,"source":streams::child_protocol::SOURCE_DIGEST})
    );

    child.send(json!({"type":"request","id":1,"request":{"kind":"session_started"},"pi_context":context()}));
    assert_eq!(child.next()["type"], "view");
    assert_eq!(child.response(1)["outcome"]["status"], "success");

    let enrollment = json!({
        "version": 2,
        "catalog_source": catalog_source,
        "harness": "pi",
        "harness_session_id": "pi-session",
        "actor": "user",
        "cwd": f.directory.path(),
        "stream_id": stream_id,
        "session": { "title": "Initial Title" }
    });
    child.send(json!({"type":"request","id":2,"request":{"kind":"enroll","request":enrollment},"pi_context":context()}));
    assert_eq!(child.next()["type"], "view");
    let resp2 = child.response(2);
    assert_eq!(resp2["outcome"]["status"], "success");
    let enrolled_session = resp2["outcome"]["result"]["session"].clone();

    // Place a `.git` marker in cwd and arm the VCS trap before every Apply.
    fs::create_dir_all(f.directory.path().join(".git")).unwrap();
    fs::write(&trap_armed, "armed").unwrap();

    // No-op Apply (same title, no changes): no view frame, no hc notice/association, no VCS.
    let noop_apply = json!({
        "version": 2,
        "catalog_source": catalog_source,
        "actor": "automatic",
        "kind": "session",
        "before": enrolled_session,
        "changes": {},
        "inputs": [],
        "title_default": "automatic"
    });
    child.send(json!({"type":"request","id":3,"request":{"kind":"apply","request":noop_apply},"pi_context":context()}));
    let resp3 = child.next();
    assert_eq!(
        resp3["type"], "response",
        "no-op apply emitted unexpected frame: {resp3}"
    );
    assert_eq!(resp3["id"], 3);
    assert_eq!(resp3["outcome"]["status"], "success");
    assert_eq!(resp3["outcome"]["result"]["save"]["local"], "unchanged");
    assert_eq!(
        resp3["outcome"]["result"]["save"]["domain"]["outcome"],
        "no-change"
    );
    assert!(
        !vcs_log.exists(),
        "no-op apply spawned VCS: {:?}",
        fs::read_to_string(&vcs_log)
    );

    // Writing Apply: emits view + save notice, no space association, no VCS.
    let write_apply = json!({
        "version": 2,
        "catalog_source": catalog_source,
        "actor": "user",
        "kind": "session",
        "before": enrolled_session,
        "changes": { "title": "Updated Title" },
        "inputs": [],
        "title_default": "automatic"
    });
    child.send(json!({"type":"request","id":4,"request":{"kind":"apply","request":write_apply},"pi_context":context()}));
    let view4 = child.next();
    assert_eq!(
        view4,
        json!({"type":"view","view":{"desired_pi_name":"Updated Title"}})
    );
    let resp4 = child.response(4);
    assert_eq!(resp4["outcome"]["status"], "success");
    assert_eq!(resp4["outcome"]["result"]["save"]["local"], "persisted");
    assert!(
        !vcs_log.exists(),
        "writing apply spawned VCS: {:?}",
        fs::read_to_string(&vcs_log)
    );

    // Repeating the same Apply is now a no-op: no view, no hc notice, no VCS.
    child.send(json!({"type":"request","id":5,"request":{"kind":"apply","request":write_apply},"pi_context":context()}));
    let resp5 = child.next();
    assert_eq!(
        resp5["type"], "response",
        "repeat apply emitted unexpected frame: {resp5}"
    );
    assert_eq!(resp5["id"], 5);
    assert_eq!(resp5["outcome"]["status"], "success");
    assert_eq!(resp5["outcome"]["result"]["save"]["local"], "unchanged");
    assert!(
        !vcs_log.exists(),
        "repeat apply spawned VCS: {:?}",
        fs::read_to_string(&vcs_log)
    );

    assert_eq!(child.finish(), (0, String::new()));
    server.join().unwrap();
}
