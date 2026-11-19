use super::*;
use crate::fake_herdr::FakeHerdr;

fn caller(environment: BTreeMap<String, String>) -> Caller {
    Caller {
        scope: WorkScope {
            catalog: "catalog".into(),
            host: "host".into(),
        },
        environment,
        cwd: "/tmp".into(),
        timeout: 1.0,
    }
}

#[test]
fn hc_without_a_terminal_associates_and_notifies_nothing_and_never_connects() {
    // No HC_SOCKET or HC_SESSION either: any connection attempt would fail.
    let mut frontend = connect(FrontendId::Hc, caller(BTreeMap::new()));
    frontend.associate_caller("stream").unwrap();
    frontend.notify_saved().unwrap();
    assert_eq!(frontend.context_stream(), None);
    assert!(frontend.caller_stream().is_err());
}

#[test]
fn hc_with_a_terminal_but_no_endpoint_reports_the_failure() {
    let environment = BTreeMap::from([("HC_TERMINAL_ID".into(), "t1".into())]);
    let mut frontend = connect(FrontendId::Hc, caller(environment));
    assert!(frontend.associate_caller("stream").is_err());
    assert!(frontend.notify_saved().is_err());
    assert_eq!(frontend.context_stream(), None);
}

#[test]
fn observed_herdr_answers_from_the_child_observation_without_a_snapshot() {
    let temp = tempfile::Builder::new()
        .prefix("frontends-")
        .tempdir_in("/tmp")
        .unwrap();
    let server = FakeHerdr::start(&temp.path().join("herdr.sock"), |method, _| {
        panic!("unexpected {method}")
    });
    let environment = BTreeMap::from([
        ("HERDR_ENV".into(), "1".into()),
        ("HERDR_PANE_ID".into(), "owner".into()),
        (
            "HERDR_SOCKET_PATH".into(),
            server.socket.to_str().unwrap().into(),
        ),
    ]);
    let client = HerdrClient::connect(environment.clone(), 1.0).unwrap();
    for observed in [Some("stream".to_string()), None] {
        let mut frontend =
            HerdrFrontend::observed(caller(environment.clone()), &client, observed.clone())
                .unwrap();
        assert_eq!(frontend.context_stream(), observed);
        assert_eq!(frontend.caller_stream().unwrap(), observed);
        assert_eq!(frontend.workspace_stream().unwrap(), observed);
        frontend.associate_caller("stream").unwrap();
        frontend.notify_saved().unwrap();
    }
    assert_eq!(server.methods(), ["ping"]);
}

#[test]
fn cmux_names_no_stream_and_sees_nothing_running() {
    let mut frontend = connect(FrontendId::Cmux, caller(BTreeMap::new()));
    assert_eq!(frontend.context_stream(), None);
    assert!(frontend.workspace_stream().is_err());
    let session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "stream",
        "pi",
        "native",
        "Work",
    )
    .unwrap();
    let selected = SelectedConversation {
        session: &session,
        locator: None,
    };
    assert!(!frontend.running(&selected).unwrap());
}

#[test]
fn an_hc_opening_always_reports_the_hc_backend() {
    assert_eq!(serde_json::to_value(HcBackend).unwrap(), "hc");
}

#[test]
fn hc_associate_caller_and_notify_saved_share_one_connection_even_after_correlated_error() {
    use serde_json::{Value, json};
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
        thread,
    };

    let temp = tempfile::Builder::new()
        .prefix("frontends-hc-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = temp.path().join("hc.sock");
    let listener = UnixListener::bind(&socket).unwrap();

    let server = thread::spawn(move || {
        // Accepts strictly ONE connection for both associate_caller and notify_saved.
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let next = |reader: &mut BufReader<std::os::unix::net::UnixStream>| -> Value {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            serde_json::from_str(&line).unwrap()
        };

        let hello = next(&mut reader);
        assert_eq!(hello["method"], "hello");
        writeln!(
            reader.get_mut(),
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": hello["id"],
                "result": {
                    "generation": 1,
                    "framed": false,
                    "core": { "id": "core" },
                    "methods": ["hello", "terminal.get", "terminal.label", "doc.get", "doc.set"]
                }
            })
        )
        .unwrap();

        // associate_caller starts with doc.get("streams"); reply with a correlated JSON-RPC error.
        let doc_get = next(&mut reader);
        assert_eq!(doc_get["method"], "doc.get");
        writeln!(
            reader.get_mut(),
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": doc_get["id"],
                "error": { "code": -32602, "message": "simulated doc error", "data": { "kind": "invalid_params" } }
            })
        )
        .unwrap();

        // notify_saved reuses the SAME connection and succeeds.
        let label = next(&mut reader);
        assert_eq!(label["method"], "terminal.label");
        let labels = label["params"]["labels"].clone();
        writeln!(
            reader.get_mut(),
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": label["id"],
                "result": {
                    "address": "t1",
                    "state": "live",
                    "pid": std::process::id(),
                    "labels": labels
                }
            })
        )
        .unwrap();

        // Connection closes cleanly with no further requests.
        let mut eof = String::new();
        assert_eq!(reader.read_line(&mut eof).unwrap(), 0);
    });

    let environment = BTreeMap::from([
        ("HC_SOCKET".into(), socket.to_str().unwrap().into()),
        ("HC_TERMINAL_ID".into(), "t1".into()),
    ]);
    let mut frontend = connect(FrontendId::Hc, caller(environment));
    assert!(frontend.associate_caller("stream").is_err());
    frontend.notify_saved().unwrap();
    drop(frontend);
    server.join().unwrap();
}

#[test]
fn hc_notify_saved_reconnects_after_associate_caller_ends_the_connection() {
    use serde_json::{Value, json};
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::{UnixListener, UnixStream},
        thread,
    };

    let temp = tempfile::Builder::new()
        .prefix("frontends-hc-eof-")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = temp.path().join("hc.sock");
    let listener = UnixListener::bind(&socket).unwrap();

    let server = thread::spawn(move || {
        let next = |reader: &mut BufReader<UnixStream>| -> Value {
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0);
            serde_json::from_str(&line).unwrap()
        };
        let respond_hello = |reader: &mut BufReader<UnixStream>| {
            let hello = next(reader);
            assert_eq!(hello["method"], "hello");
            writeln!(
                reader.get_mut(),
                "{}",
                json!({
                    "jsonrpc": "2.0",
                    "id": hello["id"],
                    "result": {
                        "generation": 1,
                        "framed": false,
                        "core": { "id": "core" },
                        "methods": ["hello", "terminal.get", "terminal.label", "doc.get", "doc.set"]
                    }
                })
            )
            .unwrap();
        };

        // Connection 1: associate_caller hits EOF after doc.get, ending the HcClient.
        let (stream1, _) = listener.accept().unwrap();
        let mut reader1 = BufReader::new(stream1);
        respond_hello(&mut reader1);
        let doc_get = next(&mut reader1);
        assert_eq!(doc_get["method"], "doc.get");
        drop(reader1);

        // Connection 2: notify_saved opens a fresh connection and delivers the notice.
        let (stream2, _) = listener.accept().unwrap();
        let mut reader2 = BufReader::new(stream2);
        respond_hello(&mut reader2);
        let label = next(&mut reader2);
        assert_eq!(label["method"], "terminal.label");
        let labels = label["params"]["labels"].clone();
        writeln!(
            reader2.get_mut(),
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": label["id"],
                "result": {
                    "address": "t1",
                    "state": "live",
                    "pid": std::process::id(),
                    "labels": labels
                }
            })
        )
        .unwrap();

        let mut eof = String::new();
        assert_eq!(reader2.read_line(&mut eof).unwrap(), 0);
    });

    let environment = BTreeMap::from([
        ("HC_SOCKET".into(), socket.to_str().unwrap().into()),
        ("HC_TERMINAL_ID".into(), "t1".into()),
    ]);
    let mut frontend = connect(FrontendId::Hc, caller(environment));
    assert!(frontend.associate_caller("stream").is_err());
    frontend.notify_saved().unwrap();
    drop(frontend);
    server.join().unwrap();
}
