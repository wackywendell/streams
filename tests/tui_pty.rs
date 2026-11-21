//! Real PTY / Crossterm / Ratatui acceptance. Only the native host is a fixture;
//! CLI/native authorization and durable effect evidence have separate boundary tests.
use crate::support::pty::Pty;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use streams::tui::{
    CardSnapshot, DisplayContext, DocumentLocation, NativeOwnerObservation, ResolvedDocument,
    Target,
    application::{self, PaneHost},
};

fn snapshot(read: usize) -> CardSnapshot {
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../src/tui/fixtures/active-renderer.json")).unwrap();
    let source = &fixtures[0]["snapshot"];
    let mut session = source["session"].clone();
    session["recap"] = serde_json::json!({
        "text": format!("Contribution [read {read}]"),
        "short": null
    });
    CardSnapshot::new(
        serde_json::from_value(source["stream"].clone()).unwrap(),
        serde_json::from_value(session).unwrap(),
        DisplayContext {
            catalog_source: "catalog".into(),
            owner: NativeOwnerObservation {
                pane_id: "w:p1".into(),
                tab_id: "w:t1".into(),
                workspace_id: "w".into(),
            },
            documents: vec![DocumentLocation {
                path: "docs/design.md".into(),
                location: ResolvedDocument::File {
                    root: "/code/library".into(),
                    file: "/code/library/docs/design.md".into(),
                },
            }],
        },
    )
    .unwrap()
}

#[derive(Clone, Copy, strum::Display, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
enum Scenario {
    Normal,
    BlockedRead,
    BlockedActivation,
    ReadError,
    FactoryError,
    FactoryPanic,
}

struct Host {
    root: PathBuf,
    mode: Scenario,
    cancel: Arc<AtomicBool>,
    snapshot: CardSnapshot,
}
impl Host {
    fn event(&self, text: &str) {
        writeln!(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.root.join("events"))
                .unwrap(),
            "{text}"
        )
        .unwrap();
    }
    fn wait(&self, release: &str) -> Result<(), String> {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !self.root.join(release).exists() {
            if self.cancel.load(Ordering::SeqCst) {
                self.event("cancelled");
                return Err("fixture cancelled".into());
            }
            if Instant::now() >= deadline {
                return Err("fixture release timed out".into());
            }
            thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
}
impl PaneHost for Host {
    fn read(&mut self, _refresh: application::Refresh) -> Result<CardSnapshot, String> {
        self.event("read");
        if matches!(self.mode, Scenario::BlockedRead) || self.root.join("block-read").exists() {
            self.wait("release-read")?;
        }
        if matches!(self.mode, Scenario::ReadError) {
            return Err("injected read failure".into());
        }
        let read = events(&self.root).matches("read\n").count();
        self.snapshot = snapshot(read);
        Ok(self.snapshot.clone())
    }
    fn activate(&mut self, observed: &CardSnapshot, target: &Target) -> Result<(), String> {
        assert_eq!(observed, &self.snapshot);
        match target {
            Target::Document(document) => {
                assert_eq!(
                    **document,
                    self.snapshot.document_target("docs/design.md").unwrap()
                );
                self.event("document:lib:docs/design.md");
            }
            Target::Link(_) => panic!("PTY fixture must never open a real browser"),
            Target::Artifact(_) => panic!("this PTY fixture has no artifact references"),
        }
        if matches!(self.mode, Scenario::BlockedActivation) {
            self.wait("release-activation")?;
        }
        if self.root.join("activation-error").exists() {
            return Err("injected activation failure".into());
        }
        Ok(())
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.event("dropped");
    }
}

// Re-enter the same test in a fresh process with real tty stdin/stdout. No extra
// test-only executable, ignored helper test, production hook or global env mutation.
fn child_fixture() -> bool {
    let Some(root) = std::env::var_os("STREAMS_TEST_PTY_ROOT") else {
        return false;
    };
    let root = PathBuf::from(root);
    let mode: Scenario = std::env::var("STREAMS_TEST_PTY_MODE")
        .unwrap()
        .parse()
        .unwrap();
    let result_root = root.clone();
    let result = application::run(move |cancel, _events| -> Result<Host, String> {
        if matches!(mode, Scenario::FactoryError) {
            return Err("injected factory failure".into());
        }
        if matches!(mode, Scenario::FactoryPanic) {
            panic!("injected worker panic");
        }
        Ok(Host {
            root,
            mode,
            cancel: cancel.flag(),
            snapshot: snapshot(0),
        })
    });
    fs::write(
        result_root.join("result.tmp"),
        serde_json::to_vec(&result.map(|_| ())).unwrap(),
    )
    .unwrap();
    fs::rename(result_root.join("result.tmp"), result_root.join("result")).unwrap();
    // Darwin revokes the slave when its controlling session exits. Keep the child
    // alive until the parent has checked restored termios on the live terminal.
    let deadline = Instant::now() + Duration::from_secs(8);
    while !result_root.join("release-exit").exists() {
        assert!(
            Instant::now() < deadline,
            "parent did not acknowledge terminal cleanup"
        );
        thread::sleep(Duration::from_millis(5));
    }
    true
}

fn start(root: &Path, mode: &str, raw: bool) -> Pty {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            thread::current().name().unwrap(),
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .current_dir(root)
        .env("HOME", root)
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_DATA_HOME", root.join("data"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("TERM", "xterm-256color")
        .env("LC_ALL", "C.UTF-8")
        .env("STREAMS_TEST_PTY_ROOT", root)
        .env("STREAMS_TEST_PTY_MODE", mode);
    Pty::spawn(command, raw)
}
fn events(root: &Path) -> String {
    fs::read_to_string(root.join("events")).unwrap_or_default()
}
fn result(root: &Path) -> Result<(), String> {
    serde_json::from_slice(&fs::read(root.join("result")).unwrap()).unwrap()
}
fn ready(pty: &mut Pty) {
    pty.until(|p| {
        let screen = p.screen();
        screen.contains("[a] Document Design")
            && screen.contains("[q] close")
            && !screen.contains("Refreshing")
    });
}
fn release(root: &Path, file: &str) {
    fs::write(root.join(file), "release").unwrap();
}
fn finish(pty: &mut Pty, root: &Path) {
    pty.until(|_| root.join("result").exists());
    pty.assert_cleanup();
    release(root, "release-exit");
    pty.finish();
}

#[test]
fn paints_resizes_and_activates_the_painted_document() {
    if child_fixture() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut pty = start(root.path(), "normal", false);
    ready(&mut pty);
    assert!(pty.screen().contains("Ship library | client"));
    let reads = events(root.path());
    pty.resize(12, 50);
    pty.until(|p| {
        p.screen()
            .lines()
            .nth(11)
            .is_some_and(|line| line.contains("[q] close"))
            && p.screen().starts_with("Ship library | client")
    });
    assert_eq!(
        events(root.path()),
        reads,
        "resize must use the cached observation"
    );
    pty.send(b"a");
    pty.until(|_| events(root.path()).matches("read\n").count() == 2);
    ready(&mut pty);
    pty.send(b"q");
    finish(&mut pty, root.path());
    assert_eq!(result(root.path()), Ok(()));
    assert_eq!(
        events(root.path()),
        "read\ndocument:lib:docs/design.md\nread\ndropped\n"
    );
}

#[test]
fn ctrl_c_restores_and_exits_without_a_close_operation() {
    if child_fixture() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut pty = start(root.path(), "normal", false);
    ready(&mut pty);
    pty.send(b"\x03");
    finish(&mut pty, root.path());
    assert_eq!(result(root.path()), Ok(()));
    assert_eq!(events(root.path()), "read\ndropped\n");
}

#[test]
fn quit_cancels_the_initial_read_and_never_replays_busy_input() {
    if child_fixture() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut pty = start(root.path(), "blocked-read", false);
    pty.until(|_| events(root.path()) == "read\n");
    pty.send(b"aq");
    pty.until(Pty::restored);
    pty.assert_cleanup();
    finish(&mut pty, root.path());
    assert_eq!(result(root.path()), Ok(()));
    assert!(
        !root.path().join("release-read").exists(),
        "quit must not await the blocked read"
    );
    assert_eq!(events(root.path()), "read\ncancelled\ndropped\n");
}

#[test]
fn external_signals_cancel_and_join_inflight_work_without_closing() {
    if child_fixture() {
        return;
    }
    for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP] {
        let root = tempfile::tempdir().unwrap();
        let mut pty = start(root.path(), "blocked-activation", false);
        ready(&mut pty);
        pty.send(b"a");
        pty.until(|_| events(root.path()).contains("document:lib:docs/design.md\n"));
        pty.send(b"a"); // Busy input must not be replayed after cancellation.
        pty.signal(signal);
        finish(&mut pty, root.path());
        assert_eq!(result(root.path()), Ok(()));
        assert_eq!(
            events(root.path()),
            "read\ndocument:lib:docs/design.md\ncancelled\ndropped\n"
        );
    }
}

#[test]
fn failed_observation_disables_actions_but_allows_exit_without_effects() {
    if child_fixture() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut pty = start(root.path(), "read-error", false);
    pty.until(|p| p.screen().contains("injected read failure"));
    pty.send(b"aq");
    finish(&mut pty, root.path());
    assert_eq!(result(root.path()), Ok(()));
    assert_eq!(events(root.path()), "read\ndropped\n");
}

#[test]
fn startup_failure_restores_both_cooked_and_preexisting_raw_modes() {
    if child_fixture() {
        return;
    }
    for raw in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut pty = start(root.path(), "factory-error", raw);
        finish(&mut pty, root.path());
        assert_eq!(result(root.path()), Err("injected factory failure".into()));
        assert_eq!(events(root.path()), "");
    }
}

#[test]
fn refresh_clears_failure_notice_and_clipped_documents_cannot_activate() {
    if child_fixture() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    release(root.path(), "activation-error");
    let mut pty = start(root.path(), "normal", false);
    ready(&mut pty);
    pty.send(b"a");
    pty.until(|p| {
        p.screen().contains("[read 2]")
            && p.screen()
                .contains("Activation failed: injected activation failure")
    });
    fs::remove_file(root.path().join("activation-error")).unwrap();
    release(root.path(), "block-read");
    pty.send(b"\x12");
    pty.until(|p| {
        events(root.path()).matches("read\n").count() == 3
            && p.screen().contains("Document Design")
            && !p.screen().contains("Refreshing")
            && !p.screen().contains("Activation failed")
    });
    pty.send(b"a"); // This visible-but-stale shortcut must be discarded while refreshing.
    release(root.path(), "release-read");
    pty.until(|p| {
        p.screen().contains("[read 3]")
            && !p.screen().contains("Activation failed")
            && !p.screen().contains("Refreshing")
    });
    pty.resize(2, 40);
    pty.until(|p| {
        p.screen()
            .lines()
            .nth(1)
            .is_some_and(|line| line.contains("[q] close"))
            && !p.screen().contains("Document Design")
    });
    pty.send(b"aq");
    finish(&mut pty, root.path());
    assert_eq!(result(root.path()), Ok(()));
    assert_eq!(events(root.path()).matches("document:").count(), 1);
    assert_eq!(events(root.path()).matches("read\n").count(), 3);
}

#[test]
fn worker_panic_restores_terminal_without_native_effects() {
    if child_fixture() {
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut pty = start(root.path(), "factory-panic", false);
    finish(&mut pty, root.path());
    assert!(
        result(root.path())
            .unwrap_err()
            .contains("Native worker disconnected")
    );
    assert_eq!(events(root.path()), "");
}
