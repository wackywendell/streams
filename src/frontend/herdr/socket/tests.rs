use super::*;
use std::os::unix::net::UnixListener;
use std::sync::Arc;
use std::thread;

pub(crate) fn schema() -> Value {
    serde_json::from_str(include_str!(
        "../../../../tests/fixtures/herdr-api-schema.json"
    ))
    .unwrap()
}

#[test]
fn pinned_protocol_matches_schema() {
    assert_eq!(schema()["protocol"], PROTOCOL);
}

pub(crate) fn server() -> (tempfile::TempDir, UnixListener, PathBuf) {
    let directory = tempfile::Builder::new()
        .prefix("herdr-test-")
        .tempdir_in("/tmp")
        .unwrap();
    let path = directory.path().join("api.sock");
    let listener = UnixListener::bind(&path).unwrap();
    (directory, listener, path)
}

pub(crate) fn request(
    listener: &UnixListener,
    method: &str,
    result: Value,
) -> BufReader<UnixStream> {
    let (stream, _) = listener.accept().unwrap();
    let mut reader = BufReader::new(stream);
    let frame = read_frame(&mut reader, Some(Instant::now() + Duration::from_secs(2))).unwrap();
    let request: Value = serde_json::from_slice(&frame).unwrap();
    assert_eq!(request["method"], method);
    writeln!(
        reader.get_mut(),
        "{}",
        json!({"id":request["id"],"result":result})
    )
    .unwrap();
    reader
}

pub(crate) fn ping(listener: &UnixListener) {
    request(
        listener,
        "ping",
        json!({"type":"pong","version":"0.9.5","protocol":22}),
    );
}

#[test]
fn separate_request_sockets_and_one_version_check_for_clones() {
    let (_directory, listener, path) = server();
    let handle = thread::spawn(move || {
        ping(&listener);
        for _ in 0..2 {
            request(
                &listener,
                "session.snapshot",
                json!({"type":"session_snapshot","snapshot":{}}),
            );
        }
    });
    let client = Arc::new(SocketClient::connect(path, 1.0).unwrap());
    for client in [Arc::clone(&client), client] {
        assert_eq!(
            client.request("session.snapshot", json!({})).unwrap()["type"],
            "session_snapshot"
        );
    }
    handle.join().unwrap();
}

#[test]
fn unknown_protocol_and_mismatched_ids_fail_closed() {
    for response in [
        json!({"id":"1","result":{"type":"pong","version":"0.9.5","protocol":23}}),
        json!({"id":"wrong","result":{"type":"pong","version":"0.9.5","protocol":22}}),
        json!({"id":"1","error":{"code":"unsupported","message":"no"}}),
    ] {
        let (_directory, listener, path) = server();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            writeln!(stream, "{response}").unwrap();
        });
        assert!(SocketClient::connect(path, 1.0).is_err());
        handle.join().unwrap();
    }
}

#[test]
fn partial_eof_and_timeout_are_bounded_errors() {
    for timeout in [false, true] {
        let (_directory, listener, path) = server();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap())
                .read_line(&mut line)
                .unwrap();
            write!(stream, "{{\"id\":").unwrap();
            if timeout {
                thread::sleep(Duration::from_millis(100));
            }
        });
        assert!(SocketClient::connect(path, 0.03).is_err());
        handle.join().unwrap();
    }
}
