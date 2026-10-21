//! Terminal observations are never effect authority; the host owns fresh native proofs.
use std::{
    io::{self, IsTerminal, Stdout, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use super::{CardSnapshot, CardView, PaintedFrame, Rect, RefreshToken, Target};
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

/// All methods are bounded, including construction and cancellation cleanup.
/// `read` returns only after fresh source attestation AND native display verification.
/// `activate` and `close` independently revalidate fresh proofs before effects;
/// UI frames and their captured snapshots are observations, never effect authority.
/// The host checks its factory-supplied cancellation flag between bounded effects.
/// Dropping a host must not close the native card or perform new native effects.
pub trait PaneHost {
    fn read(&mut self) -> Result<CardSnapshot, String>;
    fn activate(&mut self, snapshot: &CardSnapshot, target: &Target) -> Result<(), String>;
    fn close(&mut self) -> Result<(), String>;
}

enum Request {
    Read(RefreshToken),
    Activate(Box<CardSnapshot>, Target),
    Close,
}
enum Response {
    Ready(Result<(), String>),
    Read(RefreshToken, Result<Box<CardSnapshot>, String>),
    Activated(Result<(), String>),
    Closed(Result<(), String>),
}

struct Worker {
    requests: Option<Sender<Request>>,
    responses: Receiver<Response>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    fn start<H, F>(make_host: F) -> Result<Self, String>
    where
        H: PaneHost + 'static,
        F: FnOnce(Arc<AtomicBool>) -> Result<H, String> + Send + 'static,
    {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (tx, requests) = mpsc::channel();
        let (responses, rx) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("card-native-io".into())
            .spawn(move || {
                // H need not be Send: its entire lifetime is on this thread.
                let mut host = match make_host(flag.clone()) {
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
                        Request::Read(token) => Response::Read(token, host.read().map(Box::new)),
                        Request::Activate(snapshot, target) => {
                            Response::Activated(host.activate(&snapshot, &target))
                        }
                        Request::Close => {
                            let _ = responses.send(Response::Closed(host.close()));
                            break; // Never retry an uncertain close.
                        }
                    };
                    if responses.send(response).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
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
        self.cancel.store(true, Ordering::SeqCst);
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
    Closing,
}
struct Controller {
    view: CardView,
    snapshot: Option<CardSnapshot>,
    painted: Option<PaintedFrame>,
    busy: Busy,
    dirty: bool,
}
impl Controller {
    fn new() -> Self {
        Self {
            view: CardView::new(),
            snapshot: None,
            painted: None,
            busy: Busy::Starting,
            dirty: true,
        }
    }
    fn refresh(&mut self) -> Option<Request> {
        if self.busy != Busy::Idle {
            return None;
        }
        let token = self.view.begin_refresh(); // Invalidate BEFORE dispatch, not on completion.
        self.busy = Busy::Read(token);
        self.dirty = true;
        Some(Request::Read(token))
    }
    fn complete(&mut self, response: Response) -> Result<Option<Request>, String> {
        match response {
            Response::Ready(result) if self.busy == Busy::Starting => {
                result?;
                self.busy = Busy::Idle;
                return Ok(self.refresh());
            }
            Response::Read(token, result) if self.busy == Busy::Read(token) => {
                let result = result.map(|snapshot| *snapshot);
                let snapshot = result.as_ref().ok().cloned();
                if self.view.complete_refresh(token, result) {
                    self.snapshot = snapshot;
                }
                self.busy = Busy::Idle;
            }
            Response::Activated(result) if self.busy == Busy::Activate => {
                self.view
                    .set_notice(result.err().map(|e| format!("Activation failed: {e}")));
                self.busy = Busy::Idle;
                return Ok(self.refresh());
            }
            _ => return Ok(None),
        }
        self.dirty = true;
        Ok(None)
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
    fn input(&mut self, event: Event) -> Option<Request> {
        if self.busy != Busy::Idle {
            return None;
        }
        if matches!(&event, Event::Key(k) if k.kind == KeyEventKind::Press
            && k.code == KeyCode::Char('r') && k.modifiers == KeyModifiers::CONTROL)
        {
            return self.refresh();
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
    fn close(&mut self) -> Option<Request> {
        if self.busy != Busy::Idle {
            return None;
        }
        self.busy = Busy::Closing;
        Some(Request::Close)
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
}
impl Signals {
    fn install() -> Result<Self, String> {
        let mut signals = Self {
            raised: Arc::new(AtomicBool::new(false)),
            ids: Vec::new(),
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
}
impl Drop for Signals {
    fn drop(&mut self) {
        for id in self.ids.drain(..) {
            signal_hook::low_level::unregister(id);
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

/// Requires interactive stdin/stdout. The single worker is joined on every exit,
/// including unwind; bounded native calls are the host's responsibility.
/// External signals retain the native card. Only explicit q/Ctrl-C requests close.
pub fn run<H, F>(make_host: F, poll_interval: Duration) -> Result<(), String>
where
    H: PaneHost + 'static,
    F: FnOnce(Arc<AtomicBool>) -> Result<H, String> + Send + 'static,
{
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err("Card requires interactive stdin and stdout".into());
    }
    let signals = Signals::install()?;
    let mut state = TerminalState::enter().map_err(|e| e.to_string())?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).map_err(|e| e.to_string())?;
    // Validate terminal input before even constructing the native host.
    event::poll(Duration::ZERO).map_err(|e| e.to_string())?;
    if signals.raised.load(Ordering::SeqCst) {
        return Ok(());
    }
    let mut worker = Worker::start(make_host)?;
    let mut controller = Controller::new();
    let outcome = interact(
        &mut terminal,
        &mut controller,
        &worker,
        &signals,
        poll_interval,
    );
    // Ratatui's Drop may show the cursor; run it BEFORE restoring saved modes.
    drop(terminal);
    // A terminal failure or external signal never schedules native close.
    match outcome {
        Ok(true) => {
            // Keep handlers alive, but do NOT translate the resulting Herdr HUP into
            // cancellation while the one bounded close is awaiting acknowledgement.
            state
                .restore()
                .map_err(|e| format!("Terminal restore failed; native card retained: {e}"))?;
            explicit_close(&mut controller, &worker)?;
        }
        Ok(false) => {
            worker.cancel.store(true, Ordering::SeqCst);
        }
        Err(error) => {
            worker.cancel.store(true, Ordering::SeqCst);
            return Err(error);
        }
    }
    worker.finish()?;
    state.restore().map_err(|e| e.to_string())
}

fn interact(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    controller: &mut Controller,
    worker: &Worker,
    signals: &Signals,
    interval: Duration,
) -> Result<bool, String> {
    let mut refreshed = Instant::now();
    let tick = Duration::from_millis(20);
    loop {
        if signals.raised.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if controller.dirty {
            terminal
                .draw(|frame| {
                    let area = frame.area();
                    frame.buffer_mut().merge(&controller.paint(area).buffer);
                })
                .map_err(|e| e.to_string())?;
        }
        // Drain bounded batches BEFORE accepting completions. Input accumulated
        // during I/O remains busy/discarded, never replayed against a fresh frame.
        let mut drained = false;
        for _ in 0..64 {
            if !event::poll(Duration::ZERO).map_err(|e| e.to_string())? {
                drained = true;
                break;
            }
            let input = event::read().map_err(|e| e.to_string())?;
            if signals.raised.load(Ordering::SeqCst) {
                return Ok(false);
            }
            if quit(&input) {
                return Ok(true);
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
        match worker.responses.try_recv() {
            Ok(response) => {
                if let Some(request) = controller.complete(response)? {
                    worker.send(request)?;
                }
                refreshed = Instant::now();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err("Native worker disconnected; native card retained or uncertain".into());
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if refreshed.elapsed() >= interval.max(tick)
            && let Some(request) = controller.refresh()
        {
            worker.send(request)?;
            refreshed = Instant::now();
        }
        event::poll(tick).map_err(|e| e.to_string())?;
    }
}

fn explicit_close(controller: &mut Controller, worker: &Worker) -> Result<(), String> {
    let retained = |e| format!("Native card retained or close uncertain: {e}");
    // Settle the sole operation, without scheduling its usual follow-up refresh.
    while !matches!(controller.busy, Busy::Idle | Busy::Closing) {
        match worker
            .responses
            .recv()
            .map_err(|e| retained(e.to_string()))?
        {
            Response::Ready(result) => {
                result.map_err(retained)?;
                controller.busy = Busy::Idle;
            }
            Response::Read(..) | Response::Activated(..) => controller.busy = Busy::Idle,
            Response::Closed(result) => return result.map_err(retained),
        }
    }
    if let Some(request) = controller.close() {
        worker.send(request).map_err(retained)?;
    }
    match worker
        .responses
        .recv()
        .map_err(|e| retained(e.to_string()))?
    {
        Response::Closed(result) => result.map_err(retained),
        _ => Err(retained("Unexpected close response".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::{
        CheckoutBinding, CheckoutSelection, DisplayContext, DocumentLocation,
        NativeOwnerObservation, ResolvedDocument,
    };
    use ratatui::crossterm::event::KeyEvent;

    fn snapshot() -> CardSnapshot {
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
                    repository: "lib".into(),
                    selection: selection.clone(),
                }],
                documents: vec![DocumentLocation {
                    repository: "lib".into(),
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
        let Some(Request::Read(token)) = c.complete(Response::Ready(Ok(()))).unwrap() else {
            panic!()
        };
        c.complete(Response::Read(token, Ok(Box::new(snapshot()))))
            .unwrap();
        c.paint(Rect::new(0, 0, 100, 20));
        c
    }
    #[test]
    fn refresh_invalidates_and_stale_completion_is_ignored() {
        let mut c = ready();
        let action = c.painted.as_ref().unwrap().documents[&'a'].clone();
        let Some(Request::Read(old)) = c.refresh() else {
            panic!()
        };
        assert!(!c.view.is_current(&action));
        assert!(c.input(key('a')).is_none());
        assert!(c.refresh().is_none());
        c.complete(Response::Read(old, Ok(Box::new(snapshot()))))
            .unwrap();
        let Some(Request::Read(new)) = c.refresh() else {
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
            Some(Request::Read(_))
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
        assert!(matches!(c.input(refresh), Some(Request::Read(_))));
    }
    #[test]
    fn lowercase_r_is_a_document_key_and_links_use_hit_regions() {
        let mut s = snapshot();
        for index in 1..18 {
            let mut document = s.stream.documents[0].clone();
            document.path = format!("docs/{index}.md");
            let mut location = s.context.documents[0].clone();
            location.path = document.path.clone();
            s.stream.documents.push(document);
            s.context.documents.push(location);
        }
        let s = CardSnapshot::new(s.stream, s.session, s.context).unwrap();
        let mut c = Controller::new();
        let Some(Request::Read(token)) = c.complete(Response::Ready(Ok(()))).unwrap() else {
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
        let Some(Request::Read(token)) = c.refresh() else {
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
        fail_close: bool,
        _not_send: std::rc::Rc<()>,
    }
    impl PaneHost for Fake {
        fn read(&mut self) -> Result<CardSnapshot, String> {
            self.calls.send("read").unwrap();
            self.release.recv().unwrap();
            Ok(snapshot())
        }
        fn activate(&mut self, _: &CardSnapshot, _: &Target) -> Result<(), String> {
            self.calls.send("activate").unwrap();
            Ok(())
        }
        fn close(&mut self) -> Result<(), String> {
            self.calls.send("close").unwrap();
            if self.fail_close {
                Err("ack missing".into())
            } else {
                Ok(())
            }
        }
    }
    #[test]
    fn signal_cancellation_joins_without_close() {
        let (calls, seen) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let mut worker = Worker::start(move |_| {
            Ok(Fake {
                calls,
                release: blocked,
                fail_close: false,
                _not_send: std::rc::Rc::new(()),
            })
        })
        .unwrap();
        let mut c = Controller::new();
        worker
            .send(
                c.complete(worker.responses.recv().unwrap())
                    .unwrap()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(seen.recv().unwrap(), "read");
        worker.cancel.store(true, Ordering::SeqCst); // Same path as external signal.
        release.send(()).unwrap();
        worker.finish().unwrap();
        assert!(seen.try_recv().is_err());
    }
    #[test]
    fn explicit_close_is_once_and_uncertainty_is_returned() {
        let (calls, seen) = mpsc::channel();
        let (_release, blocked) = mpsc::channel();
        let mut worker = Worker::start(move |_| {
            Ok(Fake {
                calls,
                release: blocked,
                fail_close: true,
                _not_send: std::rc::Rc::new(()),
            })
        })
        .unwrap();
        assert!(matches!(
            worker.responses.recv().unwrap(),
            Response::Ready(Ok(()))
        ));
        let mut c = ready();
        assert!(
            explicit_close(&mut c, &worker)
                .unwrap_err()
                .contains("retained or close uncertain: ack missing")
        );
        assert_eq!(seen.recv().unwrap(), "close");
        assert!(c.close().is_none());
        worker.finish().unwrap();
        assert!(seen.try_recv().is_err());
    }
}
