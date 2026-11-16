//! Terminal observations are never effect authority; the host owns fresh native proofs.
use std::{
    io::{self, IsTerminal, Read, Stdout, Write},
    os::unix::net::UnixStream,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    task::{Context, Poll, Wake, Waker},
    thread::{self, JoinHandle},
    time::Duration,
};

use super::{CardSnapshot, CardView, PaintedFrame, Rect, RefreshToken, Target};
use crate::cancellation::Cancellation;
use futures_core::Stream;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    crossterm::{
        cursor::{Hide, Show},
        event::{
            self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste,
            EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton,
            MouseEventKind,
        },
        execute,
        terminal::{
            self, DisableLineWrap, EnableLineWrap, EnterAlternateScreen, LeaveAlternateScreen,
        },
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    Full,
    Changed,
}

/// All methods are bounded, including construction and cancellation cleanup.
/// `read` returns fresh record context for the verified card lifetime.
/// `activate` independently revalidates fresh proofs before effects; UI frames
/// and their captured snapshots are observations, never effect authority.
/// The host checks its factory-supplied cancellation flag between bounded effects.
/// Dropping a host must not close the native card or perform new native effects.
/// A synchronous Exit/Failed event precedes this method's completion on the same
/// FIFO response channel, so terminal outcomes cannot become recoverable frames.
pub trait PaneHost {
    fn read(&mut self, _refresh: Refresh) -> Result<CardSnapshot, String>;
    fn activate(&mut self, snapshot: &CardSnapshot, target: &Target) -> Result<(), String>;
}

pub enum PaneEvent {
    Changed,
    Exit,
    Failed(String),
}
#[derive(Clone)]
pub struct PaneEvents(Arc<dyn Fn(PaneEvent) + Send + Sync>);
impl PaneEvents {
    pub fn new(send: impl Fn(PaneEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }
    pub fn send(&self, event: PaneEvent) {
        (self.0)(event);
    }
}

enum Request {
    Read(RefreshToken, Refresh),
    Activate(Box<CardSnapshot>, Target),
}
enum Response {
    Input,
    Signal,
    Stopped,
    Event(PaneEvent),
    Ready(Result<(), String>),
    Read(RefreshToken, Result<Box<CardSnapshot>, String>),
    Activated(Result<(), String>),
}

struct WorkerDone(Sender<Response>);
impl Drop for WorkerDone {
    fn drop(&mut self) {
        let _ = self.0.send(Response::Stopped);
    }
}
struct InputWake(Sender<Response>);
impl Wake for InputWake {
    fn wake(self: Arc<Self>) {
        let _ = self.0.send(Response::Input);
    }
}

struct Worker {
    events: Sender<Response>,
    requests: Option<Sender<Request>>,
    responses: Receiver<Response>,
    cancel: Cancellation,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    fn start<H, F>(make_host: F) -> Result<Self, String>
    where
        H: PaneHost + 'static,
        F: FnOnce(Cancellation, PaneEvents) -> Result<H, String> + Send + 'static,
    {
        let cancel = Cancellation::new().map_err(|e| e.to_string())?;
        let token = cancel.clone();
        let flag = cancel.flag();
        let (tx, requests) = mpsc::channel();
        let (responses, rx) = mpsc::channel();
        let events = responses.clone();
        let thread = thread::Builder::new()
            .name("card-native-io".into())
            .spawn(move || {
                let _done = WorkerDone(responses.clone());
                // H need not be Send: its entire lifetime is on this thread.
                let events = PaneEvents::new({
                    let responses = responses.clone();
                    move |event| {
                        let _ = responses.send(Response::Event(event));
                    }
                });
                let mut host = match make_host(token, events) {
                    Ok(host) => host,
                    Err(error) => {
                        let _ = responses.send(Response::Ready(Err(error)));
                        return;
                    }
                };
                if responses.send(Response::Ready(Ok(()))).is_err() {
                    return;
                }
                while let Ok(request) = requests.recv() {
                    if flag.load(Ordering::SeqCst) {
                        break;
                    }
                    let response = match request {
                        Request::Read(token, refresh) => {
                            Response::Read(token, host.read(refresh).map(Box::new))
                        }
                        Request::Activate(snapshot, target) => {
                            Response::Activated(host.activate(&snapshot, &target))
                        }
                    };
                    if responses.send(response).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            events,
            requests: Some(tx),
            responses: rx,
            cancel,
            thread: Some(thread),
        })
    }
    fn send(&self, request: Request) -> Result<(), String> {
        self.requests
            .as_ref()
            .ok_or("Native worker stopped")?
            .send(request)
            .map_err(|_| "Native worker disconnected".into())
    }
    fn finish(&mut self) -> Result<(), String> {
        self.cancel.cancel().map_err(|e| e.to_string())?;
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| {
                "Native worker panicked; native card retained or uncertain".to_string()
            })?;
        }
        Ok(())
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Busy {
    Starting,
    Idle,
    Read(RefreshToken),
    Activate,
}
struct Controller {
    #[cfg(test)]
    wakeups: Arc<std::sync::atomic::AtomicUsize>,
    view: CardView,
    snapshot: Option<CardSnapshot>,
    painted: Option<PaintedFrame>,
    busy: Busy,
    pending_read: Option<Refresh>,
    dirty: bool,
}
impl Controller {
    fn new() -> Self {
        Self {
            #[cfg(test)]
            wakeups: Arc::default(),
            view: CardView::new(),
            snapshot: None,
            painted: None,
            busy: Busy::Starting,
            pending_read: None,
            dirty: true,
        }
    }
    fn queue_read(&mut self, refresh: Refresh) {
        if refresh == Refresh::Full || self.pending_read.is_none() {
            self.pending_read = Some(refresh);
        }
    }
    fn next_read(&mut self) -> Option<Request> {
        if self.busy != Busy::Idle {
            return None;
        }
        let refresh = self.pending_read.take()?;
        let token = self.view.begin_quiet_refresh(); // Invalidate BEFORE dispatch, not on completion.
        self.busy = Busy::Read(token);
        self.dirty = true;
        Some(Request::Read(token, refresh))
    }
    #[cfg(test)]
    fn refresh(&mut self) -> Option<Request> {
        self.refresh_for(Refresh::Full)
    }
    #[cfg(test)]
    fn refresh_for(&mut self, refresh: Refresh) -> Option<Request> {
        self.queue_read(refresh);
        self.next_read()
    }
    #[cfg(test)]
    fn complete(&mut self, response: Response) -> Result<Option<Request>, String> {
        self.settle(response)?;
        Ok(self.next_read())
    }
    // Apply the sole operation's result even when quitting, without dispatching more I/O.
    fn settle(&mut self, response: Response) -> Result<(), String> {
        match response {
            Response::Ready(result) if self.busy == Busy::Starting => {
                result?;
                self.busy = Busy::Idle;
                self.queue_read(Refresh::Full);
            }
            Response::Read(token, result) if self.busy == Busy::Read(token) => {
                self.view
                    .complete_refresh(token, result.map(|snapshot| *snapshot));
                self.snapshot = self.view.current_snapshot().cloned();
                self.busy = Busy::Idle;
            }
            Response::Activated(result) if self.busy == Busy::Activate => {
                self.view
                    .set_notice(result.err().map(|e| format!("Activation failed: {e}")));
                self.busy = Busy::Idle;
                self.queue_read(Refresh::Full);
            }
            _ => return Ok(()),
        }
        self.dirty = true;
        Ok(())
    }
    fn resize(&mut self) {
        self.view.geometry_changed();
        self.painted = None;
        self.dirty = true;
    }
    fn paint(&mut self, area: Rect) -> &PaintedFrame {
        self.painted = Some(self.view.paint(area).clone());
        self.dirty = false;
        self.painted.as_ref().unwrap()
    }
    // Activations dispatch immediately; coalesced read intent is pulled by next_read.
    fn input(&mut self, event: Event) -> Option<Request> {
        if matches!(&event, Event::Key(k) if k.kind == KeyEventKind::Press
            && k.code == KeyCode::Char('r') && k.modifiers == KeyModifiers::CONTROL)
        {
            self.view.set_notice(None);
            self.painted = None;
            self.dirty = true;
            self.queue_read(Refresh::Full);
            return None;
        }
        if self.busy != Busy::Idle || self.pending_read.is_some() {
            return None;
        }
        let painted = self.painted.as_ref()?;
        let action = match event {
            Event::Key(key) if key.kind == KeyEventKind::Press && key.modifiers.is_empty() => {
                match key.code {
                    KeyCode::Char(c) if c.is_ascii_lowercase() => painted.documents.get(&c),
                    _ => None,
                }
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => painted
                .links
                .iter()
                .find(|link| link.area.contains((mouse.column, mouse.row).into()))
                .map(|link| &link.action),
            _ => None,
        }?;
        if !self.view.is_current(action) {
            return None;
        }
        let request = Request::Activate(
            Box::new(self.snapshot.as_ref()?.clone()),
            action.target().clone(),
        );
        self.busy = Busy::Activate;
        self.view.geometry_changed();
        self.painted = None;
        self.dirty = true;
        Some(request)
    }
}
fn quit(event: &Event) -> bool {
    matches!(event, Event::Key(k) if k.kind == KeyEventKind::Press &&
        (k.code == KeyCode::Char('q') && k.modifiers.is_empty()
        || k.code == KeyCode::Char('c') && k.modifiers == KeyModifiers::CONTROL))
}

struct Signals {
    raised: Arc<AtomicBool>,
    ids: Vec<signal_hook::SigId>,
    stream: Option<UnixStream>,
    thread: Option<JoinHandle<()>>,
}
impl Signals {
    fn install() -> Result<Self, String> {
        let mut signals = Self {
            raised: Arc::new(AtomicBool::new(false)),
            ids: Vec::new(),
            stream: None,
            thread: None,
        };
        for signal in [
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGHUP,
        ] {
            signals.ids.push(
                signal_hook::flag::register(signal, signals.raised.clone())
                    .map_err(|e| e.to_string())?,
            );
        }
        Ok(signals)
    }
    fn attach(&mut self, events: Sender<Response>) -> Result<(), String> {
        let (mut read, write) = UnixStream::pair().map_err(|e| e.to_string())?;
        for signal in [
            signal_hook::consts::SIGTERM,
            signal_hook::consts::SIGINT,
            signal_hook::consts::SIGHUP,
        ] {
            self.ids.push(
                signal_hook::low_level::pipe::register(
                    signal,
                    write.try_clone().map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?,
            );
        }
        self.stream = Some(write);
        self.thread = Some(
            thread::Builder::new()
                .name("card-signals".into())
                .spawn(move || {
                    loop {
                        match read.read(&mut [0; 32]) {
                            Ok(0) => break,
                            Ok(_) => {
                                if events.send(Response::Signal).is_err() {
                                    break;
                                }
                            }
                            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                })
                .map_err(|e| e.to_string())?,
        );
        Ok(())
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
        }
        if let Some(stream) = self.stream.take() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// Xterm private-mode save/restore preserves prior modes on supporting terminals.
// Explicit baseline cleanup also works on terminals that ignore private-mode saves.
const SAVE_MODES: &[u8] = b"\x1b[?7;25;1000;1002;1003;1006;1015;1049;2004s";
const RESTORE_MODES: &[u8] = b"\x1b[?7;25;1000;1002;1003;1006;1015;1049;2004r";
struct TerminalState {
    prior_raw: bool,
    active: bool,
}
impl TerminalState {
    fn enter() -> io::Result<Self> {
        let state = Self {
            prior_raw: terminal::is_raw_mode_enabled()?,
            active: true,
        };
        // The guard exists before the first state-changing operation, including partial setup.
        io::stdout().write_all(SAVE_MODES)?;
        if !state.prior_raw {
            terminal::enable_raw_mode()?;
        }
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste,
            DisableLineWrap,
            Hide
        )?;
        Ok(state)
    }
    fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let mut out = io::stdout();
        let commands = execute!(
            out,
            DisableMouseCapture,
            DisableBracketedPaste,
            EnableLineWrap,
            Show,
            LeaveAlternateScreen
        );
        // Try every restoration even if stdout has failed. Leave active on failure for Drop retry.
        let modes = out.write_all(RESTORE_MODES).and_then(|_| out.flush());
        let raw = if self.prior_raw {
            Ok(())
        } else {
            terminal::disable_raw_mode()
        };
        let result = commands.and(modes).and(raw);
        if result.is_ok() {
            self.active = false;
        }
        result
    }
}
impl Drop for TerminalState {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    Quit,
    Other,
}

/// Requires interactive stdin/stdout. The single worker is joined on every exit,
/// including unwind; bounded native calls are the host's responsibility.
/// Return explicit quit intent separately from owner loss, signals and failures.
pub fn run<H, F>(make_host: F) -> Result<ExitReason, String>
where
    H: PaneHost + 'static,
    F: FnOnce(Cancellation, PaneEvents) -> Result<H, String> + Send + 'static,
{
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("Card requires interactive stdin and stdout".into());
    }
    let mut signals = Signals::install()?;
    let mut state = TerminalState::enter().map_err(|e| e.to_string())?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    // Validate terminal input before even constructing the native host.
    event::poll(Duration::ZERO).map_err(|e| e.to_string())?;
    if signals.raised.load(Ordering::SeqCst) {
        return Ok(ExitReason::Other);
    }
    let mut worker = Worker::start(make_host)?;
    let mut controller = Controller::new();
    let outcome = signals
        .attach(worker.events.clone())
        .and_then(|_| interact(&mut terminal, &mut controller, &worker, &signals));
    // Ratatui's Drop may show the cursor; run it BEFORE restoring saved modes.
    drop(terminal);
    finish_interaction(&mut worker, outcome, || state.restore())
}

fn finish_interaction<T>(
    worker: &mut Worker,
    outcome: Result<T, String>,
    restore: impl FnOnce() -> io::Result<()>,
) -> Result<T, String> {
    // Restore on every exit before joining the cancelled worker and its subscriptions.
    let restored = restore().map_err(|e| format!("Terminal restore failed: {e}"));
    let joined = worker.finish();
    restored?;
    let result = outcome?;
    joined?;
    Ok(result)
}

fn interact(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    controller: &mut Controller,
    worker: &Worker,
    signals: &Signals,
) -> Result<ExitReason, String> {
    let waker = Waker::from(Arc::new(InputWake(worker.events.clone())));
    let mut context = Context::from_waker(&waker);
    let mut input_events = event::EventStream::new();
    let mut received = None;
    loop {
        #[cfg(test)]
        controller.wakeups.fetch_add(1, Ordering::SeqCst);
        if signals.raised.load(Ordering::SeqCst) {
            return Ok(ExitReason::Other);
        }
        if controller.dirty {
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    frame.buffer_mut().merge(&controller.paint(area).buffer);
                })
                .map_err(|e| e.to_string())?;
        }
        // Drain bounded batches BEFORE accepting completions. Discard activations
        // accumulated during I/O; coalesced refresh intent is not a captured action.
        let mut drained = false;
        for _ in 0..64 {
            let input = match Pin::new(&mut input_events).poll_next(&mut context) {
                Poll::Ready(Some(input)) => input.map_err(|e| e.to_string())?,
                Poll::Ready(None) => return Err("Terminal event stream stopped".into()),
                Poll::Pending => {
                    drained = true;
                    break;
                }
            };
            if signals.raised.load(Ordering::SeqCst) {
                return Ok(ExitReason::Other);
            }
            if quit(&input) {
                return Ok(ExitReason::Quit);
            }
            if matches!(input, Event::Resize(..)) {
                controller.resize();
            } else if let Some(request) = controller.input(input) {
                worker.send(request)?;
            }
        }
        if !drained {
            continue;
        }
        // Coalesce every queued invalidation before scheduling one read.
        loop {
            match received
                .take()
                .map(Ok)
                .unwrap_or_else(|| worker.responses.try_recv())
            {
                Ok(Response::Input) => {}
                Ok(Response::Signal) => return Ok(ExitReason::Other),
                Ok(Response::Stopped) => {
                    return Err(
                        "Native worker disconnected; native card retained or uncertain".into(),
                    );
                }
                Ok(Response::Event(PaneEvent::Changed)) => {
                    controller.queue_read(Refresh::Changed);
                }
                Ok(Response::Event(PaneEvent::Exit)) => return Ok(ExitReason::Other),
                Ok(Response::Event(PaneEvent::Failed(error))) => return Err(error),
                Ok(response) => controller.settle(response)?,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err(
                        "Native worker disconnected; native card retained or uncertain".into(),
                    );
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if let Some(request) = controller.next_read() {
            worker.send(request)?;
        }
        if controller.dirty {
            continue;
        }
        received = Some(
            worker
                .responses
                .recv()
                .map_err(|_| "Card event channel disconnected")?,
        );
    }
}

#[cfg(test)]
#[path = "application/idle_tests.rs"]
mod idle_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{
        CheckoutBinding, CheckoutSelection, DisplayContext, DocumentLocation,
        NativeOwnerObservation, ResolvedDocument,
    };
    use ratatui::crossterm::event::KeyEvent;

    pub(super) fn snapshot() -> CardSnapshot {
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("fixtures/active-renderer.json")).unwrap();
        let source = &fixtures[0]["snapshot"];
        let selection = CheckoutSelection {
            checkout_id: "lib".into(),
            root: "/code/library".into(),
        };
        CardSnapshot::new(
            serde_json::from_value(source["stream"].clone()).unwrap(),
            serde_json::from_value(source["session"].clone()).unwrap(),
            DisplayContext {
                catalog_source: "catalog".into(),
                owner: NativeOwnerObservation {
                    pane_id: "w:p1".into(),
                    tab_id: "w:t1".into(),
                    workspace_id: "w".into(),
                },
                checkouts: vec![CheckoutBinding {
                    repository_url: "https://example.com/lib".into(),
                    selection: selection.clone(),
                }],
                documents: vec![DocumentLocation {
                    repository_url: "https://example.com/lib".into(),
                    path: "docs/design.md".into(),
                    location: ResolvedDocument::File {
                        selection,
                        file: "/code/library/docs/design.md".into(),
                    },
                }],
            },
        )
        .unwrap()
    }
    fn key(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
    }
    fn ready() -> Controller {
        let mut c = Controller::new();
        let Some(Request::Read(token, _)) = c.complete(Response::Ready(Ok(()))).unwrap() else {
            panic!()
        };
        c.complete(Response::Read(token, Ok(Box::new(snapshot()))))
            .unwrap();
        c.paint(Rect::new(0, 0, 100, 20));
        c
    }
    #[test]
    fn invalidation_and_explicit_refresh_causes_reach_the_worker_without_guessing() {
        let mut c = ready();
        let Some(Request::Read(token, cause)) = c.refresh_for(Refresh::Changed) else {
            panic!()
        };
        assert_eq!(cause, Refresh::Changed);
        c.complete(Response::Read(token, Ok(Box::new(snapshot()))))
            .unwrap();
        let Some(Request::Read(_, cause)) = c.refresh() else {
            panic!()
        };
        assert_eq!(cause, Refresh::Full);
    }
    #[test]
    fn explicit_refresh_during_a_read_is_coalesced_without_replaying_actions() {
        for cause in [Refresh::Changed, Refresh::Full] {
            let mut c = ready();
            let Some(Request::Read(token, _)) = c.refresh_for(cause) else {
                panic!()
            };
            for _ in 0..2 {
                assert!(
                    c.input(Event::Key(KeyEvent::new(
                        KeyCode::Char('r'),
                        KeyModifiers::CONTROL
                    )))
                    .is_none()
                );
            }
            assert!(c.input(key('a')).is_none());
            assert!(c.refresh_for(Refresh::Changed).is_none());
            let Some(Request::Read(token, Refresh::Full)) = c
                .complete(Response::Read(token, Ok(Box::new(snapshot()))))
                .unwrap()
            else {
                panic!(
                    "Ctrl-R during a read must retain one full refresh, not replay a document action"
                );
            };
            assert!(
                c.complete(Response::Read(token, Ok(Box::new(snapshot()))))
                    .unwrap()
                    .is_none()
            );
            assert!(c.busy == Busy::Idle);
        }
    }
    #[test]
    fn manual_refresh_repaints_a_cleared_notice_while_read_remains_busy() {
        let mut c = ready();
        c.view.set_notice(Some("Activation failed: earlier".into()));
        assert!(matches!(
            c.refresh_for(Refresh::Changed),
            Some(Request::Read(_, _))
        ));
        c.paint(Rect::new(0, 0, 100, 20));
        assert!(!c.dirty);
        assert!(
            c.input(Event::Key(KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL
            )))
            .is_none()
        );
        assert!(c.dirty);
        assert!(c.painted.is_none());
        let frame = c.paint(Rect::new(0, 0, 100, 20));
        let text = (0..20)
            .map(|y| super::super::render::buffer_line(&frame.buffer, y))
            .collect::<String>();
        assert!(!text.contains("Activation failed"));
        assert!(matches!(c.busy, Busy::Read(_)));
    }
    #[test]
    fn queued_refresh_blocks_painted_actions_until_a_fresh_read() {
        let mut c = ready();
        assert!(
            c.input(Event::Key(KeyEvent::new(
                KeyCode::Char('r'),
                KeyModifiers::CONTROL
            )))
            .is_none()
        );
        assert!(c.input(key('a')).is_none());
        // A later invalidation cannot downgrade explicit refresh intent.
        c.queue_read(Refresh::Changed);
        let Some(Request::Read(token, Refresh::Full)) = c.next_read() else {
            panic!()
        };
        assert!(
            c.complete(Response::Read(token, Ok(Box::new(snapshot()))))
                .unwrap()
                .is_none()
        );
        c.paint(Rect::new(0, 0, 100, 20));
        assert!(matches!(c.input(key('a')), Some(Request::Activate(_, _))));
    }
    #[test]
    fn refresh_invalidates_and_stale_completion_is_ignored() {
        let mut c = ready();
        let action = c.painted.as_ref().unwrap().documents[&'a'].clone();
        let Some(Request::Read(old, _)) = c.refresh() else {
            panic!()
        };
        assert!(!c.view.is_current(&action));
        assert!(c.input(key('a')).is_none());
        assert!(c.refresh().is_none());
        let Some(Request::Read(new, _)) = c
            .complete(Response::Read(old, Ok(Box::new(snapshot()))))
            .unwrap()
        else {
            panic!()
        };
        c.complete(Response::Read(old, Err("stale".into())))
            .unwrap();
        assert!(c.busy == Busy::Read(new));
        assert_eq!(
            c.paint(Rect::new(0, 0, 100, 20)).freshness,
            super::super::Freshness::Refreshing
        );
    }
    #[test]
    fn resize_is_cached_and_dispatch_uses_exact_capture() {
        let mut c = ready();
        let action = c.painted.as_ref().unwrap().documents[&'a'].clone();
        c.resize();
        assert!(!c.view.is_current(&action));
        assert!(c.input(key('a')).is_none());
        assert!(c.busy == Busy::Idle);
        c.paint(Rect::new(0, 0, 100, 30));
        let expected = c.painted.as_ref().unwrap().documents[&'a'].target().clone();
        let Some(Request::Activate(observed, target)) = c.input(key('a')) else {
            panic!()
        };
        assert_eq!(*observed, snapshot());
        assert_eq!(target, expected);
        assert!(c.input(key('a')).is_none());
        assert!(matches!(
            c.complete(Response::Activated(Err("denied".into())))
                .unwrap(),
            Some(Request::Read(_, _))
        ));
        assert!(c.input(key('a')).is_none());
        let Busy::Read(token) = c.busy else { panic!() };
        c.complete(Response::Read(token, Ok(Box::new(snapshot()))))
            .unwrap();
        let frame = c.paint(Rect::new(0, 0, 100, 30));
        let text = (0..30)
            .map(|y| super::super::render::buffer_line(&frame.buffer, y))
            .collect::<String>();
        assert!(text.contains("Activation failed: denied"));
    }
    #[test]
    fn only_painted_shortcuts_and_press_events_dispatch() {
        let mut c = ready();
        for input in [
            key('1'),
            key('2'),
            key('z'),
            Event::Paste("a".into()),
            Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('a'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat,
            )),
            Event::Key(KeyEvent::new_with_kind(
                KeyCode::Char('a'),
                KeyModifiers::NONE,
                KeyEventKind::Release,
            )),
        ] {
            assert!(c.input(input).is_none());
        }
        assert!(!quit(&key('r')));
        assert!(quit(&key('q')));
        let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(c.input(refresh).is_none());
        assert!(matches!(
            c.next_read(),
            Some(Request::Read(_, Refresh::Full))
        ));
    }
    #[test]
    fn lowercase_r_is_a_document_key_and_links_use_hit_regions() {
        let mut s = snapshot();
        for index in 1..18 {
            let mut reference = s.records.stream.references[0].clone();
            let crate::model::ReferenceTarget::Document(document) = &mut reference.target else {
                panic!()
            };
            document.path = format!("docs/{index}.md");
            let mut location = s.context.documents[0].clone();
            location.path = document.path.clone();
            s.records.stream.references.push(reference);
            s.context.documents.push(location);
        }
        let s = CardSnapshot::new(s.records.stream, s.records.session, s.context).unwrap();
        let mut c = Controller::new();
        let Some(Request::Read(token, _)) = c.complete(Response::Ready(Ok(()))).unwrap() else {
            panic!()
        };
        c.complete(Response::Read(token, Ok(Box::new(s)))).unwrap();
        let expected = c.paint(Rect::new(0, 0, 120, 40)).documents[&'r']
            .target()
            .clone();
        assert!(
            matches!(c.input(key('r')), Some(Request::Activate(_, target)) if target == expected)
        );
        let mut c = ready();
        let link = c.painted.as_ref().unwrap().links[0].clone();
        let input = Event::Mouse(event::MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: link.area.x,
            row: link.area.y,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            matches!(c.input(input), Some(Request::Activate(_, target)) if target == *link.action.target())
        );
    }
    #[test]
    fn read_error_is_displayed_and_factory_error_returned() {
        let mut c = ready();
        let Some(Request::Read(token, _)) = c.refresh() else {
            panic!()
        };
        c.complete(Response::Read(token, Err("unattested".into())))
            .unwrap();
        let frame = c.paint(Rect::new(0, 0, 100, 20));
        assert!(frame.documents.is_empty());
        assert!(
            (0..20)
                .map(|y| super::super::render::buffer_line(&frame.buffer, y))
                .collect::<String>()
                .contains("unattested")
        );
        assert!(
            Controller::new()
                .complete(Response::Ready(Err("startup".into())))
                .err()
                .unwrap()
                .contains("startup")
        );
    }

    struct Fake {
        calls: Sender<&'static str>,
        release: Receiver<()>,
        read_result: Result<CardSnapshot, String>,
        _not_send: std::rc::Rc<()>,
    }
    impl PaneHost for Fake {
        fn read(&mut self, _refresh: Refresh) -> Result<CardSnapshot, String> {
            self.calls.send("read").unwrap();
            self.release.recv().unwrap();
            self.read_result.clone()
        }
        fn activate(&mut self, _: &CardSnapshot, _: &Target) -> Result<(), String> {
            self.calls.send("activate").unwrap();
            Ok(())
        }
    }
    fn observation_worker(
        result: Result<CardSnapshot, String>,
    ) -> (Worker, Receiver<&'static str>, Sender<()>) {
        let (calls, seen) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let worker = Worker::start(move |_, _| {
            Ok(Fake {
                calls,
                release: blocked,
                read_result: result,
                _not_send: std::rc::Rc::new(()),
            })
        })
        .unwrap();
        (worker, seen, release)
    }
    #[test]
    fn starting_exit_joins_without_reading_or_effects() {
        let (mut worker, seen, _release) = observation_worker(Ok(snapshot()));
        worker.finish().unwrap();
        assert!(seen.try_recv().is_err());
    }
    #[test]
    fn exit_retires_inflight_results_without_dispatching_more_io() {
        for result in [Err("initial read failed".into()), Ok(snapshot())] {
            let (mut worker, seen, release) = observation_worker(result);
            let mut controller = Controller::new();
            worker
                .send(
                    controller
                        .complete(worker.responses.recv().unwrap())
                        .unwrap()
                        .unwrap(),
                )
                .unwrap();
            assert_eq!(seen.recv().unwrap(), "read");
            worker.cancel.cancel().unwrap();
            release.send(()).unwrap();
            worker.finish().unwrap();
            assert!(seen.try_recv().is_err());
            assert!(controller.snapshot.is_none());
        }
    }

    #[test]
    fn manual_refresh_clears_activation_failure_notice() {
        let mut c = ready();
        assert!(matches!(c.input(key('a')), Some(Request::Activate(..))));
        let Some(Request::Read(token, _)) = c
            .complete(Response::Activated(Err("denied".into())))
            .unwrap()
        else {
            panic!()
        };
        c.complete(Response::Read(token, Ok(Box::new(snapshot()))))
            .unwrap();
        let frame = c.paint(Rect::new(0, 0, 100, 30));
        let text = (0..30)
            .map(|y| super::super::render::buffer_line(&frame.buffer, y))
            .collect::<String>();
        assert!(text.contains("Activation failed: denied"));
        let refresh = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(c.input(refresh).is_none());
        let Some(Request::Read(token, Refresh::Full)) = c.next_read() else {
            panic!()
        };
        c.complete(Response::Read(token, Ok(Box::new(snapshot()))))
            .unwrap();
        let frame = c.paint(Rect::new(0, 0, 100, 30));
        let text = (0..30)
            .map(|y| super::super::render::buffer_line(&frame.buffer, y))
            .collect::<String>();
        assert!(!text.contains("Activation failed"));
    }

    struct TeardownHost {
        restored: Arc<AtomicBool>,
        calls: Sender<&'static str>,
    }
    impl PaneHost for TeardownHost {
        fn read(&mut self, _refresh: Refresh) -> Result<CardSnapshot, String> {
            unreachable!()
        }
        fn activate(&mut self, _: &CardSnapshot, _: &Target) -> Result<(), String> {
            unreachable!()
        }
    }
    impl Drop for TeardownHost {
        fn drop(&mut self) {
            assert!(self.restored.load(Ordering::SeqCst));
            self.calls.send("drop").unwrap();
        }
    }
    #[test]
    fn restoration_precedes_worker_teardown_on_every_exit() {
        for outcome in [Ok(()), Err("draw failed".into())] {
            for restore_fails in [false, true] {
                let restored = Arc::new(AtomicBool::new(false));
                let observed = restored.clone();
                let (calls, seen) = mpsc::channel();
                let mut worker = Worker::start(move |_, _| {
                    Ok(TeardownHost {
                        restored: observed,
                        calls,
                    })
                })
                .unwrap();
                assert!(matches!(
                    worker.responses.recv().unwrap(),
                    Response::Ready(Ok(()))
                ));
                let fails = restore_fails || outcome.is_err();
                let result = finish_interaction(&mut worker, outcome.clone(), || {
                    restored.store(true, Ordering::SeqCst);
                    if restore_fails {
                        Err(io::Error::other("restore failed"))
                    } else {
                        Ok(())
                    }
                });
                assert_eq!(result.is_err(), fails);
                assert!(worker.cancel.flag().load(Ordering::SeqCst));
                assert!(worker.thread.is_none());
                assert_eq!(seen.try_iter().collect::<Vec<_>>(), ["drop"]);
            }
        }
    }
}
