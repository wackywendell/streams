//! One owner-local hc connection per operation, pinned by a generation-1 hello.
pub mod processes;
use super::{CommandEvidence, CommandOutcome, EndpointId, FrontendError, NativeRequest, socket_io};
use crate::cancellation::Cancellation;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, BufReader, Write},
    os::unix::ffi::OsStrExt,
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    time::Instant,
};

const MAX_FRAME: usize = 4 * 1024 * 1024;

#[derive(Clone)]
pub struct HcEndpoint {
    socket: PathBuf,
}
impl HcEndpoint {
    pub fn resolve(
        environment: &BTreeMap<String, String>,
        cwd: &Path,
    ) -> Result<Self, FrontendError> {
        if !cwd.is_absolute() {
            return Err(FrontendError::invalid(
                "hc operation directory must be absolute",
            ));
        }
        let absolute = |path: PathBuf| {
            if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            }
        };
        let value = |key: &str| environment.get(key).filter(|value| !value.is_empty());
        let session = value("HC_SESSION")
            .map(|value| value.trim())
            .unwrap_or("default");
        let session = if session.is_empty() {
            "default"
        } else {
            session
        };
        if session == "."
            || session == ".."
            || session
                .chars()
                .any(|character| character.is_control() || "/\\:*?\"<>|".contains(character))
        {
            return Err(FrontendError::invalid("invalid hc session name"));
        }
        if let Some(socket) = value("HC_SOCKET") {
            return Self::checked(absolute(socket.into()));
        }
        let home = |key: &str, suffix: &str| -> Result<PathBuf, FrontendError> {
            let path = match value(key) {
                Some(value) => PathBuf::from(value),
                None => PathBuf::from(
                    value("HOME")
                        .ok_or_else(|| FrontendError::invalid("hc home is unavailable"))?,
                )
                .join(suffix),
            };
            Ok(absolute(path))
        };
        let config = home("XDG_CONFIG_HOME", ".config")?.join("herdr-core/config.toml");
        #[derive(Deserialize)]
        struct Settings {
            socket: Option<String>,
        }
        match std::fs::read_to_string(config) {
            Ok(text) => {
                let settings: Settings = toml::from_str(&text)
                    .map_err(|_| FrontendError::invalid("hc socket configuration is invalid"))?;
                if let Some(socket) = settings.socket.filter(|socket| !socket.is_empty()) {
                    return Self::checked(absolute(socket.into()));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(FrontendError::invalid(
                    "hc socket configuration is unavailable",
                ));
            }
        }
        let runtime = value("XDG_RUNTIME_DIR")
            .map(|path| absolute(path.into()))
            .or_else(|| {
                let path = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }));
                path.is_dir().then_some(path)
            });
        let directory = match runtime {
            Some(runtime) => runtime.join("herdr-core"),
            None => home("XDG_STATE_HOME", ".local/state")?
                .join("herdr-core")
                .join(session),
        };
        Self::checked(directory.join(format!("{session}.sock")))
    }
    fn checked(socket: PathBuf) -> Result<Self, FrontendError> {
        let bytes = socket.as_os_str().as_bytes();
        let holder = socket.with_extension("holder.sock");
        let max = unsafe { std::mem::zeroed::<libc::sockaddr_un>() }
            .sun_path
            .len()
            - 1;
        if !socket.is_absolute()
            || bytes.contains(&0)
            || bytes.len().max(holder.as_os_str().as_bytes().len() + 9) > max
        {
            return Err(FrontendError::invalid("invalid or overlong hc socket path"));
        }
        Ok(Self { socket })
    }
    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

#[derive(Debug, thiserror::Error)]
#[error("{error}")]
pub struct HcFailure {
    pub rejection: Option<String>,
    pub error: FrontendError,
}

pub struct HcClient {
    socket: HcEndpoint,
    endpoint: EndpointId,
    methods: BTreeSet<String>,
    reader: BufReader<UnixStream>,
    sequence: u64,
    failed: bool,
    deadline: Instant,
    cancellation: Option<Cancellation>,
}
impl HcClient {
    pub fn connect(
        socket: HcEndpoint,
        expected: Option<&EndpointId>,
        timeout: f64,
        cancellation: Option<Cancellation>,
    ) -> Result<Self, FrontendError> {
        let deadline = crate::process::deadline(timeout, "hc")
            .map_err(|error| FrontendError::invalid(error.message))?;
        let stream = socket_io::connect(socket.socket(), deadline, cancellation.as_ref(), "hc")
            .map_err(socket_error)?;
        let mut client = Self {
            socket,
            endpoint: EndpointId(String::new()),
            methods: BTreeSet::new(),
            reader: BufReader::new(stream),
            sequence: 0,
            failed: false,
            deadline,
            cancellation,
        };
        #[derive(Deserialize)]
        struct Core {
            id: String,
        }
        #[derive(Deserialize)]
        struct Hello {
            generation: u32,
            framed: bool,
            core: Core,
            methods: BTreeSet<String>,
        }
        let hello: Hello = serde_json::from_value(
            client.request("hello", &json!({"generation":1,"framed":false}))?,
        )
        .map_err(|_| FrontendError::invalid("invalid hc hello reply"))?;
        crate::model::check_text(&hello.core.id, "hc endpoint", false)
            .map_err(|_| FrontendError::invalid("invalid hc endpoint"))?;
        if hello.generation != 1 || hello.framed {
            return Err(FrontendError::invalid("unsupported hc protocol"));
        }
        client.endpoint = EndpointId(hello.core.id);
        if expected.is_some_and(|expected| expected != &client.endpoint) {
            return Err(FrontendError::invalid(
                "hc endpoint does not match the requested core",
            ));
        }
        client.methods = hello.methods;
        Ok(client)
    }
    pub fn endpoint(&self) -> &EndpointId {
        &self.endpoint
    }
    pub fn socket(&self) -> &HcEndpoint {
        &self.socket
    }
    pub fn supports(&self, method: &str) -> bool {
        self.methods.contains(method)
    }
    pub fn request(&mut self, method: &str, params: &Value) -> Result<Value, FrontendError> {
        self.exchange(method, params, false)
            .1
            .map_err(|failure| failure.error)
    }
    pub fn exchange(
        &mut self,
        method: &str,
        params: &Value,
        effect: bool,
    ) -> (Vec<u8>, Result<Value, HcFailure>) {
        let mut frame = Vec::new();
        let mut submitted = false;
        let mut rejection = None;
        // A well-formed, correlated error reply leaves the stream in step, so the operation
        // stays usable; every other failure ends it.
        let mut rejected = false;
        let result = (|| {
            if self.failed {
                return Err(FrontendError::invalid("hc operation has ended"));
            }
            if method != "hello" && !self.supports(method) {
                return Err(FrontendError::invalid("hc method is unavailable"));
            }
            if let Some(cancellation) = &self.cancellation {
                cancellation.check().map_err(socket_error)?;
            }
            self.sequence += 1;
            let id = self.sequence;
            let mut bytes = serde_json::to_vec(
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .map_err(|_| FrontendError::invalid("invalid hc request"))?;
            bytes.push(b'\n');
            if bytes.len() > MAX_FRAME {
                return Err(FrontendError::invalid("hc request exceeds 4 MiB"));
            }
            let stream = self.reader.get_mut();
            stream
                .set_write_timeout(Some(
                    socket_io::remaining(self.deadline, "hc").map_err(socket_error)?,
                ))
                .map_err(socket_error)?;
            submitted = true;
            stream.write_all(&bytes).map_err(socket_error)?;
            socket_io::read_frame_into(
                &mut self.reader,
                Some(self.deadline),
                &mut frame,
                self.cancellation.as_ref(),
                "hc",
                MAX_FRAME,
            )
            .map_err(socket_error)?;
            let text = std::str::from_utf8(&frame)
                .map_err(|_| FrontendError::invalid("invalid hc reply"))?;
            let reply = crate::codec::load_json(text)
                .map_err(|_| FrontendError::invalid("invalid hc reply"))?;
            if reply.get("jsonrpc") != Some(&json!("2.0")) || reply.get("id") != Some(&json!(id)) {
                return Err(FrontendError::invalid(
                    "hc reply does not match the request",
                ));
            }
            match (reply.get("result"), reply.get("error")) {
                (Some(value), None) => Ok(value.clone()),
                (None, Some(error)) => {
                    if error.get("code").and_then(Value::as_i64).is_none()
                        || error.get("message").and_then(Value::as_str).is_none()
                    {
                        return Err(FrontendError::invalid("invalid hc error envelope"));
                    }
                    rejection = error
                        .get("data")
                        .and_then(|data| data.get("kind"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    rejected = true;
                    Err(FrontendError::invalid("hc request failed"))
                }
                _ => Err(FrontendError::invalid("invalid hc result envelope")),
            }
        })();
        if result.is_err() && !rejected {
            self.failed = true;
        }
        let result = result.map_err(|mut error| {
            if submitted {
                error.commands.push(CommandEvidence {
                    request: NativeRequest::Socket {
                        method: method.into(),
                        params: params.clone(),
                        response: frame.clone(),
                    },
                    outcome: if effect {
                        CommandOutcome::Uncertain
                    } else {
                        CommandOutcome::ReadOnly
                    },
                });
            }
            HcFailure { error, rejection }
        });
        (frame, result)
    }
}
fn socket_error(error: impl std::fmt::Display) -> FrontendError {
    FrontendError::invalid(format!("hc socket: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::Read,
        os::{fd::AsRawFd, unix::net::UnixListener},
        thread,
    };
    fn serve(
        script: impl FnOnce(BufReader<UnixStream>) + Send + 'static,
    ) -> (tempfile::TempDir, HcEndpoint, thread::JoinHandle<()>) {
        let directory = tempfile::Builder::new()
            .prefix("hc-wire-")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = HcEndpoint::checked(directory.path().join("s")).unwrap();
        let listener = UnixListener::bind(socket.socket()).unwrap();
        let task = thread::spawn(move || script(BufReader::new(listener.accept().unwrap().0)));
        (directory, socket, task)
    }
    fn read(reader: &mut BufReader<UnixStream>) -> Value {
        let mut frame = Vec::new();
        socket_io::read_frame_into(
            reader,
            Some(Instant::now() + std::time::Duration::from_secs(2)),
            &mut frame,
            None,
            "hc",
            MAX_FRAME,
        )
        .unwrap();
        serde_json::from_slice(&frame).unwrap()
    }
    fn hello() -> Value {
        json!({"generation":1,"framed":false,"core":{"id":"core","name":"Display only"},"methods":["hello","terminal.list","terminal.run","doc.get","doc.set"],"features":[],"future_field":true})
    }
    fn reply(reader: &mut BufReader<UnixStream>, request: &Value, result: Value) {
        writeln!(
            reader.get_mut(),
            "{}",
            json!({"jsonrpc":"2.0","id":request["id"],"result":result})
        )
        .unwrap();
    }
    #[test]
    fn configured_socket_precedence_and_invalid_configuration_have_no_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut environment = BTreeMap::from([
            ("HOME".into(), root.display().to_string()),
            (
                "XDG_RUNTIME_DIR".into(),
                root.join("run").display().to_string(),
            ),
            ("HC_SESSION".into(), " suite ".into()),
        ]);
        assert_eq!(
            HcEndpoint::resolve(&environment, root).unwrap().socket(),
            root.join("run/herdr-core/suite.sock")
        );
        let config = root.join(".config/herdr-core/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!(
                "socket = '{}'\n[future]\nunknown = true\n",
                root.join("configured.sock").display()
            ),
        )
        .unwrap();
        assert_eq!(
            HcEndpoint::resolve(&environment, root).unwrap().socket(),
            root.join("configured.sock")
        );
        std::fs::write(&config, "socket = 42").unwrap();
        assert!(HcEndpoint::resolve(&environment, root).is_err());
        environment.insert(
            "HC_SOCKET".into(),
            root.join("explicit.sock").display().to_string(),
        );
        assert_eq!(
            HcEndpoint::resolve(&environment, root).unwrap().socket(),
            root.join("explicit.sock")
        );
        environment.insert("HC_SOCKET".into(), "relative.sock".into());
        assert_eq!(
            HcEndpoint::resolve(&environment, root).unwrap().socket(),
            root.join("relative.sock")
        );
        environment.insert("HC_SESSION".into(), "../other".into());
        assert!(HcEndpoint::resolve(&environment, root).is_err());
    }
    #[test]
    fn relative_hc_selectors_use_only_the_captured_directory_and_check_holder_length() {
        let directory = tempfile::Builder::new()
            .prefix("hc-")
            .tempdir_in("/tmp")
            .unwrap();
        let root = directory.path();
        let config = root.join("config/herdr-core/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        let mut environment = BTreeMap::from([
            ("HOME".into(), "home".into()),
            ("XDG_CONFIG_HOME".into(), "config".into()),
            ("XDG_RUNTIME_DIR".into(), "runtime".into()),
            ("HC_SESSION".into(), " café core ".into()),
        ]);
        std::fs::write(&config, "socket = 'configured.sock'").unwrap();
        assert_eq!(
            HcEndpoint::resolve(&environment, root).unwrap().socket(),
            root.join("configured.sock")
        );
        std::fs::remove_file(&config).unwrap();
        assert_eq!(
            HcEndpoint::resolve(&environment, root).unwrap().socket(),
            root.join("runtime/herdr-core/café core.sock")
        );
        environment.insert("HC_SOCKET".into(), "./explicit.sock".into());
        assert_eq!(
            HcEndpoint::resolve(&environment, root).unwrap().socket(),
            root.join("./explicit.sock")
        );
        assert!(HcEndpoint::resolve(&environment, Path::new("relative")).is_err());
        environment.insert("HC_SOCKET".into(), format!("/{}", "x".repeat(100)));
        assert!(HcEndpoint::resolve(&environment, root).is_err());
        environment.insert("HC_SOCKET".into(), "bad\0socket".into());
        assert!(HcEndpoint::resolve(&environment, root).is_err());
        for session in [".", "..", "bad/name", "bad:name", "bad\tsession"] {
            environment.insert("HC_SESSION".into(), session.into());
            assert!(HcEndpoint::resolve(&environment, root).is_err());
        }
    }
    #[test]
    fn hello_pins_one_connection_and_preserves_request_parameters() {
        let (_directory, socket, task) = serve(|mut reader| {
            let request = read(&mut reader);
            assert_eq!(
                request,
                json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"generation":1,"framed":false}})
            );
            reply(&mut reader, &request, hello());
            let request = read(&mut reader);
            assert_eq!(
                request,
                json!({"jsonrpc":"2.0","id":2,"method":"terminal.list","params":{"all":true}})
            );
            reply(&mut reader, &request, json!({"terminals":[]}));
            let request = read(&mut reader);
            assert_eq!(
                request,
                json!({"jsonrpc":"2.0","id":3,"method":"doc.get","params":{"name":"streams"}})
            );
            reply(&mut reader, &request, json!({"version":0,"value":null}));
        });
        let mut client =
            HcClient::connect(socket, Some(&EndpointId("core".into())), 2.0, None).unwrap();
        assert_eq!(client.endpoint(), &EndpointId("core".into()));
        let flags = unsafe { libc::fcntl(client.reader.get_ref().as_raw_fd(), libc::F_GETFD) };
        assert!(flags >= 0);
        assert_ne!(flags & libc::FD_CLOEXEC, 0);
        assert_eq!(
            client
                .request("terminal.list", &json!({"all":true}))
                .unwrap(),
            json!({"terminals":[]})
        );
        assert_eq!(
            client
                .request("doc.get", &json!({"name":"streams"}))
                .unwrap(),
            json!({"version":0,"value":null})
        );
        task.join().unwrap();
    }
    #[test]
    fn endpoint_mismatch_stops_after_hello_without_effects() {
        let (_directory, socket, task) = serve(|mut reader| {
            let request = read(&mut reader);
            reply(&mut reader, &request, hello());
            let mut remaining = Vec::new();
            reader.read_to_end(&mut remaining).unwrap();
            assert!(remaining.is_empty());
        });
        assert!(HcClient::connect(socket, Some(&EndpointId("other".into())), 2.0, None).is_err());
        task.join().unwrap();
    }
    #[test]
    fn lost_reply_retains_partial_bytes_and_ends_the_operation_without_replay() {
        let (_directory, socket, task) = serve(|mut reader| {
            let request = read(&mut reader);
            reply(&mut reader, &request, hello());
            assert_eq!(read(&mut reader)["method"], "terminal.run");
            reader.get_mut().write_all(b"{").unwrap();
        });
        let mut client = HcClient::connect(socket, None, 2.0, None).unwrap();
        let (frame, result) = client.exchange("terminal.run", &json!({"argv":["/bin/true"]}), true);
        assert_eq!(frame, b"{");
        assert!(result.is_err());
        assert_eq!(
            client
                .request("terminal.run", &json!({}))
                .unwrap_err()
                .message,
            "hc operation has ended"
        );
        task.join().unwrap();
    }
    #[test]
    fn malformed_protocol_and_result_envelopes_fail_before_other_requests() {
        for change in 0..5 {
            let (_directory, socket, task) = serve(move |mut reader| {
                let request = read(&mut reader);
                let mut value = hello();
                match change {
                    0 => value["generation"] = json!(2),
                    1 => value["framed"] = json!(true),
                    2 => value["core"]["id"] = json!(""),
                    _ => {}
                }
                let mut response = json!({"jsonrpc":"2.0","id":request["id"],"result":value});
                if change == 3 {
                    response["id"] = json!(99);
                }
                if change == 4 {
                    response["error"] = json!({"code":-1});
                }
                writeln!(reader.get_mut(), "{response}").unwrap();
            });
            assert!(HcClient::connect(socket, None, 2.0, None).is_err());
            task.join().unwrap();
        }
    }
    #[test]
    fn correlated_rejections_retain_uncertain_evidence_and_keep_the_operation() {
        let (_directory, socket, task) = serve(|mut reader| {
            let request = read(&mut reader);
            reply(&mut reader, &request, hello());
            let request = read(&mut reader);
            writeln!(reader.get_mut(),"{}",json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"owner detail is not the error message","data":{"kind":"conflict"}}})).unwrap();
            let request = read(&mut reader);
            reply(&mut reader, &request, json!({"version":3,"value":null}));
        });
        let mut client = HcClient::connect(socket, None, 2.0, None).unwrap();
        let params = json!({"name":"streams","version":3,"value":null});
        let (frame, result) = client.exchange("doc.set", &params, true);
        let failure = result.unwrap_err();
        assert_eq!(failure.rejection.as_deref(), Some("conflict"));
        assert_eq!(failure.error.message, "hc request failed");
        assert_eq!(
            failure.error.commands,
            vec![CommandEvidence {
                request: NativeRequest::Socket {
                    method: "doc.set".into(),
                    params,
                    response: frame
                },
                outcome: CommandOutcome::Uncertain
            }]
        );
        assert_eq!(
            client
                .request("doc.get", &json!({"name":"streams"}))
                .unwrap(),
            json!({"version":3,"value":null}),
            "a rejection leaves the operation usable"
        );
        task.join().unwrap();
    }
    #[test]
    fn cancellation_interrupts_a_blocked_hello_without_waiting_for_the_peer() {
        use std::{sync::mpsc, time::Duration};
        let (started, ready) = mpsc::channel();
        let (release, held) = mpsc::channel();
        let (_directory, socket, server) = serve(move |mut reader| {
            read(&mut reader);
            started.send(()).unwrap();
            held.recv().unwrap();
        });
        let cancellation = Cancellation::new().unwrap();
        let cancel = cancellation.clone();
        let (finished, result) = mpsc::channel();
        let client = thread::spawn(move || {
            finished
                .send(HcClient::connect(socket, None, 2.0, Some(cancellation)).is_err())
                .unwrap()
        });
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.cancel().unwrap();
        let outcome = result.recv_timeout(Duration::from_secs(1));
        release.send(()).unwrap();
        server.join().unwrap();
        client.join().unwrap();
        assert!(outcome.unwrap());
    }
}
