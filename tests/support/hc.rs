#![allow(dead_code)]

use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

pub enum Reply {
    Value(Value),
    Reject(&'static str),
    Lost,
}

pub struct Step {
    pub method: &'static str,
    pub params: Option<Value>,
    pub reply: Reply,
}

impl Step {
    pub fn exact(method: &'static str, params: Value, reply: Reply) -> Self {
        Self {
            method,
            params: Some(params),
            reply,
        }
    }

    pub fn any(method: &'static str, reply: Reply) -> Self {
        Self {
            method,
            params: None,
            reply,
        }
    }
}

pub struct FakeCore {
    _directory: tempfile::TempDir,
    pub socket: PathBuf,
    calls: Arc<Mutex<Vec<Value>>>,
    task: thread::JoinHandle<()>,
}

impl FakeCore {
    pub fn new(steps: Vec<Step>) -> Self {
        let count = steps.len();
        Self::with_handler(
            move |request, index| {
                let step = &steps[index];
                assert_eq!(request["method"], step.method);
                if let Some(expected) = &step.params {
                    assert_eq!(&request["params"], expected);
                }
                match &step.reply {
                    Reply::Value(value) => Reply::Value(value.clone()),
                    Reply::Reject(kind) => Reply::Reject(kind),
                    Reply::Lost => Reply::Lost,
                }
            },
            count,
        )
    }

    pub fn with_handler(
        mut handler: impl FnMut(&Value, usize) -> Reply + Send + 'static,
        count: usize,
    ) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("hc-fake-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = directory.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        let calls = Arc::new(Mutex::new(vec![]));
        let observed = calls.clone();
        let task = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut reader = BufReader::new(stream);
            // First frame is always the generation-1 hello handshake.
            let mut line = String::new();
            assert_ne!(reader.read_line(&mut line).unwrap(), 0, "missing hello");
            let hello: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(
                hello,
                json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"generation":1,"framed":false}})
            );
            observed.lock().unwrap().push(hello.clone());
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
            for index in 0..count {
                let mut line = String::new();
                assert_ne!(
                    reader.read_line(&mut line).unwrap(),
                    0,
                    "missing step {index}"
                );
                let request: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["jsonrpc"], "2.0");
                assert_eq!(request["id"], index + 2);
                observed.lock().unwrap().push(request.clone());
                let response = match handler(&request, index) {
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

    pub fn finish(self) -> Vec<Value> {
        self.task.join().unwrap();
        Arc::try_unwrap(self.calls).unwrap().into_inner().unwrap()
    }
}
