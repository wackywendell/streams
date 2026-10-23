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
fn cancellation_wakes_a_blocked_handshake_or_snapshot_without_releasing_the_server() {
    for method in ["ping", "session.snapshot"] {
        let (_directory, listener, path) = server();
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, wait) = std::sync::mpsc::channel();
        let server = thread::spawn(move || {
            if method != "ping" {
                ping(&listener);
            }
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let frame =
                read_frame(&mut reader, Some(Instant::now() + Duration::from_secs(2))).unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&frame).unwrap()["method"],
                method
            );
            entered.send(()).unwrap();
            let _ = wait.recv();
            listener.set_nonblocking(true).unwrap();
            assert!(
                listener.accept().is_err(),
                "cancelled work must not reconnect"
            );
        });
        let cancel = crate::cancellation::Cancellation::new().unwrap();
        let flag = cancel.clone();
        let (done, result) = std::sync::mpsc::channel();
        let worker = thread::spawn(move || {
            let result: Result<(), FrontendError> =
                SocketClient::connect_cancellable(path, 30.0, flag).and_then(|client| {
                    let error = client.request("session.snapshot", json!({})).unwrap_err();
                    assert!(
                        client
                            .request("session.snapshot", json!({}))
                            .unwrap_err()
                            .message
                            .contains("cancelled")
                    );
                    Err(error)
                });
            done.send(result.err().unwrap().message).unwrap();
        });
        observed.recv_timeout(Duration::from_secs(3)).unwrap();
        cancel.cancel().unwrap();
        assert!(
            result
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .contains("cancelled")
        );
        // Completion precedes release: cancellation did not wait for the response deadline.
        release.send(()).unwrap();
        worker.join().unwrap();
        server.join().unwrap();
    }
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
