use super::*;
use std::os::unix::net::UnixListener;
#[allow(dead_code)]
#[path = "../../../tests/support/pty.rs"]
mod pty;

struct Host;
impl PaneHost for Host {
    fn read(&mut self, _: Refresh) -> Result<CardSnapshot, String> {
        Ok(super::tests::snapshot())
    }
    fn activate(&mut self, _: &CardSnapshot, _: &Target) -> Result<(), String> {
        panic!("idle test must not activate")
    }
}
#[test]
fn event_loop_has_zero_wakeups_while_idle() {
    if let Some(root) = std::env::var_os("STREAMS_TEST_IDLE_ROOT") {
        let mut signals = Signals::install().unwrap();
        let mut state = TerminalState::enter().unwrap();
        let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout())).unwrap();
        let mut worker = Worker::start(|_, _| Ok(Host)).unwrap();
        signals.attach(worker.events.clone()).unwrap();
        let mut controller = Controller::new();
        let wakeups = controller.wakeups.clone();
        let socket = std::path::PathBuf::from(root).join("counts.sock");
        // Test-only observer uses a separate socket; sampling cannot wake the UI.
        let observer = thread::spawn(move || {
            let mut connection = UnixStream::connect(socket).unwrap();
            while connection.read_exact(&mut [0]).is_ok() {
                connection
                    .write_all(&(wakeups.load(Ordering::SeqCst) as u64).to_be_bytes())
                    .unwrap();
            }
        });
        let outcome = interact(&mut terminal, &mut controller, &worker, &signals);
        drop(terminal);
        finish_interaction(&mut worker, outcome, || state.restore()).unwrap();
        observer.join().unwrap();
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let listener = UnixListener::bind(root.path().join("counts.sock")).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tui::application::idle_tests::event_loop_has_zero_wakeups_while_idle",
            "--nocapture",
        ])
        .env("STREAMS_TEST_IDLE_ROOT", root.path())
        .env("TERM", "xterm-256color");
    let mut child = pty::Pty::spawn(command, false);
    let mut connection = None;
    child.until(|p| {
        if connection.is_none() {
            match listener.accept() {
                Ok((stream, _)) => connection = Some(stream),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => panic!("{error}"),
            }
        }
        connection.is_some()
            && p.screen().contains("Ship library | client")
            && p.screen().contains("[q] close")
    });
    let mut connection = connection.unwrap();
    connection.set_nonblocking(false).unwrap();
    connection
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let sample = |connection: &mut UnixStream| {
        connection.write_all(b"?").unwrap();
        let mut count = [0; 8];
        connection.read_exact(&mut count).unwrap();
        u64::from_be_bytes(count)
    };
    let before = sample(&mut connection);
    thread::sleep(Duration::from_millis(100));
    let after = sample(&mut connection);
    assert_eq!(after - before, 0, "idle must not wake the actual UI loop");
    drop(connection);
    child.send(b"q");
    child.finish();
}
