//! Minimal protocol-22 Herdr fake: one reply per connection, like Herdr 0.9.5.
//! Shared by unit tests (via `#[path]`) and integration tests.
#![allow(dead_code)]

use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// What the fake writes back for one request.
pub enum Reply {
    Result(Value),
    Error(Value),
    /// Exact bytes, including malformed frames; empty closes without a reply.
    Raw(Vec<u8>),
}

type Handler = dyn Fn(&str, &Value) -> Reply + Send + Sync;

pub struct FakeHerdr {
    pub socket: PathBuf,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    failure: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    subscriptions: Arc<Mutex<Vec<UnixStream>>>,
}

pub fn pong() -> Reply {
    Reply::Result(json!({"type":"pong","version":"0.9.5","protocol":22}))
}

impl FakeHerdr {
    /// Serve at `socket`; `ping` gets a protocol-22 pong unless `handler` is
    /// asked first through [`FakeHerdr::raw`].
    pub fn start(
        socket: &Path,
        handler: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static,
    ) -> Self {
        Self::raw(socket, move |method, params| match method {
            "ping" => pong(),
            _ => handler(method, params),
        })
    }

    pub fn raw(
        socket: &Path,
        handler: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static,
    ) -> Self {
        let listener = UnixListener::bind(socket).unwrap();
        let handler: Arc<Handler> = Arc::new(handler);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let failure = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let subscriptions = Arc::new(Mutex::new(Vec::new()));
        std::thread::spawn({
            let subscriptions = subscriptions.clone();
            let (requests, failure, stop) = (requests.clone(), failure.clone(), stop.clone());
            move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    let mut reader = BufReader::new(stream.unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        continue;
                    }
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let method = request["method"].as_str().unwrap().to_owned();
                    let params = request["params"].clone();
                    requests
                        .lock()
                        .unwrap()
                        .push((method.clone(), params.clone()));
                    let handler = handler.clone();
                    // Assertions inside a fake must fail the test, not look like
                    // a dropped socket that fail-closed code may accept.
                    let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        handler(&method, &params)
                    }));
                    let subscribed = matches!(&reply, Ok(Reply::Result(result))
                        if method == "events.subscribe" && result["type"] == "subscription_started");
                    let bytes = match reply {
                        Ok(Reply::Result(result)) => {
                            format!("{}\n", json!({"id":request["id"],"result":result}))
                                .into_bytes()
                        }
                        Ok(Reply::Error(error)) => {
                            format!("{}\n", json!({"id":request["id"],"error":error})).into_bytes()
                        }
                        Ok(Reply::Raw(bytes)) => bytes,
                        Err(panic) => {
                            let message = panic
                                .downcast_ref::<String>()
                                .cloned()
                                .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).into()))
                                .unwrap_or_default();
                            failure
                                .lock()
                                .unwrap()
                                .get_or_insert(format!("{method}: {message}"));
                            Vec::new()
                        }
                    };
                    if reader.get_mut().write_all(&bytes).is_ok() && subscribed {
                        subscriptions.lock().unwrap().push(reader.into_inner());
                    }
                }
            }
        });
        Self {
            socket: socket.into(),
            requests,
            failure,
            stop,
            subscriptions,
        }
    }

    /// Send an unsolicited protocol event to every acknowledged subscriber.
    pub fn emit(&self, event: Value) {
        let bytes = format!("{event}\n");
        let mut subscriptions = self.subscriptions.lock().unwrap();
        assert!(!subscriptions.is_empty(), "no acknowledged subscriber");
        subscriptions.retain_mut(|stream| stream.write_all(bytes.as_bytes()).is_ok());
    }

    pub fn disconnect_subscriptions(&self) {
        for stream in self.subscriptions.lock().unwrap().drain(..) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Every request in arrival order, including `ping`.
    pub fn requests(&self) -> Vec<(String, Value)> {
        self.requests.lock().unwrap().clone()
    }

    pub fn methods(&self) -> Vec<String> {
        self.requests()
            .into_iter()
            .map(|(method, _)| method)
            .collect()
    }

    pub fn clear(&self) {
        self.requests.lock().unwrap().clear();
    }
}

/// Serves queued replies in order to every request but `ping`; an unexpected
/// request fails the test.
pub struct Scripted {
    // Declared before the directory holding its socket.
    pub server: FakeHerdr,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    _directory: tempfile::TempDir,
}

impl Scripted {
    pub fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let replies = Arc::new(Mutex::new(VecDeque::<Reply>::new()));
        let server = FakeHerdr::start(&directory.path().join("herdr.sock"), {
            let replies = replies.clone();
            move |method, params| {
                let reply = replies.lock().unwrap().pop_front();
                reply.unwrap_or_else(|| panic!("unexpected request: {method} {params}"))
            }
        });
        Self {
            server,
            replies,
            _directory: directory,
        }
    }

    pub fn reply(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }

    pub fn result(&self, result: Value) {
        self.reply(Reply::Result(result));
    }

    pub fn environment(&self) -> BTreeMap<String, String> {
        BTreeMap::from([(
            "HERDR_SOCKET_PATH".into(),
            self.server.socket.to_str().unwrap().into(),
        )])
    }

    pub fn exhausted(&self) {
        assert!(self.replies.lock().unwrap().is_empty());
    }
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake the accept loop; never join, as the socket may already be gone.
        let _ = std::os::unix::net::UnixStream::connect(&self.socket);
        let failure = self.failure.lock().unwrap().take();
        if let Some(failure) = failure
            && !std::thread::panicking()
        {
            panic!("fake Herdr handler panicked: {failure}");
        }
    }
}
