//! A live descendant holds a FIFO reader until released or killed.
use std::{
    ffi::CString,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct DescendantProbe {
    root: PathBuf,
}

impl DescendantProbe {
    fn new(root: &Path) -> Self {
        let fifo = CString::new(root.join("descendant-probe").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        Self { root: root.into() }
    }

    fn script(&self) -> &'static str {
        // The reader also holds a writer, so parent exit cannot turn read into EOF.
        // Readiness is recorded only after fd 3 is open; no startup race can pass.
        "trap '' TERM; (exec 3<>descendant-probe; printf ready > descendant-ready; read wakeup <&3; printf survived > descendant-survived) & wait"
    }

    fn reader_exists(&self) -> bool {
        match OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(self.root.join("descendant-probe"))
        {
            Ok(_) => true,
            Err(error) => {
                assert_eq!(error.raw_os_error(), Some(libc::ENXIO), "{error}");
                false
            }
        }
    }

    fn assert_killed(&self) {
        assert_eq!(
            fs::read(self.root.join("descendant-ready")).unwrap(),
            b"ready"
        );
        // SIGKILL delivery/descriptor teardown can trail the leader's waitpid.
        let deadline = Instant::now() + Duration::from_secs(1);
        while self.reader_exists() {
            if Instant::now() >= deadline {
                // Release a survivor before failing, rather than leaking a blocked child.
                if let Ok(mut writer) = OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(self.root.join("descendant-probe"))
                {
                    let _ = writer.write_all(b"wake\n");
                }
                panic!("descendant still owns its FIFO after process-group cleanup");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!self.root.join("descendant-survived").exists());
    }
}

#[test]
fn probe_detects_a_live_reader_and_its_closure() {
    let root = tempfile::tempdir().unwrap();
    let probe = DescendantProbe::new(root.path());
    assert!(!probe.reader_exists());
    let reader = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(root.path().join("descendant-probe"))
        .unwrap();
    assert!(probe.reader_exists());
    drop(reader);
    assert!(!probe.reader_exists());
}

#[test]
fn termination_kills_ready_descendants_and_reaps_the_leader() {
    let root = tempfile::tempdir().unwrap();
    let probe = DescendantProbe::new(root.path());
    let mut child = Command::new("/bin/sh")
        .args(["-c", probe.script()])
        .current_dir(root.path())
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let ready = loop {
        if fs::read(root.path().join("descendant-ready")).is_ok_and(|bytes| bytes == b"ready") {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let error = super::terminate(&mut child, "termination test".into());
    assert!(ready, "descendant did not become ready: {}", error.message);
    assert_eq!(error.message, "termination test");
    assert_eq!(error.code, None);
    probe.assert_killed();
    let waited = unsafe {
        libc::waitpid(
            child.id() as libc::pid_t,
            std::ptr::null_mut(),
            libc::WNOHANG,
        )
    };
    assert_eq!(waited, -1, "termination did not reap the leader");
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD)
    );
}
