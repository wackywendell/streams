//! Herdr launch evidence, independent of catalog open journals. Never replay a pending attempt.

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{Result, digest, invalid};
use crate::file_io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resources {
    pane_id: String,
    terminal_id: String,
    tab_id: String,
}

impl Resources {
    pub fn new(
        pane: impl Into<String>,
        terminal: impl Into<String>,
        tab: impl Into<String>,
    ) -> Result<Self> {
        let value = Self {
            pane_id: pane.into(),
            terminal_id: terminal.into(),
            tab_id: tab.into(),
        };
        for id in [&value.pane_id, &value.terminal_id, &value.tab_id] {
            if id.is_empty() || id.chars().any(char::is_control) {
                return Err(invalid(
                    "native resources require nonempty, control-free identities",
                ));
            }
        }
        Ok(value)
    }
    pub fn pane_id(&self) -> &str {
        &self.pane_id
    }
    pub fn terminal_id(&self) -> &str {
        &self.terminal_id
    }
    pub fn tab_id(&self) -> &str {
        &self.tab_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativePhase {
    Unlaunched,
    Reserved(Resources),
    Ready(Resources),
    Closed(Resources),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeAttempt {
    key: String,
    nonce: String,
    phase: NativePhase,
}

fn validate_key(key: &str) -> Result<()> {
    if key.len() != 64
        || !key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("native key must be a lowercase SHA256 digest"));
    }
    Ok(())
}

impl NativeAttempt {
    pub fn new(key: impl Into<String>, nonce: impl Into<String>) -> Result<Self> {
        let value = Self {
            key: key.into(),
            nonce: nonce.into(),
            phase: NativePhase::Unlaunched,
        };
        validate_key(&value.key)?;
        if value.nonce.is_empty()
            || !value
                .nonce
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(invalid("invalid native attempt nonce"));
        }
        Ok(value)
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
    pub fn phase(&self) -> &NativePhase {
        &self.phase
    }
    pub fn resources(&self) -> Option<&Resources> {
        match &self.phase {
            NativePhase::Unlaunched => None,
            NativePhase::Reserved(r) | NativePhase::Ready(r) | NativePhase::Closed(r) => Some(r),
        }
    }
    pub fn reserve(&mut self, resources: Resources) -> Result<()> {
        if self.phase != NativePhase::Unlaunched {
            return Err(invalid("native attempt already reserved"));
        }
        self.phase = NativePhase::Reserved(resources);
        Ok(())
    }
    pub fn mark_ready(&mut self) -> Result<()> {
        let NativePhase::Reserved(resources) = &self.phase else {
            return Err(invalid("native attempt is not reserved"));
        };
        self.phase = NativePhase::Ready(resources.clone());
        Ok(())
    }
    pub fn mark_closed(&mut self) -> Result<()> {
        let (NativePhase::Reserved(resources) | NativePhase::Ready(resources)) = &self.phase else {
            return Err(invalid("native attempt is not reserved or ready"));
        };
        self.phase = NativePhase::Closed(resources.clone());
        Ok(())
    }
}

#[derive(Clone, Copy, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
enum WirePhase {
    Pending,
    Ready,
    Closed,
}
crate::codec::deserialize_string_enum!(WirePhase);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Wire {
    version: u32,
    key: String,
    nonce: String,
    phase: WirePhase,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_string"
    )]
    pane_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_string"
    )]
    terminal_id: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present_string"
    )]
    tab_id: Option<String>,
}

fn present_string<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(d).map(Some)
}

impl Serialize for NativeAttempt {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        Wire {
            version: 1,
            key: self.key.clone(),
            nonce: self.nonce.clone(),
            phase: match self.phase {
                NativePhase::Unlaunched | NativePhase::Reserved(_) => WirePhase::Pending,
                NativePhase::Ready(_) => WirePhase::Ready,
                NativePhase::Closed(_) => WirePhase::Closed,
            },
            pane_id: self.resources().map(|r| r.pane_id.clone()),
            terminal_id: self.resources().map(|r| r.terminal_id.clone()),
            tab_id: self.resources().map(|r| r.tab_id.clone()),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for NativeAttempt {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w: Wire = crate::codec::object(d)?;
        let validated = || -> Result<Self> {
            if w.version != 1 {
                return Err(invalid("unsupported native evidence version"));
            }
            let mut attempt = Self::new(w.key, w.nonce)?;
            match (w.pane_id, w.terminal_id, w.tab_id) {
                (None, None, None) if matches!(w.phase, WirePhase::Pending) => return Ok(attempt),
                (Some(p), Some(t), Some(b)) => attempt.reserve(Resources::new(p, t, b)?)?,
                _ => return Err(invalid("incomplete native resource tuple")),
            }
            match w.phase {
                WirePhase::Pending => {}
                WirePhase::Ready => attempt.mark_ready()?,
                WirePhase::Closed => attempt.mark_closed()?,
            }
            Ok(attempt)
        };
        validated().map_err(serde::de::Error::custom)
    }
}

fn private(meta: &fs::Metadata) -> Result<()> {
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err(invalid("native state must be private and user-owned"));
    }
    Ok(())
}

// Above the managed root, existing directory aliases are trusted. Sync each
// newly created parent entry too, or a durable marker could lose its whole tree.
fn trusted_parent(path: &Path) -> Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("native parent is not a directory"))?;
    trusted_parent(parent)?;
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(error) => return Err(error.into()),
    }
    file_io::sync_directory(path)?;
    file_io::sync_directory(parent)?;
    Ok(())
}

// An injected pause lets tests release a peer at the exact final-wait boundary.
fn wait_for_flock(
    file: &File,
    started: Instant,
    budget: Duration,
    cancelled: &AtomicBool,
    mut pause: impl FnMut(Duration),
) -> Result<()> {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(invalid("native lock cancelled"));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::WouldBlock {
            return Err(error.into());
        }
        let remaining = budget.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(invalid(
                "native workspace lock timed out; never remove or steal it",
            ));
        }
        pause(remaining.min(Duration::from_millis(25)));
    }
}

pub struct NativeState {
    directory: PathBuf,
}

impl NativeState {
    /// Configuration only: ancestors above this root are trusted, including /var aliases.
    pub fn new(directory: impl AsRef<Path>) -> Result<Self> {
        let directory = std::path::absolute(directory)?;
        if directory.file_name().is_none()
            || directory.components().any(|c| c == Component::ParentDir)
        {
            return Err(invalid("native state needs a lexical directory name"));
        }
        Ok(Self { directory })
    }
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    fn path(&self, key: &str) -> Result<PathBuf> {
        validate_key(key)?;
        Ok(self.directory.join(format!("{key}.json")))
    }
    fn check_directory(&self) -> Result<bool> {
        match fs::symlink_metadata(&self.directory) {
            Ok(meta) if meta.is_dir() => {
                private(&meta)?;
                Ok(true)
            }
            Ok(_) => Err(invalid("native state root must be a real directory")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
    fn prepare(&self) -> Result<()> {
        if !self.check_directory()? {
            let parent = self.directory.parent().expect("state parent");
            // Existing ancestors are trusted; do not reject OS-managed alias directories.
            if !parent.is_dir() {
                trusted_parent(parent)?;
            }
            match fs::DirBuilder::new().mode(0o700).create(&self.directory) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
            self.check_directory()?;
            file_io::sync_directory(&self.directory)?;
            file_io::sync_directory(parent)?;
        }
        Ok(())
    }
    pub fn read(&self, key: &str) -> Result<Option<NativeAttempt>> {
        let path = self.path(key)?;
        if !self.check_directory()? {
            return Ok(None);
        }
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => {
                return Ok(None);
            }
            Err(e) => return Err(e.into()),
        };
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(invalid("native evidence must be a regular file"));
        }
        private(&meta)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let value: NativeAttempt = file_io::decode_json(&bytes)?;
        if value.key() != key {
            return Err(invalid("native evidence key mismatch"));
        }
        Ok(Some(value))
    }
    /// Timeout is seconds; zero performs one nonblocking attempt. Never create the socket directory.
    pub fn lock(
        &self,
        socket: &Path,
        workspace: &str,
        timeout: f64,
        cancelled: &AtomicBool,
    ) -> Result<NativeStateGuard<'_>> {
        super::text(workspace)?;
        let socket_text = socket
            .to_str()
            .filter(|_| socket.is_absolute() && socket.file_name().is_some())
            .ok_or_else(|| invalid("native socket must be an absolute UTF-8 file path"))?;
        let budget = Duration::try_from_secs_f64(timeout)
            .map_err(|_| invalid("invalid native lock timeout"))?;
        let started = Instant::now();
        let check_cancelled = || {
            if cancelled.load(Ordering::Acquire) {
                Err(invalid("native lock cancelled"))
            } else {
                Ok(())
            }
        };
        check_cancelled()?;
        let path = socket.parent().expect("socket parent").join(format!(
            ".work-cards-{}.lock",
            digest(format!("{socket_text}\0{workspace}").as_bytes())
        ));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let identity = file.metadata()?;
        if !identity.is_file() {
            return Err(invalid("native lock must be a regular file"));
        }
        private(&identity)?;
        wait_for_flock(&file, started, budget, cancelled, std::thread::sleep)?;
        check_cancelled()?;
        let current = fs::symlink_metadata(&path)?;
        if !current.is_file() || current.dev() != identity.dev() || current.ino() != identity.ino()
        {
            return Err(invalid(
                "native lock inode changed; inspect without removing it",
            ));
        }
        check_cancelled()?;
        Ok(NativeStateGuard {
            state: self,
            _lock: file,
        })
    }
}

/// The retained descriptor owns flock until Drop closes it; never unlink the lock file.
pub struct NativeStateGuard<'a> {
    state: &'a NativeState,
    _lock: File,
}

impl NativeStateGuard<'_> {
    pub fn directory(&self) -> &Path {
        self.state.directory()
    }
    /// Explicit writer preparation, never invoked by read-only inspection.
    pub fn prepare_directory(&self) -> Result<&Path> {
        self.state.prepare()?;
        Ok(self.directory())
    }
    pub fn read(&self, key: &str) -> Result<Option<NativeAttempt>> {
        self.state.read(key)
    }
    pub fn write(
        &self,
        expected: Option<&NativeAttempt>,
        replacement: &NativeAttempt,
    ) -> Result<()> {
        if expected.is_some_and(|e| e.key != replacement.key)
            || self.read(replacement.key())?.as_ref() != expected
        {
            return Err(invalid("native launch evidence changed; write refused"));
        }
        if expected.is_none() && replacement.phase != NativePhase::Unlaunched {
            return Err(invalid(
                "native evidence must begin with a durable pending marker",
            ));
        }
        if let Some(old) = expected {
            let forward = old == replacement
                || (old.nonce == replacement.nonce
                    && match (&old.phase, &replacement.phase) {
                        (NativePhase::Unlaunched, NativePhase::Reserved(_)) => true,
                        (
                            NativePhase::Reserved(a),
                            NativePhase::Ready(b) | NativePhase::Closed(b),
                        )
                        | (NativePhase::Ready(a), NativePhase::Closed(b)) => a == b,
                        _ => false,
                    })
                || (matches!(old.phase, NativePhase::Ready(_) | NativePhase::Closed(_))
                    && old.nonce != replacement.nonce
                    && replacement.phase == NativePhase::Unlaunched);
            if !forward {
                return Err(invalid(
                    "native evidence cannot be rolled back or replaced while pending",
                ));
            }
        }
        self.persist(replacement)
    }
    fn persist(&self, value: &NativeAttempt) -> Result<()> {
        self.state.prepare()?;
        file_io::write_json(&self.state.path(value.key())?, value)?;
        Ok(())
    }
    /// Caller must establish that NO launch occurred. Reserved evidence is never rollback proof.
    pub fn discard_unlaunched(
        &self,
        expected: &NativeAttempt,
        previous: Option<&NativeAttempt>,
    ) -> Result<()> {
        if expected.phase != NativePhase::Unlaunched
            || previous.is_some_and(|p| {
                p.key != expected.key
                    || matches!(p.phase, NativePhase::Unlaunched | NativePhase::Reserved(_))
            })
            || self.read(expected.key())?.as_ref() != Some(expected)
        {
            return Err(invalid(
                "native reservation changed; unlaunched rollback refused",
            ));
        }
        if let Some(previous) = previous {
            return self.persist(previous);
        }
        fs::remove_file(self.state.path(expected.key())?)?;
        file_io::sync_directory(&self.state.directory)
            .map_err(crate::storage::StorageError::after_write)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn key() -> String {
        "ab".repeat(32)
    }
    fn attempt(nonce: &str) -> NativeAttempt {
        NativeAttempt::new(key(), nonce).unwrap()
    }
    fn resources() -> Resources {
        Resources::new("pane", "terminal", "tab").unwrap()
    }
    fn setup() -> (tempfile::TempDir, NativeState, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let state = NativeState::new(temp.path().join("state")).unwrap();
        let socket = temp.path().join("herdr.sock");
        (temp, state, socket)
    }
    fn guard<'a>(state: &'a NativeState, socket: &Path) -> NativeStateGuard<'a> {
        state
            .lock(socket, "workspace", 0.0, &AtomicBool::new(false))
            .unwrap()
    }

    #[test]
    fn exact_ts_wire_and_transitions() {
        let mut value = attempt("Nonce_1-2");
        for phase in ["pending", "pending", "ready", "closed"]
            .into_iter()
            .enumerate()
        {
            match phase.0 {
                1 => value.reserve(resources()).unwrap(),
                2 => value.mark_ready().unwrap(),
                3 => value.mark_closed().unwrap(),
                _ => {}
            }
            let mut wire =
                serde_json::json!({"version":1,"key":key(),"nonce":"Nonce_1-2","phase":phase.1});
            if phase.0 > 0 {
                wire["paneId"] = "pane".into();
                wire["terminalId"] = "terminal".into();
                wire["tabId"] = "tab".into();
            }
            assert_eq!(serde_json::to_value(&value).unwrap(), wire);
            assert_eq!(
                serde_json::from_value::<NativeAttempt>(wire).unwrap(),
                value
            );
        }
        assert!(NativeAttempt::new("A".repeat(64), "n").is_err());
        assert!(NativeAttempt::new(key(), "bad nonce").is_err());
        assert!(Resources::new("", "t", "b").is_err());
        assert!(Resources::new("p", "t\u{85}", "b").is_err());
    }

    #[test]
    fn every_phase_checks_every_transition_without_losing_identity_or_resources() {
        use NativePhase::{Closed, Ready, Reserved, Unlaunched};
        let original = resources();
        let replacement = Resources::new("other-pane", "other-terminal", "other-tab").unwrap();
        let cases = [
            (
                Unlaunched,
                [Some(Reserved(replacement.clone())), None, None],
            ),
            (
                Reserved(original.clone()),
                [
                    None,
                    Some(Ready(original.clone())),
                    Some(Closed(original.clone())),
                ],
            ),
            (
                Ready(original.clone()),
                [None, None, Some(Closed(original.clone()))],
            ),
            (Closed(original), [None, None, None]),
        ];
        for (phase, outcomes) in cases {
            for (operation, expected) in outcomes.into_iter().enumerate() {
                let before = NativeAttempt {
                    phase: phase.clone(),
                    ..attempt("original-nonce")
                };
                let mut after = before.clone();
                let result = match operation {
                    0 => after.reserve(replacement.clone()),
                    1 => after.mark_ready(),
                    2 => after.mark_closed(),
                    _ => unreachable!(),
                };
                assert_eq!(
                    result.is_ok(),
                    expected.is_some(),
                    "{phase:?}, operation {operation}"
                );
                assert_eq!(
                    after,
                    NativeAttempt {
                        phase: expected.unwrap_or(phase.clone()),
                        ..before
                    },
                    "{phase:?}, operation {operation}",
                );
            }
        }
    }

    #[test]
    fn rejects_malformed_schema() {
        let base = serde_json::json!({"version":1,"key":key(),"nonce":"n","phase":"pending"});
        for (field, value) in [
            ("version", serde_json::json!(2)),
            ("phase", "ready".into()),
            ("phase", "closed".into()),
            ("phase", "unknown".into()),
            ("phase", serde_json::json!({"pending": null})),
            ("nonce", "".into()),
            ("paneId", "p".into()),
            ("paneId", serde_json::Value::Null),
            ("extra", true.into()),
        ] {
            let mut wire = base.clone();
            wire[field] = value;
            assert!(serde_json::from_value::<NativeAttempt>(wire).is_err());
        }
        for mask in 1..7 {
            let mut wire = base.clone();
            for (bit, name) in ["paneId", "terminalId", "tabId"].into_iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    wire[name] = "id".into();
                }
            }
            assert!(serde_json::from_value::<NativeAttempt>(wire).is_err());
        }
        let duplicate = format!(
            "{{\"version\":1,\"key\":\"{}\",\"nonce\":\"n\",\"nonce\":\"m\",\"phase\":\"pending\"}}",
            key()
        );
        assert!(serde_json::from_str::<NativeAttempt>(&duplicate).is_err());
        assert!(file_io::decode_json::<NativeAttempt>(duplicate.as_bytes()).is_err());
        assert!(
            serde_json::from_str::<NativeAttempt>(&format!("[1,\"{}\",\"n\",\"pending\"]", key()))
                .is_err()
        );
    }

    #[test]
    fn missing_reads_create_nothing() {
        let (temp, _, _) = setup();
        let state = NativeState::new(temp.path().join("absent/nested/state")).unwrap();
        assert!(state.read(&key()).unwrap().is_none());
        assert!(!temp.path().join("absent").exists());
        assert!(state.read("../bad").is_err());
    }

    #[test]
    fn initial_evidence_requires_a_durable_pending_marker() {
        let (_temp, state, socket) = setup();
        let lock = guard(&state, &socket);
        let mut reserved = attempt("n");
        reserved.reserve(resources()).unwrap();
        assert!(lock.write(None, &reserved).is_err());
        reserved.mark_ready().unwrap();
        assert!(lock.write(None, &reserved).is_err());
        assert!(!state.directory().exists());
    }

    #[test]
    fn cas_and_rollback_preserve_evidence() {
        let (_temp, state, socket) = setup();
        let lock = guard(&state, &socket);
        let first = attempt("first");
        lock.write(None, &first).unwrap();
        assert!(lock.write(None, &first).is_err());
        lock.discard_unlaunched(&first, None).unwrap();
        assert!(lock.read(&key()).unwrap().is_none());
        lock.write(None, &first).unwrap();
        let mut reserved = first.clone();
        reserved.reserve(resources()).unwrap();
        lock.write(Some(&first), &reserved).unwrap();
        assert!(lock.write(Some(&first), &reserved).is_err());
        assert!(lock.discard_unlaunched(&reserved, None).is_err());
        assert!(lock.write(Some(&reserved), &first).is_err());
        let mut closed = reserved.clone();
        closed.mark_closed().unwrap();
        lock.write(Some(&reserved), &closed).unwrap();
        let retry = attempt("retry");
        lock.write(Some(&closed), &retry).unwrap();
        assert!(lock.discard_unlaunched(&retry, Some(&reserved)).is_err());
        let mut other = closed.clone();
        other.key = "cd".repeat(32);
        assert!(lock.discard_unlaunched(&retry, Some(&other)).is_err());
        lock.discard_unlaunched(&retry, Some(&closed)).unwrap();
        assert_eq!(lock.read(&key()).unwrap(), Some(closed));
    }

    #[test]
    fn unsafe_roots_leaves_and_modes_fail_closed() {
        let (temp, state, socket) = setup();
        let lock = guard(&state, &socket);
        let first = attempt("n");
        lock.write(None, &first).unwrap();
        let path = state.path(&key()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(state.read(&key()).is_err());
        assert!(lock.write(Some(&first), &first).is_err());
        fs::remove_file(&path).unwrap();
        symlink(temp.path().join("missing"), &path).unwrap();
        assert!(state.read(&key()).is_err());
        fs::remove_file(&path).unwrap();
        let cpath = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(state.read(&key()).is_err());
        fs::remove_file(path).unwrap();
        fs::set_permissions(state.directory(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(state.read(&key()).is_err());
        fs::set_permissions(state.directory(), fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir(state.directory()).unwrap();
        symlink(temp.path(), state.directory()).unwrap();
        assert!(state.read(&key()).is_err());
        assert!(lock.write(None, &first).is_err());
    }

    #[test]
    fn lock_contention_timeout_cancellation_and_retained_descriptor() {
        let (_temp, state, socket) = setup();
        let held = guard(&state, &socket);
        assert!(
            state
                .lock(&socket, "workspace", 0.0, &AtomicBool::new(false))
                .is_err()
        );
        let started = Instant::now();
        assert!(
            state
                .lock(&socket, "workspace", 0.03, &AtomicBool::new(false))
                .is_err()
        );
        assert!(started.elapsed() >= Duration::from_millis(30));
        let cancelled = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(10));
                cancelled.store(true, Ordering::Release);
            });
            assert!(
                state
                    .lock(&socket, "workspace", 5.0, &cancelled)
                    .err()
                    .expect("cancelled acquisition")
                    .to_string()
                    .contains("cancelled")
            );
        });
        assert!(
            state
                .lock(&socket, "workspace", 0.0, &AtomicBool::new(true))
                .is_err()
        );
        let path = socket.parent().unwrap().join(format!(
            ".work-cards-{}.lock",
            digest(format!("{}\0workspace", socket.to_str().unwrap()).as_bytes())
        ));
        let fd = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert_ne!(
            unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        drop(held);
        assert_eq!(
            unsafe { libc::flock(fd.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        drop(fd);
        assert!(path.exists());
        drop(guard(&state, &socket));
    }

    #[test]
    fn lock_retries_after_the_final_wait() {
        let (_temp, state, socket) = setup();
        let mut held = Some(guard(&state, &socket));
        let path = socket.parent().unwrap().join(format!(
            ".work-cards-{}.lock",
            digest(format!("{}\0workspace", socket.to_str().unwrap()).as_bytes())
        ));
        let contender = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        let budget = Duration::from_millis(100);
        let mut pauses = 0;
        wait_for_flock(
            &contender,
            Instant::now(),
            budget,
            &AtomicBool::new(false),
            |_| {
                pauses += 1;
                drop(held.take());
                std::thread::sleep(budget);
            },
        )
        .unwrap();
        assert_eq!(pauses, 1);
    }

    #[test]
    fn unsafe_lock_and_missing_socket_directory() {
        let (temp, state, socket) = setup();
        let missing = temp.path().join("missing/socket");
        assert!(
            state
                .lock(&missing, "w", 0.0, &AtomicBool::new(false))
                .is_err()
        );
        assert!(!missing.parent().unwrap().exists());
        let path = socket.parent().unwrap().join(format!(
            ".work-cards-{}.lock",
            digest(format!("{}\0workspace", socket.to_str().unwrap()).as_bytes())
        ));
        symlink(temp.path().join("missing-lock"), &path).unwrap();
        assert!(
            state
                .lock(&socket, "workspace", 0.0, &AtomicBool::new(false))
                .is_err()
        );
        fs::remove_file(&path).unwrap();
        let cpath = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(
            state
                .lock(&socket, "workspace", 0.0, &AtomicBool::new(false))
                .is_err()
        );
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(
            state
                .lock(&socket, "workspace", 0.0, &AtomicBool::new(false))
                .is_err()
        );
    }

    #[test]
    fn trusted_ancestor_alias_is_allowed() {
        let (temp, _, socket) = setup();
        let actual = temp.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let alias = temp.path().join("alias");
        symlink(&actual, &alias).unwrap();
        let nested = NativeState::new(alias.join("missing/parent/state")).unwrap();
        assert!(nested.read(&key()).unwrap().is_none());
        assert!(!actual.join("missing").exists());
        let nested_lock = guard(&nested, &socket);
        nested_lock.write(None, &attempt("nested")).unwrap();
        assert_eq!(nested.read(&key()).unwrap(), Some(attempt("nested")));
        drop(nested_lock);
        let state = NativeState::new(alias.join("state")).unwrap();
        assert!(state.read(&key()).unwrap().is_none());
        let lock = guard(&state, &socket);
        lock.write(None, &attempt("n")).unwrap();
        assert_eq!(state.read(&key()).unwrap(), Some(attempt("n")));
        #[cfg(target_os = "macos")]
        if temp.path().starts_with("/private/var") {
            let path = Path::new("/var")
                .join(temp.path().strip_prefix("/private/var").unwrap())
                .join("alias/state");
            assert_eq!(
                NativeState::new(path).unwrap().read(&key()).unwrap(),
                Some(attempt("n"))
            );
        }
    }
}
