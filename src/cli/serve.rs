//! Typed transport with one FIFO for native work; read-only catalog work is independent.
mod control;
mod dispatch;
mod files;
mod hc;
mod native;

use super::{App, CliError, Output, PathBuf, Result};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{BufRead, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
use streams::{
    child_protocol::{
        self, ClientFrame, Outcome, PiRequestContext, Request, RequestError, RequestId,
        ServerFrame, Version2, Warnings,
    },
    frontend::herdr::events::SubscriptionUpdate,
};

type SharedOutput = Arc<Mutex<Option<std::io::Stdout>>>;

enum Wake {
    Request(RequestId, Request, Option<Box<PiRequestContext>>),
    Finished,
    InputFinished,
    Failed(String),
    Files(u8, notify::Result<notify::Event>),
    Native(std::result::Result<SubscriptionUpdate, String>),
    Control(RequestId, Request, std::os::unix::net::UnixStream),
}

enum NativeWork {
    Request(RequestId, Request, Option<Box<PiRequestContext>>),
    Retire(Instant, mpsc::SyncSender<Result<()>>),
    Control(RequestId, Request, std::os::unix::net::UnixStream),
    Files(u8, notify::Result<notify::Event>),
    Native(std::result::Result<SubscriptionUpdate, String>),
}

const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

struct NativeControllers {
    herdr: Option<native::Controller>,
    hc: HcSource,
}

enum HcSource {
    Unattempted,
    Registered(hc::Source),
    Failed,
}
impl HcSource {
    fn request(
        &mut self,
        app: &App,
        session_id: &str,
        sdk_pid: Option<u32>,
        request: &Request,
        stopping: &streams::cancellation::Cancellation,
        cancelled: &AtomicBool,
    ) -> Result<Output> {
        if matches!(request, Request::OpenCard {} | Request::CloseCard {}) {
            return Err(CliError::message("hc native display is not available yet"));
        }
        if matches!(self, Self::Unattempted) {
            *self = Self::Failed;
            let pid = sdk_pid
                .filter(|pid| *pid != 0)
                .ok_or_else(|| CliError::message("Pi SDK process is unavailable"))?;
            *self = Self::Registered(hc::Source::register(
                app,
                session_id,
                pid,
                stopping.clone(),
                cancelled,
            )?);
        }
        if matches!(self, Self::Failed) {
            return Err(CliError::message(
                "hc Pi source is unavailable; reload required",
            ));
        }
        Ok(Output::value(serde_json::json!({"backend":"hc","report":{
            "native":{"kind":"inactive","reason":"unavailable"},
            "projection":{"companion":null,"desired_pi_name":null,"warnings":["hc native display and lifecycle are not yet available"]}
        }})))
    }
    fn retire(&mut self, deadline: Instant) -> Result<()> {
        match std::mem::replace(self, Self::Failed) {
            Self::Registered(source) => source.retire(deadline),
            _ => Ok(()),
        }
    }
}

struct Server {
    session_id: String,
    sdk_pid: Option<u32>,
    output: SharedOutput,
    app: Mutex<Option<App>>,
    active: Mutex<BTreeMap<u64, Arc<AtomicBool>>>,
    sender: mpsc::Sender<Wake>,
}

impl Server {
    fn app(&self) -> Result<App> {
        let mut app = self
            .app
            .lock()
            .map_err(|error| CliError::message(error.to_string()))?;
        if app.is_none() {
            *app = Some(App::new()?);
        }
        Ok(app.as_ref().expect("application initialized").clone())
    }

    fn remember_app(&self, app: Option<&App>) -> Result<()> {
        if let Some(app) = app {
            *self
                .app
                .lock()
                .map_err(|error| CliError::message(error.to_string()))? = Some(app.clone());
        }
        Ok(())
    }

    fn cancelled(&self, id: RequestId) -> Result<Arc<AtomicBool>> {
        Ok(self
            .active
            .lock()
            .map_err(|error| CliError::message(error.to_string()))?
            .get(&id.get())
            .expect("admitted request")
            .clone())
    }

    fn outcome(result: Result<Output>) -> Result<Outcome> {
        Ok(match result {
            Err(error) => Outcome::Error {
                error: RequestError {
                    kind: error.data()["error"]["kind"]
                        .as_str()
                        .expect("error kind")
                        .into(),
                    message: error.to_string(),
                },
            },
            Ok(mut output) => {
                let result = output.value.as_object().cloned().ok_or_else(|| {
                    CliError::message("application operation returned a non-object result")
                })?;
                if output.accepted_save && (output.code != 0 || !output.notices.is_empty()) {
                    if output.notices.is_empty() {
                        output.notices.push("local save accepted; completion needs attention; inspect the saved result before retrying".into());
                    }
                    Outcome::SavedWithWarnings {
                        result,
                        warnings: Warnings::new(output.notices)?,
                    }
                } else {
                    Outcome::Success { result }
                }
            }
        })
    }
    fn respond(&self, id: RequestId, result: Result<Output>) -> Result<()> {
        let outcome = Self::outcome(result)?;
        send(&self.output, &ServerFrame::Response { id, outcome })?;
        self.active
            .lock()
            .map_err(|error| CliError::message(error.to_string()))?
            .remove(&id.get());
        Ok(())
    }

    fn warning(&self, error: &CliError) -> Result<()> {
        send(
            &self.output,
            &ServerFrame::Warning {
                message: error.to_string(),
            },
        )
    }

    fn catalog(&self, request: Request, cancelled: &AtomicBool) -> Result<Output> {
        if cancelled.load(Ordering::Acquire) {
            return Err(CliError::message("request cancelled before dispatch"));
        }
        // Replay has no application/config/catalog/native capability, even in a poisoned environment.
        if let Request::Preview {
            catalog,
            action,
            request,
        } = request
        {
            return dispatch::preview(catalog, action, request);
        }
        dispatch::execute(&self.app()?, &self.session_id, request)
    }

    fn native_request(
        &self,
        controllers: &mut NativeControllers,
        id: RequestId,
        request: Request,
        pi_context: Option<&PiRequestContext>,
        stopping: &streams::cancellation::Cancellation,
        plugin_root: &std::path::Path,
    ) -> Result<()> {
        let cancelled = self.cancelled(id)?;
        let outcome = (|| {
            if cancelled.load(Ordering::Acquire) {
                return Err(CliError::message("request cancelled before dispatch"));
            }
            let app = self.app()?;
            if matches!(
                request,
                Request::SessionStarted {}
                    | Request::Attach {}
                    | Request::OpenCard {}
                    | Request::CloseCard {}
            ) && app.hc_selected(&app.config.load()?.frontend, None)?
            {
                if pi_context.is_none() {
                    return Err(CliError::message("hc Pi request context is unavailable"));
                }
                return controllers.hc.request(
                    &app,
                    &self.session_id,
                    self.sdk_pid,
                    &request,
                    stopping,
                    &cancelled,
                );
            }
            let controller = &mut controllers.herdr;
            match request {
                Request::SessionStarted {}
                | Request::Attach {}
                | Request::OpenCard {}
                | Request::CloseCard {} => {
                    let controller = controller.get_or_insert_with(|| {
                        native::Controller::new(
                            app.clone(),
                            self.session_id.clone(),
                            plugin_root.into(),
                            self.sender.clone(),
                            self.output.clone(),
                            stopping.clone(),
                        )
                    });
                    controller.request(&request, cancelled)
                }
                Request::Enroll { request } => {
                    let value = serde_json::Value::Object(request);
                    match controller {
                        Some(controller) => controller.enroll(&app, &value, cancelled),
                        None => app.enroll_request(&value.to_string()),
                    }
                }
                request => {
                    let saved = matches!(request, Request::Apply { .. });
                    let app = controller
                        .as_ref()
                        .map(|controller| &controller.app)
                        .unwrap_or(&app);
                    let mut output = dispatch::execute(app, &self.session_id, request)?;
                    if saved && (output.code == 0 || output.accepted_save) {
                        output.accepted_save = true;
                        if let Some(controller) = controller
                            && let Err(error) = controller.saved(cancelled)
                        {
                            output.notices.push(error.to_string());
                        }
                    }
                    Ok(output)
                }
            }
        })();
        self.remember_app(controllers.herdr.as_ref().map(|controller| &controller.app))?;
        self.respond(id, outcome)
    }

    fn cancel_all(&self) {
        if let Ok(active) = self.active.lock() {
            for flag in active.values() {
                flag.store(true, Ordering::Release);
            }
        }
    }
}

fn native_queue(request: &Request) -> bool {
    matches!(
        request,
        Request::SessionStarted {}
            | Request::Attach {}
            | Request::OpenCard {}
            | Request::CloseCard {}
            | Request::Apply { .. }
            | Request::Enroll { .. }
            | Request::SetCardMode { .. }
    )
}

fn send(output: &SharedOutput, frame: &ServerFrame) -> Result<()> {
    let mut bytes = serde_json::to_vec(frame)?;
    bytes.push(b'\n');
    let mut output = output
        .lock()
        .map_err(|error| CliError::message(error.to_string()))?;
    if let Some(stdout) = output.as_mut() {
        stdout.write_all(&bytes)?;
        stdout.flush()?;
    }
    Ok(())
}

fn read_input(server: Arc<Server>) {
    let result = (|| -> Result<()> {
        let mut seen = BTreeSet::new();
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            if input.read_line(&mut line)? == 0 {
                break;
            }
            if !line.ends_with('\n') {
                return Err(CliError::message("unterminated protocol frame"));
            }
            let frame = child_protocol::decode_client(&line)?;
            match frame {
                ClientFrame::Cancel { id } => {
                    if let Some(flag) = server
                        .active
                        .lock()
                        .map_err(|error| CliError::message(error.to_string()))?
                        .get(&id.get())
                    {
                        flag.store(true, Ordering::Release);
                    }
                }
                ClientFrame::Request {
                    id,
                    request,
                    pi_context,
                } => {
                    if !seen.insert(id.get()) {
                        return Err(CliError::message("request ID reused"));
                    }
                    server
                        .active
                        .lock()
                        .map_err(|error| CliError::message(error.to_string()))?
                        .insert(id.get(), Arc::new(AtomicBool::new(false)));
                    if let Some(context) = &pi_context
                        && (server.sdk_pid.is_none_or(|pid| pid == 0)
                            || context.conversation.harness != "pi"
                            || context.conversation.native_id != server.session_id)
                    {
                        server.respond(
                            id,
                            Err(CliError::message(
                                "Pi request context does not match this helper",
                            )),
                        )?;
                        continue;
                    }
                    if native_queue(&request) {
                        server
                            .sender
                            .send(Wake::Request(id, request, pi_context))
                            .map_err(|error| CliError::message(error.to_string()))?;
                    } else {
                        let worker = Arc::clone(&server);
                        thread::spawn(move || {
                            let completed =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    let flag = worker.cancelled(id)?;
                                    let result = worker.catalog(request, &flag);
                                    worker.respond(id, result)
                                }));
                            let wake = match completed {
                                Ok(Ok(())) => Wake::Finished,
                                Ok(Err(error)) => Wake::Failed(error.to_string()),
                                Err(_) => Wake::Failed("Streams catalog worker panicked".into()),
                            };
                            let _ = worker.sender.send(wake);
                        });
                    }
                }
            }
        }
        Ok(())
    })();
    let wake = match result {
        Ok(()) => Wake::InputFinished,
        Err(error) => Wake::Failed(error.to_string()),
    };
    let _ = server.sender.send(wake);
}

fn native_worker(
    server: Arc<Server>,
    receiver: mpsc::Receiver<NativeWork>,
    stopping: streams::cancellation::Cancellation,
    plugin_root: PathBuf,
) -> Result<()> {
    let mut controllers = NativeControllers {
        herdr: None,
        hc: HcSource::Unattempted,
    };
    let flag = stopping.flag();
    for work in &receiver {
        if flag.load(Ordering::Acquire) {
            break;
        }
        match work {
            NativeWork::Request(id, request, pi_context) => server.native_request(
                &mut controllers,
                id,
                request,
                pi_context.as_deref(),
                &stopping,
                &plugin_root,
            )?,
            NativeWork::Retire(deadline, acknowledgement) => {
                let result = controllers.hc.retire(deadline);
                let _ = acknowledgement.send(result);
                break;
            }
            NativeWork::Control(id, request, mut stream) => {
                let result = server
                    .app()
                    .and_then(|app| dispatch::execute(&app, &server.session_id, request));
                let outcome = Server::outcome(result)?;
                // A departed caller does not undo an accepted preference write.
                let _ = writeln!(
                    stream,
                    "{}",
                    serde_json::to_string(&ServerFrame::Response { id, outcome })?
                );
            }
            NativeWork::Native(update) => {
                if let Some(controller) = &mut controllers.herdr {
                    if let Err(error) = controller.update(update, flag.clone()) {
                        server.warning(&error)?;
                    }
                    server.remember_app(Some(&controller.app))?;
                }
            }
            NativeWork::Files(generation, event) => {
                if let Some(controller) = &mut controllers.herdr {
                    if let Err(error) = controller.file_event(generation, event, flag.clone()) {
                        server.warning(&error)?;
                    }
                    server.remember_app(Some(&controller.app))?;
                }
            }
        }
        let _ = server.sender.send(Wake::Finished);
    }
    Ok(())
}

pub(super) fn run(session_id: String, sdk_pid: Option<u32>, plugin_root: PathBuf) -> Result<()> {
    let output = Arc::new(Mutex::new(Some(std::io::stdout())));
    send(&output, &ServerFrame::Ready { protocol: Version2 })?;
    let (sender, receiver) = mpsc::channel();
    let server = Arc::new(Server {
        session_id,
        sdk_pid,
        output,
        app: Mutex::new(None),
        active: Mutex::new(BTreeMap::new()),
        sender,
    });
    let (native_sender, native_receiver) = mpsc::channel();
    let (done, completed) = mpsc::sync_channel(1);
    let stopping = streams::cancellation::Cancellation::new()?;
    let native = {
        let server = server.clone();
        let stopping = stopping.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                native_worker(server.clone(), native_receiver, stopping, plugin_root)
            }))
            .unwrap_or_else(|_| Err(CliError::message("Streams native worker panicked")));
            if let Err(error) = &result {
                let _ = server.sender.send(Wake::Failed(error.to_string()));
            }
            let _ = done.send(result);
        })
    };
    let input = server.clone();
    thread::spawn(move || read_input(input));
    let mut deadline: Option<Instant> = None;
    let result = (|| -> Result<()> {
        loop {
            if deadline.is_some()
                && server
                    .active
                    .lock()
                    .map_err(|error| CliError::message(error.to_string()))?
                    .is_empty()
            {
                break;
            }
            let wake = match deadline {
                Some(deadline) => receiver
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| {
                        CliError::message(
                            "child shutdown timed out; unfinished operations remain uncertain",
                        )
                    })?,
                None => receiver
                    .recv()
                    .map_err(|error| CliError::message(error.to_string()))?,
            };
            let work = match wake {
                Wake::Request(id, request, pi_context) => {
                    Some(NativeWork::Request(id, request, pi_context))
                }
                Wake::Native(update) => Some(NativeWork::Native(update)),
                Wake::Files(generation, event) => Some(NativeWork::Files(generation, event)),
                Wake::Control(id, request, stream) => {
                    Some(NativeWork::Control(id, request, stream))
                }
                Wake::Finished => None,
                Wake::InputFinished => {
                    deadline = Some(Instant::now() + SHUTDOWN_GRACE);
                    None
                }
                Wake::Failed(error) => return Err(CliError::message(error)),
            };
            if let Some(work) = work {
                native_sender
                    .send(work)
                    .map_err(|error| CliError::message(error.to_string()))?;
            }
        }
        Ok(())
    })();
    server.cancel_all();
    let deadline = deadline.unwrap_or_else(|| Instant::now() + SHUTDOWN_GRACE);
    let (acknowledge, retired) = mpsc::sync_channel(1);
    let retirement = match native_sender.send(NativeWork::Retire(deadline, acknowledge)) {
        Ok(()) => retired
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|_| Err(CliError::message("hc Pi source retirement timed out"))),
        Err(_) => Err(CliError::message(
            "native worker stopped before source retirement",
        )),
    };
    stopping.cancel()?;
    if let Ok(mut output) = server.output.lock() {
        *output = None;
    }
    drop(native_sender);
    drop(receiver);
    let joined = completed.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    match joined {
        Ok(native_result) => {
            let _ = native.join();
            result.and(retirement).and(native_result)
        }
        Err(_) => result.and(Err(CliError::message(
            "native worker did not stop promptly",
        ))),
    }
}
