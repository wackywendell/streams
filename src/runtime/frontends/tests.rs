use super::*;
use crate::fake_herdr::FakeHerdr;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    thread,
    time::Duration,
};

fn caller(environment: BTreeMap<String, String>) -> Caller {
    Caller {
        catalog_source: "catalog".into(),
        host: "host".into(),
        environment,
        cwd: PathBuf::from("/tmp"),
        timeout: 1.0,
    }
}

#[test]
fn hc_without_terminal_id_is_a_no_op() {
    let mut frontend = connect(FrontendId::Hc, caller(BTreeMap::new()));
    frontend.notify_saved().unwrap();
    assert_eq!(frontend.context_stream(), None);
    assert_eq!(frontend.caller_stream().unwrap(), None);
}

#[test]
fn hc_save_notice_writes_fixed_streams_saved_label() {
    let dir = tempfile::tempdir_in("/tmp").unwrap();
    let socket = dir.path().join("hc.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    let server = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let hello: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(hello["method"], "hello");
        writeln!(
            reader.get_mut(),
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "generation": 1,
                    "framed": false,
                    "core": { "id": "core" },
                    "methods": ["hello", "terminal.label"]
                }
            })
        )
        .unwrap();
        line.clear();
        reader.read_line(&mut line).unwrap();
        let label: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(label["method"], "terminal.label");
        assert_eq!(label["params"]["terminal"], "t1");
        let token = label["params"]["labels"]["streams.saved"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(!token.is_empty());
        writeln!(
            reader.get_mut(),
            "{}",
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "result": {
                    "address": "t1",
                    "state": "live",
                    "labels": { "streams.saved": token }
                }
            })
        )
        .unwrap();
    });
    let environment = BTreeMap::from([
        ("HC_TERMINAL_ID".into(), "t1".into()),
        ("HC_SOCKET".into(), socket.to_string_lossy().into_owned()),
    ]);
    let mut frontend = connect(FrontendId::Hc, caller(environment));
    frontend.notify_saved().unwrap();
    server.join().unwrap();
}

#[test]
fn observed_herdr_reuses_its_workspace_stream() {
    let directory = tempfile::tempdir_in("/tmp").unwrap();
    let socket = directory.path().join("herdr.sock");
    let server = FakeHerdr::start(&socket, |method, _| {
        panic!("observed Herdr should not call {method}")
    });
    let environment = BTreeMap::from([(
        "HERDR_SOCKET_PATH".into(),
        socket.to_string_lossy().into_owned(),
    )]);
    let client = HerdrClient::connect(environment.clone(), 1.0).unwrap();
    for observed in [Some("effort".to_owned()), None] {
        let mut frontend = HerdrFrontend::observed(
            caller(environment.clone()),
            client.clone(),
            observed.clone(),
        );
        assert_eq!(frontend.context_stream(), observed);
        assert_eq!(frontend.caller_stream().unwrap(), observed);
        frontend.notify_saved().unwrap();
    }
    drop(server);
}

#[test]
fn cmux_stub_is_harmless_no_op() {
    let mut frontend = connect(FrontendId::Cmux, caller(BTreeMap::new()));
    assert_eq!(frontend.context_stream(), None);
    assert_eq!(frontend.caller_stream().unwrap(), None);
    frontend.notify_saved().unwrap();
}
