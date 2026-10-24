//! Protocol 1 transports concurrent CLI calls in-process. Stdout is exclusively frames.
mod files;
mod scope;

use scope::{Owner, Sample};
use streams_temper::{
    frontend::herdr::{
        HerdrClient, HerdrNativeSnapshot,
        events::{HerdrEvent, SubscriptionUpdate},
    },
    runtime::native::native_inventory_from_snapshot,
};

use std::{
    collections::BTreeSet,
    io::{BufRead, Write},
    sync::{Arc, Mutex, mpsc},
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

type Worker = thread::JoinHandle<Result<()>>;

enum Wake {
    InputFinished(Result<Vec<Worker>>),
    Failed(String),
    Files(u8, notify::Result<notify::Event>),
    Native(
        std::result::Result<SubscriptionUpdate, String>,
        mpsc::SyncSender<Owner>,
    ),
}

enum Trigger<'a> {
    Refresh,
    Native(&'a HerdrEvent),
    NativeRecovery,
    FileRecovery,
}

struct Context {
    owner: Owner,
    files: files::Files,
}

struct Server {
    session_id: String,
    plugin_root: PathBuf,
    output: Arc<Mutex<std::io::Stdout>>,
    failures: mpsc::Sender<Wake>,
    previous_scope: Mutex<Option<Scope>>,
    app: Mutex<super::App>,
    context: Mutex<Context>,
}
impl Server {
    fn send(&self, frame: &Outgoing<'_>) -> Result<()> {
        send(&self.output, frame)
    }

    fn warning(&self, message: &str) -> Result<()> {
        self.send(&Outgoing::Event {
            event: Event::Warning { message },
        })
    }

    // Serialize observation I/O, never stdout. Each matching trigger spends one snapshot.
    fn recompute_scope(&self, trigger: Trigger<'_>) -> Result<Output> {
        let resubscribed = matches!(trigger, Trigger::NativeRecovery);
        let mut previous = self
            .previous_scope
            .lock()
            .map_err(|e| CliError::message(e.to_string()))?;
        let mut context = self
            .context
            .lock()
            .map_err(|e| CliError::message(e.to_string()))?;
        if let Some(reason) = &context.owner.refusal {
            let outcome = Err(CliError::NativeContextChanged(reason.clone()));
            self.emit_scope(&mut previous, Scope::from(&outcome))?;
            return Ok(response_output(outcome));
        }
        let target = match trigger {
            Trigger::Native(event) => match context.owner.event_target(event) {
                Some(target) => target,
                None => return Ok(Output::value(Value::Null)),
            },
            _ => context.owner.clone(),
        };
        let workspace = context.owner.workspace.clone();
        context.files.prepare(Some(&workspace))?;
        let mut app = self
            .app
            .lock()
            .map_err(|e| CliError::message(e.to_string()))?
            .clone();
        let input = (|| -> Result<_> {
            let read = app
                .herdr
                .as_ref()
                .ok_or_else(|| CliError::message("No Herdr owner context"))?;
            let snapshot = HerdrNativeSnapshot::parse(
                &read
                    .client
                    .session_snapshot()
                    .map_err(|e| CliError::message(e.to_string()))?,
            )
            .map_err(|e| CliError::message(e.to_string()))?;
            let inventory = native_inventory_from_snapshot(&read.client, snapshot.raw())?;
            let samples: Vec<_> = inventory
                .panes()
                .iter()
                .map(|pane| Sample {
                    pane: pane.pane_id().into(),
                    tab: pane.tab_id().into(),
                    workspace: pane.workspace_id().into(),
                    terminal: pane.terminal_id().into(),
                    session: pane.native_session_id().map(str::to_owned),
                })
                .collect();
            Ok((snapshot, samples))
        })();
        let input = match input {
            Err(error)
                if matches!(trigger, Trigger::NativeRecovery | Trigger::FileRecovery)
                    && app.herdr.is_some() =>
            {
                return Err(error);
            }
            input => input,
        };
        let prepared = (|| -> Result<super::App> {
            let (snapshot, samples) = match input {
                Err(error) if context.owner.terminal.is_none() && app.herdr.is_some() => {
                    let reason = error.to_string();
                    self.refuse_origin(&mut context, &reason)?;
                    return Err(CliError::NativeContextChanged(reason));
                }
                input => input?,
            };
            let read = app.herdr.as_ref().expect("validated native input");
            let sample = if resubscribed && let Some(terminal) = &context.owner.terminal {
                samples.iter().find(|sample| &sample.terminal == terminal)
            } else {
                samples.iter().find(|sample| sample.pane == target.pane)
            };
            let target = if resubscribed
                && context.owner.terminal.is_some()
                && let Some(sample) = sample
            {
                Owner {
                    pane: sample.pane.clone(),
                    tab: sample.tab.clone(),
                    workspace: sample.workspace.clone(),
                    terminal: context.owner.terminal.clone(),
                    refusal: None,
                }
            } else {
                target
            };
            let owner = match context.owner.confirm(&target, sample, &self.session_id) {
                Ok(owner) => owner,
                Err(reason) => {
                    self.refuse_origin(&mut context, &reason)?;
                    return Err(CliError::NativeContextChanged(reason));
                }
            };
            let mut environment = app.environment.clone();
            owner.apply(&mut environment);
            let client = read
                .client
                .with_context(environment)
                .map_err(|e| CliError::message(e.to_string()))?;
            // Bind through the existing strict caller check, not an inventory match.
            // The same check re-verifies the Pi identity after each continuity move.
            if let Err(error) = streams_temper::runtime::native::verify_pi_caller_from_snapshot(
                &client,
                snapshot.raw(),
                &self.session_id,
            ) {
                let reason = error.to_string();
                self.refuse_origin(&mut context, &reason)?;
                return Err(CliError::NativeContextChanged(reason));
            }
            owner.apply(&mut app.environment);
            app.herdr.as_mut().expect("pinned Herdr read").terminal = owner.terminal.clone();
            *self
                .app
                .lock()
                .map_err(|e| CliError::message(e.to_string()))? = app.clone();
            context.owner = owner;
            app.herdr.as_mut().expect("pinned Herdr read").snapshot = Some(snapshot);
            app.input = Input::request(None);
            Ok(app)
        })();
        let outcome = match prepared {
            Ok(app) => {
                if context.owner.workspace != workspace {
                    let workspace = context.owner.workspace.clone();
                    // Watch-registration failure is fatal, not a null native observation.
                    context.files.prepare(Some(&workspace))?;
                }
                app.execute(self.observe_command())
            }
            Err(error) => Err(error),
        };
        self.emit_scope(&mut previous, Scope::from(&outcome))?;
        Ok(response_output(outcome))
    }

    fn refuse_origin(&self, context: &mut Context, reason: &str) -> Result<()> {
        context.owner.refusal = Some(reason.into());
        let mut app = self
            .app
            .lock()
            .map_err(|e| CliError::message(e.to_string()))?;
        app.herdr.as_mut().expect("native caller refusal").refusal = Some(reason.into());
        Ok(())
    }

    fn observe_command(&self) -> Command {
        Command::Native {
            session_id: Some(self.session_id.clone()),
            plugin_root: Some(self.plugin_root.clone()),
            state_dir: None,
            expect: None,
            command: NativeCommand::Observe,
        }
    }

    fn execute(&self, command: Command, input: Input) -> Result<Output> {
        let mut app = self
            .app
            .lock()
            .map_err(|e| CliError::message(e.to_string()))?
            .clone();
        if matches!(command, Command::Native { .. }) && app.herdr.is_none() {
            return Err(CliError::message("No Herdr owner context"));
        }
        app.input = input;
        app.execute(command)
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
                    | NativeCommand::Project
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
        let standard_observe = matches!(
            &command,
            Command::Native {
                command: NativeCommand::Observe,
                plugin_root: Some(root),
                state_dir: None,
                ..
            } if root == &self.plugin_root
        );
        if standard_observe && unfenced_observe {
            // Default observations share owner confirmation, watches and refusal state.
            let output = self.recompute_scope(Trigger::Refresh)?;
            return self.respond(id, Ok(output));
        }
        // Serialize successful fenced observations with refreshes, never stdout.
        let previous = if standard_observe {
            Some(
                self.previous_scope
                    .lock()
                    .map_err(|error| CliError::message(error.to_string()))?,
            )
        } else {
            None
        };
        let outcome = self.execute(command, Input::request(input));
        let observation = previous.as_ref().and_then(|_| {
            outcome
                .as_ref()
                .is_ok_and(|output| output.code == 0)
                .then(|| Scope::from(&outcome))
        });
        let refresh_scope = native || (save && outcome.is_ok());
        if let Some(mut previous) = previous {
            if let Some(scope) = observation {
                self.emit_scope(&mut previous, scope)?;
                return self.respond(id, outcome);
            }
            drop(previous);
        }
        if refresh_scope {
            // Complete the caused scope change before acknowledging request completion.
            self.recompute_scope(Trigger::Refresh)?;
        }
        self.respond(id, outcome)
    }

    fn respond(&self, id: RequestId, outcome: Result<Output>) -> Result<()> {
        let output = response_output(outcome);
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

fn response_output(outcome: Result<Output>) -> Output {
    outcome.unwrap_or_else(|error| {
        let mut output = Output::value(error.data());
        output.code = 1;
        output
    })
}

fn send(output: &Mutex<std::io::Stdout>, frame: &Outgoing<'_>) -> Result<()> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    let mut output = output
        .lock()
        .map_err(|e| CliError::message(e.to_string()))?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

fn read_input(sender: mpsc::Sender<Wake>, server: Arc<Server>) {
    let mut input = std::io::stdin().lock();
    let mut frame = Vec::new();
    let mut workers = Vec::new();
    let mut seen = BTreeSet::new();
    let result = (|| -> Result<Vec<Worker>> {
        loop {
            frame.clear();
            let message = (|| -> Result<Option<Incoming>> {
                if input.read_until(b'\n', &mut frame)? == 0 {
                    return Ok(None);
                }
                if !frame.ends_with(b"\n") {
                    return Err(CliError::message("Unterminated protocol frame"));
                }
                // Reject duplicate object keys, including private request input.
                let value = streams_temper::codec::load_json(
                    std::str::from_utf8(&frame).map_err(|e| CliError::message(e.to_string()))?,
                )?;
                Ok(Some(serde_json::from_value(value)?))
            })();
            // Intake is independent of unsolicited observation I/O on the main lane.
            while let Some(index) = workers.iter().position(thread::JoinHandle::is_finished) {
                server.finish_worker(workers.swap_remove(index))?;
            }
            match message? {
                None => return Ok(workers),
                Some(Incoming::Request { id, argv, input }) => {
                    if !seen.insert(id) {
                        return Err(CliError::message("Duplicate request id"));
                    }
                    let server = Arc::clone(&server);
                    workers.push(thread::spawn(move || {
                        let result = server.request(id, argv, input);
                        if let Err(error) = &result {
                            let _ = server.warning(&error.to_string());
                            let _ = server.failures.send(Wake::Failed(error.to_string()));
                        }
                        result
                    }));
                }
                Some(Incoming::Cancel { id }) => {
                    let _ = id;
                }
            }
        }
    })();
    let _ = sender.send(Wake::InputFinished(result));
}

pub(super) fn run(session_id: String, plugin_root: PathBuf) -> Result<()> {
    let output = Arc::new(Mutex::new(std::io::stdout()));
    send(&output, &Outgoing::Ready { protocol: 1 })?;
    let (sender, receiver) = mpsc::channel();
    let setup = (|| -> Result<_> {
        let mut app = super::App::new()?;
        let owner = Owner::inherited(&app.environment);
        let subscription = if app.environment.get("HERDR_ENV").map(String::as_str) == Some("1") {
            let config = streams_temper::runtime::native_cards::NativeCardsConfig::load(
                &app.environment,
                &app.cwd,
                &plugin_root,
            )?;
            let client = HerdrClient::connect(app.environment.clone(), config.timeout_seconds())
                .map_err(|e| CliError::message(e.to_string()))?;
            let subscription = client
                .socket()
                .subscribe(owner.filter())
                .map_err(|e| CliError::message(e.to_string()))?;
            app.herdr = Some(super::HerdrRead {
                client,
                snapshot: None,
                terminal: None,
                refusal: None,
            });
            Some(subscription)
        } else {
            None
        };
        let mut files = files::Files::new(&app, &session_id, sender.clone())?;
        files.discover(&app, &session_id)?;
        Ok((app, Context { owner, files }, subscription))
    })();
    let (app, context, subscription) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            send(
                &output,
                &Outgoing::Event {
                    event: Event::Warning {
                        message: &error.to_string(),
                    },
                },
            )?;
            return Err(error);
        }
    };
    let server = Arc::new(Server {
        session_id,
        plugin_root,
        output,
        failures: sender.clone(),
        previous_scope: Mutex::new(None),
        app: Mutex::new(app),
        context: Mutex::new(context),
    });
    if let Err(error) = server.recompute_scope(Trigger::Refresh) {
        server.warning(&error.to_string())?;
        return Err(error);
    }
    let cancellation = subscription
        .as_ref()
        .map(|subscription| subscription.cancellation_handle());
    let native = subscription.map(|mut subscription| {
        let sender = sender.clone();
        let filter = server.context.lock().expect("serve context").owner.filter();
        subscription.set_watched_ids(filter.pane_ids, filter.tab_ids, filter.workspace_ids);
        thread::spawn(move || {
            loop {
                let update = subscription.receive().map_err(|e| e.to_string());
                let failed = update.is_err();
                let (reply, owner) = mpsc::sync_channel(0);
                if sender.send(Wake::Native(update, reply)).is_err() || failed {
                    break;
                }
                let Ok(owner) = owner.recv() else {
                    break;
                };
                let filter = owner.filter();
                subscription.set_watched_ids(filter.pane_ids, filter.tab_ids, filter.workspace_ids);
            }
        })
    });
    let input_server = Arc::clone(&server);
    thread::spawn(move || read_input(sender, input_server));
    let mut workers = Vec::new();
    let mut subscription_error = false;
    let result = (|| -> Result<()> {
        for wake in &receiver {
            match wake {
                Wake::Failed(message) => {
                    subscription_error = false;
                    return Err(CliError::message(message));
                }
                Wake::InputFinished(completed) => {
                    workers = completed?;
                    break;
                }
                Wake::Native(update, reply) => {
                    subscription_error = true;
                    match update.map_err(CliError::message)? {
                        SubscriptionUpdate::Event(event) => {
                            server.recompute_scope(Trigger::Native(&event))?;
                        }
                        SubscriptionUpdate::Resubscribed => {
                            server.recompute_scope(Trigger::NativeRecovery)?;
                        }
                    }
                    let owner = server
                        .context
                        .lock()
                        .map_err(|e| CliError::message(e.to_string()))?
                        .owner
                        .clone();
                    let _ = reply.send(owner);
                    subscription_error = false;
                }
                Wake::Files(generation, event) => {
                    subscription_error = true;
                    let mut context = server
                        .context
                        .lock()
                        .map_err(|e| CliError::message(e.to_string()))?;
                    if !context.files.current(generation) {
                        subscription_error = false;
                        continue;
                    }
                    let recovered = event.as_ref().map_or(true, |event| event.need_rescan());
                    let relevant = recovered
                        || event
                            .as_ref()
                            .is_ok_and(|event| context.files.relevant(event));
                    if !relevant {
                        subscription_error = false;
                        continue;
                    }
                    if recovered {
                        context.files.recover()?;
                    }
                    let workspace = context.owner.workspace.clone();
                    let changed = context.files.prepare(Some(&workspace))?;
                    drop(context);
                    if recovered || changed {
                        server.recompute_scope(if recovered {
                            Trigger::FileRecovery
                        } else {
                            Trigger::Refresh
                        })?;
                    }
                    subscription_error = false;
                }
            }
        }
        Ok(())
    })();
    // Drop queued reply handles before joining a subscriber waiting for routing acknowledgement.
    drop(receiver);
    if let Some(cancellation) = cancellation {
        let _ = cancellation.cancel();
    }
    if let Some(native) = native {
        let _ = native.join();
    }
    if let Err(error) = result {
        if subscription_error {
            server.warning(&error.to_string())?;
        }
        return Err(error);
    }
    // Only clean EOF drains requests. Their responses fence their own native work.
    let mut result = Ok(());
    for worker in workers {
        let completed = server.finish_worker(worker);
        if result.is_ok() {
            result = completed;
        }
    }
    result
}
