use super::*;
use std::{
    collections::BTreeMap,
    os::unix::net::{UnixListener, UnixStream},
};
use streams::{
    frontend::{EndpointId, hc::processes::OwnerProcesses},
    runtime::hc::{PiSourceClaim, labels::source_key, verify_claim_route},
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
    assert_eq!(source_key(&claim.helper).unwrap(), *key);
    verify_claim_route(
        key,
        value.as_str().unwrap(),
        &EndpointId("core".into()),
        std::process::id(),
        &OwnerProcesses::new().unwrap(),
    )
    .unwrap();
    let mut saved = BTreeMap::from([("other".to_string(), "keep".to_string())]);
    saved.insert(key.clone(), value.as_str().unwrap().to_string());
    reply(reader, &write, terminal(json!(saved)));
    key.clone()
}
fn start(f: &Fixture, socket: &std::path::Path) -> Transport {
    f.ok(&["config", "set", "frontend.mode", "auto"], None);
    let mut command = f.command();
    command.env("HC_SOCKET", socket).env("HC_TERMINAL_ID", "t1");
    let child = Transport::start_with_sdk(command, "pi-session", Some(std::process::id()));
    assert_eq!(child.next(), json!({"type":"ready","protocol":2}));
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
    let enrollment = json!({"version":2,"operation_id":"locator-enroll","catalog_source":saved["catalog_source"],"harness":"pi","harness_session_id":"pi-session","actor":"user","cwd":f.directory.path(),"stream_id":stream,"session":{"title":"Session"}});
    let file = f.directory.path().join("session.jsonl");
    let mut pi = context();
    pi["session_file"] = json!(file);
    let mut child = Transport::start_with_sdk(f.command(), "pi-session", Some(std::process::id()));
    assert_eq!(child.next(), json!({"type":"ready","protocol":2}));
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
    let wrong = child.response(2);
    assert_eq!(wrong["outcome"]["status"], "saved_with_warnings", "{wrong}");
    assert_eq!(wrong["outcome"]["result"]["session"]["id"], session_id);
    assert_eq!(
        wrong["outcome"]["warnings"],
        json!(["Pi session file could not be verified"])
    );
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
        child.response(5)["outcome"]["status"],
        "saved_with_warnings"
    );
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
        "unavailable"
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
