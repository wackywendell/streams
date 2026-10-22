//! Protocol 1 transports concurrent CLI calls in-process. Stdout is exclusively frames.

use std::{
    collections::BTreeSet,
    io::{BufRead, Write},
    sync::{Arc, Mutex},
    thread,
};

use clap::Parser;
use serde::{Deserialize, Serialize};

use super::{
    AgentCommand, CliError, Command, Input, NativeCommand, Output, PathBuf, Result, Value,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(try_from = "u64", into = "u64")]
struct RequestId(u64);
impl TryFrom<u64> for RequestId {
    type Error = &'static str;

    fn try_from(value: u64) -> std::result::Result<Self, Self::Error> {
        if (1..=9_007_199_254_740_991).contains(&value) {
            Ok(Self(value))
        } else {
            Err("request id must be a positive JavaScript safe integer")
        }
    }
}
impl From<RequestId> for u64 {
    fn from(value: RequestId) -> Self {
        value.0
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum Incoming {
    Request {
        id: RequestId,
        argv: Vec<String>,
        #[serde(default, deserialize_with = "present_input")]
        input: Option<Value>,
    },
    Cancel {
        id: RequestId,
    },
}

// JSON null is stdin text "null", not absent input (empty stdin).
fn present_input<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Value>, D::Error> {
    Value::deserialize(deserializer).map(Some)
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum Outgoing<'a> {
    Ready {
        protocol: u8,
    },
    Response {
        id: RequestId,
        code: u8,
        value: Value,
        notices: Vec<String>,
    },
    Event {
        #[serde(flatten)]
        event: Event<'a>,
    },
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum Event<'a> {
    Scope {
        #[serde(flatten)]
        scope: &'a Scope,
    },
    Warning {
        message: &'a str,
    },
}

#[derive(PartialEq, Serialize)]
struct Scope {
    value: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}
impl From<&Result<Output>> for Scope {
    fn from(outcome: &Result<Output>) -> Self {
        match outcome {
            Ok(output) if output.code == 0 && !output.value.is_null() => Self {
                value: output.value.clone(),
                reason: None,
            },
            Ok(output) => Self {
                value: Value::Null,
                reason: Some(
                    output.value["error"]["message"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            if output.notices.is_empty() {
                                "No owner observation".into()
                            } else {
                                output.notices.join("\n")
                            }
                        }),
                ),
            },
            Err(error) => Self {
                value: Value::Null,
                reason: Some(error.to_string()),
            },
        }
    }
}

struct Server {
    session_id: String,
    plugin_root: PathBuf,
    output: Mutex<std::io::Stdout>,
    previous_scope: Mutex<Option<Scope>>,
}
impl Server {
    fn send(&self, frame: &Outgoing<'_>) -> Result<()> {
        let mut bytes = serde_json::to_vec(frame)?;
        bytes.push(b'\n');
        let mut output = self
            .output
            .lock()
            .map_err(|error| CliError::message(error.to_string()))?;
        output.write_all(&bytes)?;
        output.flush()?;
        Ok(())
    }

    // Subscription setup can report its failure through this hook before exiting.
    fn warning(&self, message: &str) -> Result<()> {
        self.send(&Outgoing::Event {
            event: Event::Warning { message },
        })
    }

    // Future native/file events use this same edge-triggered observation path.
    // Serialize observations, but never hold the output lock during native I/O.
    fn recompute_scope(&self) -> Result<()> {
        let mut previous = self
            .previous_scope
            .lock()
            .map_err(|error| CliError::message(error.to_string()))?;
        let outcome = super::execute(
            Command::Native {
                session_id: Some(self.session_id.clone()),
                plugin_root: Some(self.plugin_root.clone()),
                state_dir: None,
                expect: None,
                command: NativeCommand::Observe,
            },
            Input::request(None),
            true,
        );
        self.emit_scope(&mut previous, Scope::from(&outcome))
    }

    fn emit_scope(&self, previous: &mut Option<Scope>, scope: Scope) -> Result<()> {
        if previous.as_ref() != Some(&scope) {
            self.send(&Outgoing::Event {
                event: Event::Scope { scope: &scope },
            })?;
            *previous = Some(scope);
        }
        Ok(())
    }

    fn finish_worker(&self, worker: thread::JoinHandle<Result<()>>) -> Result<()> {
        worker.join().unwrap_or_else(|_| {
            let message = "Streams request worker panicked";
            let _ = self.warning(message);
            Err(CliError::message(message))
        })
    }

    fn prepare(&self, command: &mut Command) -> Result<()> {
        match command {
            Command::Agent { .. }
            | Command::List { .. }
            | Command::Sync { .. }
            | Command::Workspace { .. } => Ok(()),
            Command::Native {
                session_id,
                plugin_root,
                command:
                    NativeCommand::Observe
                    | NativeCommand::Open { .. }
                    | NativeCommand::Close
                    | NativeCommand::Project { .. }
                    | NativeCommand::Capture,
                ..
            } => {
                if session_id.as_ref().is_some_and(|id| id != &self.session_id) {
                    return Err(CliError::SessionMismatch);
                }
                *session_id = Some(self.session_id.clone());
                if plugin_root.is_none() {
                    *plugin_root = Some(self.plugin_root.clone());
                }
                Ok(())
            }
            _ => Err(CliError::UnsupportedCommand(
                "Command is not supported by the child".into(),
            )),
        }
    }

    fn request(&self, id: RequestId, argv: Vec<String>, input: Option<Value>) -> Result<()> {
        let parsed = allow_command(&argv).and_then(|()| {
            super::args::Cli::try_parse_from(std::iter::once("streams".to_owned()).chain(argv))
                .map_err(|error| CliError::message(error.to_string()))
        });
        let command = parsed.and_then(|mut parsed| {
            self.prepare(&mut parsed.command)?;
            Ok(parsed.command)
        });
        let command = match command {
            Ok(command) => command,
            Err(error) => return self.respond(id, Err(error)),
        };
        let native = matches!(command, Command::Native { .. });
        let save = matches!(
            command,
            Command::Agent {
                command: AgentCommand::Enroll | AgentCommand::Apply
            }
        );
        let unfenced_observe = matches!(
            &command,
            Command::Native {
                command: NativeCommand::Observe,
                expect: None,
                ..
            }
        );
        // Serialize reused observations with refreshes, without locking stdout
        // during I/O. An override observes another scope and cannot be reused.
        let previous = if matches!(
            &command,
            Command::Native {
                command: NativeCommand::Observe,
                plugin_root: Some(root),
                state_dir: None,
                ..
            } if root == &self.plugin_root
        ) {
            Some(
                self.previous_scope
                    .lock()
                    .map_err(|error| CliError::message(error.to_string()))?,
            )
        } else {
            None
        };
        let outcome = super::execute(command, Input::request(input), true);
        let observation = previous.as_ref().and_then(|_| {
            (unfenced_observe || outcome.as_ref().is_ok_and(|output| output.code == 0))
                .then(|| Scope::from(&outcome))
        });
        let refresh_scope = native || (save && outcome.is_ok());
        self.respond(id, outcome)?;
        if let Some(mut previous) = previous {
            if let Some(scope) = observation {
                return self.emit_scope(&mut previous, scope);
            }
            drop(previous);
        }
        if refresh_scope {
            // Failed observations may have a stale --expect: read current scope.
            self.recompute_scope()?;
        }
        Ok(())
    }

    fn respond(&self, id: RequestId, outcome: Result<Output>) -> Result<()> {
        let output = outcome.unwrap_or_else(|error| {
            let mut output = Output::value(error.data());
            output.code = 1;
            output
        });
        self.send(&Outgoing::Response {
            id,
            code: output.code,
            value: output.value,
            notices: output.notices,
        })
    }
}

// Check command tokens before clap: forbidden commands must not leak usage
// errors (or successful help exits) from the full one-shot CLI grammar.
fn allow_command(argv: &[String]) -> Result<()> {
    let mut tokens = argv
        .iter()
        .map(String::as_str)
        .filter(|arg| *arg != "--json");
    let supported = !argv
        .iter()
        .any(|arg| matches!(arg.as_str(), "--help" | "-h"))
        && match tokens.next() {
            Some("list" | "sync") => true,
            Some("agent") => matches!(
                tokens.next(),
                Some("context" | "capture-inputs" | "preview" | "enroll" | "apply")
            ),
            Some("workspace") => matches!(tokens.next(), Some("capture")),
            Some("native") => loop {
                match tokens.next() {
                    Some("--session-id" | "--plugin-root" | "--state-dir" | "--expect") => {
                        tokens.next();
                    }
                    Some(arg)
                        if [
                            "--session-id=",
                            "--plugin-root=",
                            "--state-dir=",
                            "--expect=",
                        ]
                        .iter()
                        .any(|prefix| arg.starts_with(prefix)) => {}
                    command => {
                        break matches!(
                            command,
                            Some("observe" | "open" | "close" | "project" | "capture")
                        );
                    }
                }
            },
            _ => false,
        };
    if supported {
        Ok(())
    } else {
        Err(CliError::UnsupportedCommand(
            "Command is not supported by the child".into(),
        ))
    }
}

pub(super) fn run(session_id: String, plugin_root: PathBuf) -> Result<()> {
    let server = Arc::new(Server {
        session_id,
        plugin_root,
        output: Mutex::new(std::io::stdout()),
        previous_scope: Mutex::new(None),
    });
    server.send(&Outgoing::Ready { protocol: 1 })?;
    server.recompute_scope()?;
    let mut input = std::io::stdin().lock();
    let mut seen = BTreeSet::new();
    let mut frame = Vec::new();
    let mut workers = Vec::new();
    loop {
        frame.clear();
        if input.read_until(b'\n', &mut frame)? == 0 {
            break;
        }
        if !frame.ends_with(b"\n") {
            return Err(CliError::message("Unterminated protocol frame"));
        }
        // Value alone would silently overwrite duplicate keys, including in input.
        let value = streams_temper::codec::load_json(
            std::str::from_utf8(&frame).map_err(|error| CliError::message(error.to_string()))?,
        )?;
        let message = serde_json::from_value::<Incoming>(value)?;
        // Reap on input events, never on a timer; retain only in-flight handles.
        while let Some(index) = workers.iter().position(thread::JoinHandle::is_finished) {
            server.finish_worker(workers.swap_remove(index))?;
        }
        match message {
            Incoming::Request { id, argv, input } => {
                if !seen.insert(id) {
                    return Err(CliError::message("Duplicate request id"));
                }
                let server = Arc::clone(&server);
                workers.push(thread::spawn(move || server.request(id, argv, input)));
            }
            // Advisory: no operation is interrupted or its response suppressed.
            Incoming::Cancel { id } => {
                let _ = id;
            }
        }
    }
    // Only clean EOF drains work. Malformed input returns above and crashes the child.
    let mut result = Ok(());
    for worker in workers {
        let completed = server.finish_worker(worker);
        if result.is_ok() {
            result = completed;
        }
    }
    result
}
