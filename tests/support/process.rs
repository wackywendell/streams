//! A live descendant holds a FIFO reader until released or killed.
use std::{
    ffi::CString,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct DescendantProbe {
    root: PathBuf,
}

impl DescendantProbe {
    pub fn new(root: &Path) -> Self {
        let fifo = CString::new(root.join("descendant-probe").as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        Self { root: root.into() }
    }

    pub fn script(&self) -> &'static str {
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

    pub fn assert_killed(&self) {
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
