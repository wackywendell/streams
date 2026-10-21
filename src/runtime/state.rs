//! Host-local native bindings and durable, non-replayable open journals.

use std::fs::{self, File};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::codec::{ascii_json, required_nullable};
use crate::file_io;
use crate::operations::Authority;

use super::{Result, digest, invalid, lock, read_json, write_json};

fn state_text(value: &str) -> Result<()> {
    // Preserve Python's persisted text contract, including its accepted whitespace.
    if value.trim().is_empty() || value.contains('\0') {
        return Err(invalid(
            "runtime identity requires nonempty text without NUL",
        ));
    }
    Ok(())
}

fn scope_text(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(invalid("runtime scope must be a lowercase SHA256 digest"));
    }
    Ok(())
}

fn key(parts: &[&str]) -> String {
    digest(
        format!(
            "[{}]",
            parts
                .iter()
                .map(|p| ascii_json(p))
                .collect::<Vec<_>>()
                .join(", ")
        )
        .as_bytes(),
    )
}

/// Python-compatible namespace for one pinned Herdr endpoint.
pub fn herdr_scope(socket: &Path, session: Option<&str>) -> String {
    digest(
        format!(
            "[\"herdr\", {}, {}]",
            ascii_json(&socket.to_string_lossy()),
            session.map(ascii_json).unwrap_or_else(|| "null".into()),
        )
        .as_bytes(),
    )
}

fn version(value: u32) -> Result<()> {
    if value != 1 {
        return Err(invalid("unsupported runtime state version"));
    }
    Ok(())
}

fn filename(path: &Path, identity: &str) -> Result<()> {
    let expected = format!("{}.json", key(&[identity]));
    if path.file_name().and_then(|s| s.to_str()) != Some(expected.as_str()) {
        return Err(invalid(
            "runtime state filename does not match record identity",
        ));
    }
    Ok(())
}

/// Session identity and its exact launch command cannot be independently present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchReference {
    session_id: String,
    command: String,
}

impl LaunchReference {
    pub fn new(session_id: impl Into<String>, command: impl Into<String>) -> Result<Self> {
        let value = Self {
            session_id: session_id.into(),
            command: command.into(),
        };
        state_text(&value.session_id)?;
        state_text(&value.command)?;
        Ok(value)
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }
    pub fn command(&self) -> &str {
        &self.command
    }
}

fn launch(session_id: Option<String>, command: Option<String>) -> Result<Option<LaunchReference>> {
    match (session_id, command) {
        (None, None) => Ok(None),
        (Some(session), Some(command)) => LaunchReference::new(session, command).map(Some),
        _ => Err(invalid("session_id and command must be paired")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Association {
    stream_id: String,
    workspace_id: String,
    launch: Option<LaunchReference>,
    ready: bool,
}

impl Association {
    pub fn new(
        stream_id: impl Into<String>,
        workspace_id: impl Into<String>,
        launch: Option<LaunchReference>,
        ready: bool,
    ) -> Result<Self> {
        let value = Self {
            stream_id: stream_id.into(),
            workspace_id: workspace_id.into(),
            launch,
            ready,
        };
        state_text(&value.stream_id)?;
        state_text(&value.workspace_id)?;
        Ok(value)
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
    pub fn launch(&self) -> Option<&LaunchReference> {
        self.launch.as_ref()
    }
    pub fn ready(&self) -> bool {
        self.ready
    }

    fn wire(&self) -> AssociationWire {
        AssociationWire {
            version: 1,
            stream_id: self.stream_id.clone(),
            workspace_id: self.workspace_id.clone(),
            session_id: self.launch.as_ref().map(|l| l.session_id.clone()),
            command: self.launch.as_ref().map(|l| l.command.clone()),
            ready: self.ready,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AssociationWire {
    version: u32,
    stream_id: String,
    workspace_id: String,
    #[serde(deserialize_with = "required_nullable")]
    session_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    command: Option<String>,
    ready: bool,
}

impl AssociationWire {
    fn validated(self, path: &Path) -> Result<Association> {
        version(self.version)?;
        filename(path, &self.workspace_id)?;
        Association::new(
            self.stream_id,
            self.workspace_id,
            launch(self.session_id, self.command)?,
            self.ready,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrollmentClaim {
    stream_id: String,
    workspace_id: String,
}

impl EnrollmentClaim {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimWire {
    version: u32,
    stream_id: String,
    workspace_id: String,
}

impl ClaimWire {
    fn validated(self, path: &Path) -> Result<EnrollmentClaim> {
        version(self.version)?;
        state_text(&self.stream_id)?;
        state_text(&self.workspace_id)?;
        filename(path, &self.workspace_id)?;
        Ok(EnrollmentClaim {
            stream_id: self.stream_id,
            workspace_id: self.workspace_id,
        })
    }
}

/// Persisted diagnostic evidence, deliberately not convertible to an active attempt guard.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingAttempt {
    stream_id: String,
    launch: Option<LaunchReference>,
    cwd: String,
    workspace_id: Option<String>,
    effects: Vec<Value>,
    launches: Vec<Value>,
}

impl PendingAttempt {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }
    pub fn launch(&self) -> Option<&LaunchReference> {
        self.launch.as_ref()
    }
    pub fn cwd(&self) -> &str {
        &self.cwd
    }
    pub fn workspace_id(&self) -> Option<&str> {
        self.workspace_id.as_deref()
    }
    pub fn effects(&self) -> &[Value] {
        &self.effects
    }
    pub fn launches(&self) -> &[Value] {
        &self.launches
    }

    fn wire(&self) -> AttemptWire {
        AttemptWire {
            version: 1,
            stream_id: self.stream_id.clone(),
            session_id: self.launch.as_ref().map(|l| l.session_id.clone()),
            command: self.launch.as_ref().map(|l| l.command.clone()),
            cwd: self.cwd.clone(),
            workspace_id: self.workspace_id.clone(),
            effects: self.effects.clone(),
            launches: self.launches.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AttemptWire {
    version: u32,
    stream_id: String,
    #[serde(deserialize_with = "required_nullable")]
    session_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    command: Option<String>,
    cwd: String,
    #[serde(deserialize_with = "required_nullable")]
    workspace_id: Option<String>,
    effects: Vec<Value>,
    launches: Vec<Value>,
}

impl AttemptWire {
    fn validated(self, path: &Path) -> Result<PendingAttempt> {
        version(self.version)?;
        state_text(&self.stream_id)?;
        state_text(&self.cwd)?;
        if let Some(workspace) = &self.workspace_id {
            state_text(workspace)?;
        }
        filename(path, &self.stream_id)?;
        Ok(PendingAttempt {
            stream_id: self.stream_id,
            launch: launch(self.session_id, self.command)?,
            cwd: self.cwd,
            workspace_id: self.workspace_id,
            effects: self.effects,
            launches: self.launches,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scoped<T> {
    scope: String,
    record: T,
}

impl<T> Scoped<T> {
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn record(&self) -> &T {
        &self.record
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeState {
    root: PathBuf,
    host_id: String,
}

impl RuntimeState {
    /// `catalog_runtime_base` is the Python runtime base, not an already host-keyed root.
    pub fn new(catalog_runtime_base: impl AsRef<Path>, host_id: impl Into<String>) -> Result<Self> {
        let host_id = host_id.into();
        state_text(&host_id)?;
        let base = std::path::absolute(catalog_runtime_base.as_ref())?;
        // Resolve only the trusted prefix. Resolving the base leaf would hide a forbidden link.
        let base = if let Some(parent) = base.parent() {
            crate::config::resolve_path(parent)
                .map_err(|error| invalid(error.to_string()))?
                .join(
                    base.components()
                        .next_back()
                        .expect("absolute base component"),
                )
        } else {
            base
        };
        Ok(Self {
            root: base.join(key(&[&host_id])),
            host_id,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    fn path(&self, collection: &str, scope: &str, identity: &str) -> Result<PathBuf> {
        scope_text(scope)?;
        state_text(identity)?;
        let path = self
            .root
            .join(collection)
            .join(scope)
            .join(format!("{}.json", key(&[identity])));
        self.check_directories(path.parent().expect("record parent"))?;
        Ok(path)
    }

    fn check_directories(&self, directory: &Path) -> Result<()> {
        // The configured base is the trust boundary, not OS-managed ancestors such as /var.
        let base = self.root.parent().expect("runtime base");
        let relative = directory
            .strip_prefix(base)
            .map_err(|_| invalid("runtime directory is outside configured base"))?;
        file_io::ensure_directory(base, false)?;
        let mut prefix = base.to_path_buf();
        for component in relative.components() {
            prefix.push(component);
            file_io::ensure_directory(&prefix, false)?;
        }
        Ok(())
    }

    fn writable(&self, path: &Path) -> Result<()> {
        self.check_directories(path.parent().expect("state parent"))?;
        match fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_file() => Err(invalid("state destination must be a regular file")),
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn association(&self, scope: &str, workspace: &str) -> Result<Option<Association>> {
        let path = self.path("associations", scope, workspace)?;
        self.read_association(&path)
    }

    fn read_association(&self, path: &Path) -> Result<Option<Association>> {
        read_json::<AssociationWire>(path)?
            .map(|w| w.validated(path))
            .transpose()
    }

    pub fn enrollment_claim(
        &self,
        scope: &str,
        workspace: &str,
    ) -> Result<Option<EnrollmentClaim>> {
        let path = self.path("enrollments", scope, workspace)?;
        self.read_claim(&path)
    }

    fn read_claim(&self, path: &Path) -> Result<Option<EnrollmentClaim>> {
        read_json::<ClaimWire>(path)?
            .map(|w| w.validated(path))
            .transpose()
    }

    pub fn pending_attempt(&self, scope: &str, stream: &str) -> Result<Option<PendingAttempt>> {
        let path = self.path("attempts", scope, stream)?;
        self.read_attempt(&path)
    }

    fn read_attempt(&self, path: &Path) -> Result<Option<PendingAttempt>> {
        read_json::<AttemptWire>(path)?
            .map(|w| w.validated(path))
            .transpose()
    }

    fn entries(&self, collection: &str, selected: Option<&str>) -> Result<Vec<(String, PathBuf)>> {
        if let Some(scope) = selected {
            scope_text(scope)?;
        }
        let base = self.root.join(collection);
        self.check_directories(&base)?;
        if !base.exists() {
            return Ok(Vec::new());
        }
        let mut scopes = Vec::new();
        if let Some(scope) = selected {
            scopes.push(scope.to_owned());
        } else {
            for entry in fs::read_dir(&base)? {
                let name = entry?
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("non-UTF8 runtime scope"))?;
                scope_text(&name)?;
                scopes.push(name);
            }
        }
        scopes.sort();
        let mut paths = Vec::new();
        for scope in scopes {
            let directory = base.join(&scope);
            self.check_directories(&directory)?;
            if !directory.exists() {
                continue;
            }
            for entry in fs::read_dir(directory)? {
                let path = entry?.path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    paths.push((scope.clone(), path));
                }
            }
        }
        paths.sort();
        Ok(paths)
    }

    pub fn associations(&self, scope: Option<&str>) -> Result<Vec<Scoped<Association>>> {
        self.entries("associations", scope)?
            .into_iter()
            .map(|(scope, path)| {
                let record = self
                    .read_association(&path)?
                    .ok_or_else(|| invalid("association disappeared during inventory"))?;
                Ok(Scoped { scope, record })
            })
            .collect()
    }

    pub fn enrollment_claims(&self, scope: Option<&str>) -> Result<Vec<Scoped<EnrollmentClaim>>> {
        self.entries("enrollments", scope)?
            .into_iter()
            .map(|(scope, path)| {
                let record = self
                    .read_claim(&path)?
                    .ok_or_else(|| invalid("claim disappeared during inventory"))?;
                Ok(Scoped { scope, record })
            })
            .collect()
    }

    pub fn pending_attempts(&self, scope: Option<&str>) -> Result<Vec<Scoped<PendingAttempt>>> {
        self.entries("attempts", scope)?
            .into_iter()
            .map(|(scope, path)| {
                let record = self
                    .read_attempt(&path)?
                    .ok_or_else(|| invalid("attempt disappeared during inventory"))?;
                Ok(Scoped { scope, record })
            })
            .collect()
    }

    fn check_workspace_available(
        &self,
        scope: &str,
        workspace: &str,
        stream: &str,
        active: Option<&PendingAttempt>,
    ) -> Result<()> {
        if self
            .association(scope, workspace)?
            .is_some_and(|b| b.stream_id != stream)
            || self
                .enrollment_claim(scope, workspace)?
                .is_some_and(|c| c.stream_id != stream)
        {
            return Err(invalid(
                "workspace is already bound or claimed by another stream",
            ));
        }
        for pending in self.pending_attempts(Some(scope))? {
            if pending.record.workspace_id.as_deref() == Some(workspace)
                && !(pending.record.stream_id == stream && active == Some(&pending.record))
            {
                return Err(invalid("workspace has a pending open attempt"));
            }
        }
        Ok(())
    }

    /// Scopes select records, never the serialization boundary. Pending attempts always block.
    pub fn stream_lock<'a>(&'a self, scope: Option<&str>, stream: &str) -> Result<StreamGuard<'a>> {
        if let Some(scope) = scope {
            scope_text(scope)?;
        }
        state_text(stream)?;
        let path = self
            .root
            .join("stream-locks")
            .join(format!("{}.lock", key(&[stream])));
        self.writable(&path)?;
        let file = lock(&path)?;
        self.check_stream_scopes(scope, stream)?;
        Ok(StreamGuard {
            state: self,
            scope: scope.map(str::to_owned),
            stream_id: stream.to_owned(),
            _lock: file,
        })
    }

    fn check_stream_scopes(&self, scope: Option<&str>, stream: &str) -> Result<()> {
        for binding in self.associations(None)? {
            if binding.record.stream_id == stream && Some(binding.scope.as_str()) != scope {
                return Err(invalid("stream has an association in another scope"));
            }
        }
        for claim in self.enrollment_claims(None)? {
            if claim.record.stream_id == stream && Some(claim.scope.as_str()) != scope {
                return Err(invalid("stream has an enrollment claim in another scope"));
            }
        }
        if self
            .pending_attempts(None)?
            .iter()
            .any(|p| p.record.stream_id == stream)
        {
            return Err(invalid(
                "pending open attempt; inspect state; no automatic retry",
            ));
        }
        Ok(())
    }

    pub fn conversation_lock<'a>(
        &'a self,
        harness: &str,
        native_id: &str,
    ) -> Result<ConversationGuard<'a>> {
        state_text(harness)?;
        state_text(native_id)?;
        let path = self
            .root
            .join("agent-locks")
            .join(format!("{}.lock", key(&[harness, native_id])));
        self.writable(&path)?;
        Ok(ConversationGuard {
            _state: self,
            _lock: lock(&path)?,
        })
    }

    /// `None` means no native workspace; explicit identity still observes global exclusions.
    pub fn enrollment<'a>(
        &'a self,
        context: Option<(&str, &str)>,
        explicit_stream: Option<&str>,
    ) -> Result<EnrollmentGuard<'a>> {
        if let Some(stream) = explicit_stream {
            state_text(stream)?;
        }
        let (workspace, binding, claim) = if let Some((scope, workspace)) = context {
            let path = self
                .path("enrollments", scope, workspace)?
                .with_extension("lock");
            self.writable(&path)?;
            let file = lock(&path)?;
            if self
                .pending_attempts(Some(scope))?
                .iter()
                .any(|attempt| attempt.record.workspace_id.as_deref() == Some(workspace))
            {
                return Err(invalid("workspace has a pending open attempt"));
            }
            (
                Some(LockedWorkspace {
                    scope: scope.to_owned(),
                    workspace_id: workspace.to_owned(),
                    _lock: file,
                }),
                self.association(scope, workspace)?,
                self.enrollment_claim(scope, workspace)?,
            )
        } else {
            (None, None, None)
        };
        if binding.as_ref().is_some_and(|b| !b.ready) {
            return Err(invalid("partial association blocks enrollment"));
        }
        let mut stream_id = explicit_stream.map(str::to_owned);
        for identity in binding
            .as_ref()
            .map(|b| b.stream_id.as_str())
            .into_iter()
            .chain(claim.as_ref().map(|c| c.stream_id.as_str()))
        {
            if stream_id
                .as_deref()
                .is_some_and(|current| current != identity)
            {
                return Err(invalid(
                    "explicit identity, association and enrollment claim disagree",
                ));
            }
            stream_id = Some(identity.to_owned());
        }
        Ok(EnrollmentGuard {
            state: self,
            workspace,
            stream_id,
        })
    }
}

pub struct ConversationGuard<'a> {
    _state: &'a RuntimeState,
    _lock: File,
}

pub struct StreamGuard<'a> {
    state: &'a RuntimeState,
    scope: Option<String>,
    stream_id: String,
    _lock: File,
}

impl<'s> StreamGuard<'s> {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }
    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    pub fn association(&self) -> Result<Option<Association>> {
        let Some(scope) = self.scope() else {
            return Ok(None);
        };
        let bindings: Vec<_> = self
            .state
            .associations(Some(scope))?
            .into_iter()
            .filter(|b| b.record.stream_id == self.stream_id)
            .collect();
        if bindings.len() > 1 {
            return Err(invalid("multiple workspace associations for stream"));
        }
        Ok(bindings.into_iter().next().map(|b| b.record))
    }

    /// Check enrollment completeness before orchestrating open/resume, without native probing.
    pub fn check_open(&self) -> Result<Option<Association>> {
        self.state
            .check_stream_scopes(self.scope(), &self.stream_id)?;
        let binding = self.association()?;
        if binding.as_ref().is_some_and(|b| !b.ready) {
            return Err(invalid("partial association blocks open"));
        }
        if let Some(scope) = self.scope() {
            if let Some(binding) = &binding {
                self.state.check_workspace_available(
                    scope,
                    &binding.workspace_id,
                    &self.stream_id,
                    None,
                )?;
            }
            for claim in self.state.enrollment_claims(Some(scope))? {
                if claim.record.stream_id == self.stream_id
                    && !binding
                        .as_ref()
                        .is_some_and(|b| b.workspace_id == claim.record.workspace_id && b.ready)
                {
                    return Err(invalid("unmatched enrollment claim blocks open"));
                }
            }
        }
        Ok(binding)
    }

    pub fn write_association(&mut self, binding: &Association) -> Result<()> {
        self.write_association_available(binding, None)
    }

    fn write_association_available(
        &mut self,
        binding: &Association,
        active: Option<&PendingAttempt>,
    ) -> Result<()> {
        let scope = self
            .scope()
            .ok_or_else(|| invalid("association requires native scope"))?;
        if binding.stream_id != self.stream_id {
            return Err(invalid("association stream differs from locked identity"));
        }
        if self
            .association()?
            .is_some_and(|b| b.workspace_id != binding.workspace_id)
        {
            return Err(invalid("stream is already bound to another workspace"));
        }
        self.state.check_workspace_available(
            scope,
            &binding.workspace_id,
            &self.stream_id,
            active,
        )?;
        for claim in self.state.enrollment_claims(Some(scope))? {
            if claim.record.stream_id == self.stream_id
                && claim.record.workspace_id != binding.workspace_id
            {
                return Err(invalid("stream is claimed by another workspace"));
            }
        }
        let path = self
            .state
            .path("associations", scope, &binding.workspace_id)?;
        self.state.writable(&path)?;
        write_json(&path, &binding.wire())
    }

    /// Only a fresh journal can yield this guard. Dropping it retains evidence and blocks retries.
    pub fn begin_attempt<'g>(
        &'g mut self,
        launch: Option<LaunchReference>,
        cwd: &str,
        workspace: Option<&str>,
    ) -> Result<AttemptGuard<'g, 's>> {
        self.check_open()?;
        state_text(cwd)?;
        if let Some(workspace) = workspace {
            state_text(workspace)?;
        }
        let scope = self
            .scope()
            .ok_or_else(|| invalid("open attempt requires native scope"))?;
        let binding = self.association()?;
        if binding.as_ref().map(|b| b.workspace_id.as_str()) != workspace {
            return Err(invalid(
                "attempt workspace differs from existing association",
            ));
        }
        let scope = scope.to_owned();
        let path = self.state.path("attempts", &scope, &self.stream_id)?;
        if self.state.read_attempt(&path)?.is_some() {
            return Err(invalid("pending attempt cannot be resumed"));
        }
        let record = PendingAttempt {
            stream_id: self.stream_id.clone(),
            launch,
            cwd: cwd.to_owned(),
            workspace_id: workspace.map(str::to_owned),
            effects: Vec::new(),
            launches: Vec::new(),
        };
        self.state.writable(&path)?;
        write_json(&path, &record.wire())?;
        Ok(AttemptGuard {
            stream: self,
            scope,
            record,
        })
    }
}

struct LockedWorkspace {
    scope: String,
    workspace_id: String,
    _lock: File,
}

pub struct EnrollmentGuard<'a> {
    state: &'a RuntimeState,
    workspace: Option<LockedWorkspace>,
    stream_id: Option<String>,
}

impl<'s> EnrollmentGuard<'s> {
    pub fn stream_id(&self) -> Option<&str> {
        self.stream_id.as_deref()
    }

    pub fn claim(&mut self, stream: &str) -> Result<()> {
        self.claim_locked(stream).map(drop)
    }

    fn claim_locked(&mut self, stream: &str) -> Result<StreamGuard<'s>> {
        state_text(stream)?;
        if self.stream_id.as_deref().is_some_and(|id| id != stream) {
            return Err(invalid("enrollment identity cannot be replaced"));
        }
        let guard = self
            .state
            .stream_lock(self.workspace.as_ref().map(|w| w.scope.as_str()), stream)?;
        if let Some(locked) = &self.workspace {
            let scope = &locked.scope;
            let workspace = &locked.workspace_id;
            for binding in self.state.associations(Some(scope))? {
                if binding.record.stream_id == stream && binding.record.workspace_id != *workspace {
                    return Err(invalid("stream is associated with another workspace"));
                }
            }
            for claim in self.state.enrollment_claims(Some(scope))? {
                if claim.record.stream_id == stream && claim.record.workspace_id != *workspace {
                    return Err(invalid("stream is claimed by another workspace"));
                }
            }
            self.state
                .check_workspace_available(scope, workspace, stream, None)?;
            let binding = self.state.association(scope, workspace)?;
            let claim = self.state.enrollment_claim(scope, workspace)?;
            if binding.as_ref().is_some_and(|b| !b.ready) {
                return Err(invalid("partial association blocks enrollment"));
            }
            // An open-created binding is not enrollment evidence. Never mint a bypass claim.
            if binding.is_none() || claim.is_some() {
                let path = self.state.path("enrollments", scope, workspace)?;
                self.state.writable(&path)?;
                write_json(
                    &path,
                    &ClaimWire {
                        version: 1,
                        stream_id: stream.to_owned(),
                        workspace_id: workspace.clone(),
                    },
                )?;
            }
        }
        self.stream_id = Some(stream.to_owned());
        Ok(guard)
    }

    /// Authority must be effective local coordinator evidence supplied by the caller.
    /// This never launches, focuses, renames, or rolls a surviving claim back.
    pub fn bind(&mut self, stream: &str, authority: &Authority) -> Result<()> {
        if authority.host_id != self.state.host_id || state_text(&authority.token).is_err() {
            return Err(invalid("effective local owner authority is required"));
        }
        let mut guard = self.claim_locked(stream)?;
        if let Some(locked) = &self.workspace
            && self
                .state
                .association(&locked.scope, &locked.workspace_id)?
                .is_none()
        {
            guard.write_association(&Association::new(
                stream,
                &locked.workspace_id,
                None,
                true,
            )?)?;
        }
        Ok(())
    }
}

/// A current-process journal capability, borrowed from the still-held global stream lock.
pub struct AttemptGuard<'g, 's> {
    stream: &'g mut StreamGuard<'s>,
    scope: String,
    record: PendingAttempt,
}

impl AttemptGuard<'_, '_> {
    pub fn record(&self) -> &PendingAttempt {
        &self.record
    }

    /// Evidence is opaque: the orchestrator owns native effect semantics, not this journal.
    pub fn checkpoint(
        &mut self,
        workspace: Option<&str>,
        effects: Vec<Value>,
        launches: Vec<Value>,
    ) -> Result<()> {
        if let Some(workspace) = workspace {
            state_text(workspace)?;
        }
        if self
            .record
            .workspace_id
            .as_deref()
            .is_some_and(|old| Some(old) != workspace)
        {
            return Err(invalid("attempt workspace identity cannot be replaced"));
        }
        if let Some(workspace) = workspace {
            self.stream.state.check_workspace_available(
                &self.scope,
                workspace,
                &self.record.stream_id,
                Some(&self.record),
            )?;
        }
        let path = self
            .stream
            .state
            .path("attempts", &self.scope, &self.record.stream_id)?;
        let mut next = self.record.clone();
        next.workspace_id = workspace.map(str::to_owned);
        next.effects = effects;
        next.launches = launches;
        self.stream.state.writable(&path)?;
        write_json(&path, &next.wire())?;
        self.record = next;
        Ok(())
    }

    pub fn write_association(&mut self, binding: &Association) -> Result<()> {
        if self.record.workspace_id.as_deref() != Some(binding.workspace_id()) {
            return Err(invalid("association differs from attempt workspace"));
        }
        self.stream
            .write_association_available(binding, Some(&self.record))
    }

    /// Clear only this fresh attempt after its association is ready. No loaded retry API exists.
    pub fn complete(self) -> Result<()> {
        let binding = self
            .stream
            .association()?
            .ok_or_else(|| invalid("attempt has no association"))?;
        if !binding.ready
            || Some(binding.workspace_id.as_str()) != self.record.workspace_id.as_deref()
        {
            return Err(invalid("attempt association is not ready"));
        }
        let path = self
            .stream
            .state
            .path("attempts", &self.scope, &self.record.stream_id)?;
        // Re-read strictly before unlinking, then durably commit journal retirement.
        let persisted = self
            .stream
            .state
            .read_attempt(&path)?
            .ok_or_else(|| invalid("attempt disappeared"))?;
        if persisted != self.record {
            return Err(invalid("attempt changed outside its guard"));
        }
        fs::remove_file(&path)?;
        file_io::sync_directory(path.parent().expect("attempt parent"))
            .map_err(crate::storage::StorageError::after_write)?;
        Ok(())
    }
}
