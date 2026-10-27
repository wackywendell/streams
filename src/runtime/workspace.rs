//! Named workspace operations. Native effects never resume a loaded receipt.

mod open;
mod resume;
#[cfg(test)]
mod tests;

pub struct SelectedConversation<'a> {
    pub session: &'a Session,
    pub locator: Option<&'a Path>,
}
pub use resume::ResumeCommand;

use super::{
    Result, checkouts::canonical_directory, conversations::ConversationRoots, invalid,
    launch::SessionCommand, locations::DirectoryBindings, native::BuiltinFrontend,
    native_markers::stream_tokens,
};
use crate::{
    frontend::{
        CreateWorkspace, FrontendId, WorkspaceFrontend, WorkspaceRef,
        herdr::{HerdrAgent, HerdrNativeSnapshot},
    },
    model::{Session, Stream},
};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Serialize)]
pub struct SkippedSession {
    pub session_id: String,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct OpenResult {
    pub stream_id: String,
    pub session_id: Option<String>,
    pub frontend: FrontendId,
    pub workspace_id: String,
    pub reused: bool,
    pub restored: Vec<String>,
    pub skipped: Vec<SkippedSession>,
    pub unavailable: Vec<SkippedSession>,
    pub needs_attention: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrangement_support: Option<&'static str>,
}

pub struct WorkspaceRuntime<'a> {
    host_id: &'a str,
    locations: &'a DirectoryBindings,
    frontend: Option<&'a BuiltinFrontend>,
    catalog_source: &'a str,
    roots: &'a ConversationRoots,
    environment: &'a BTreeMap<String, String>,
}

impl<'a> WorkspaceRuntime<'a> {
    pub fn new(
        host_id: &'a str,
        locations: &'a DirectoryBindings,
        frontend: Option<&'a BuiltinFrontend>,
        catalog_source: &'a str,
        roots: &'a ConversationRoots,
        environment: &'a BTreeMap<String, String>,
    ) -> Self {
        Self {
            host_id,
            locations,
            frontend,
            catalog_source,
            roots,
            environment,
        }
    }

    fn check_stream(&self, stream: &Stream) -> Result<()> {
        stream
            .validate()
            .map_err(|error| invalid(error.to_string()))?;
        if stream.owner_host_id != self.host_id {
            return Err(invalid(
                "native workspace requires the Stream's fixed local writer",
            ));
        }
        super::text(self.catalog_source)?;
        Ok(())
    }
    fn check_session(&self, stream: &Stream, session: &Session) -> Result<()> {
        session
            .validate_portable()
            .map_err(|error| invalid(error.to_string()))?;
        if session.host_id != self.host_id || session.stream_id != stream.id {
            return Err(invalid(
                "selected Session is not current on this local Stream",
            ));
        }
        Ok(())
    }

    fn frontend(&self) -> Result<&BuiltinFrontend> {
        self.frontend.ok_or_else(|| invalid("no native frontend selected; pass --frontend herdr or --frontend cmux (or use resume)"))
    }
    fn directory(
        &self,
        stream: &Stream,
        session: Option<&Session>,
        explicit: Option<&Path>,
    ) -> Result<PathBuf> {
        if let Some(path) = explicit {
            return canonical_directory(path);
        }
        let selected = session
            .map(|session| self.locations.read(&stream.id, Some(&session.id)))
            .transpose()?
            .flatten();
        let saved = match selected {
            Some(saved) => Some(saved),
            None => self.locations.read(&stream.id, None)?,
        };
        saved.map(|binding| binding.cwd).ok_or_else(|| {
            invalid(format!(
                "no local directory for stream {:?}; pass --cwd PATH",
                stream.id
            ))
        })
    }
    fn remember(
        &self,
        stream: &Stream,
        session: Option<&Session>,
        cwd: &Path,
        if_missing: bool,
    ) -> Result<()> {
        self.locations.remember(
            &stream.id,
            cwd,
            session.map(|session| session.id.as_str()),
            if_missing,
            None,
            None,
        )?;
        Ok(())
    }
    fn reference(&self, workspace: &str) -> Result<WorkspaceRef> {
        let adapter = self.frontend()?.adapter();
        Ok(WorkspaceRef {
            frontend: adapter.frontend_id(),
            endpoint: adapter.endpoint_id(),
            native_id: workspace.into(),
        })
    }
    fn result(
        &self,
        stream: &Stream,
        workspace: &str,
        session: Option<&str>,
        reused: bool,
    ) -> Result<OpenResult> {
        Ok(OpenResult {
            stream_id: stream.id.clone(),
            session_id: session.map(str::to_owned),
            frontend: self.frontend()?.adapter().frontend_id(),
            workspace_id: workspace.into(),
            reused,
            restored: vec![],
            skipped: vec![],
            unavailable: vec![],
            needs_attention: vec![],
            arrangement_support: None,
        })
    }
}

fn running<'a>(
    snapshot: &'a HerdrNativeSnapshot,
    selected: &SelectedConversation<'_>,
) -> Vec<&'a HerdrAgent> {
    snapshot
        .topology()
        .agents
        .iter()
        .filter(|agent| {
            let metadata = &agent.native["agent_session"];
            agent.native["agent"] == selected.session.harness
                && metadata["agent"] == selected.session.harness
                && match metadata["kind"].as_str() {
                    Some("id") => metadata["value"] == selected.session.harness_session_id,
                    Some("path") => selected
                        .locator
                        .and_then(Path::to_str)
                        .is_some_and(|path| metadata["value"] == path),
                    _ => false,
                }
        })
        .collect()
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Acquire) {
        Err(invalid("native operation cancelled"))
    } else {
        Ok(())
    }
}

/// Pin caller selectors rather than inherit a long-lived GUI server's environment.
/// Credentials and caller pane identities are intentionally excluded.
pub fn native_environment(
    stream: &str,
    caller: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    let home = Path::new(
        caller
            .get("HOME")
            .ok_or_else(|| invalid("HOME must be supplied explicitly"))?,
    );
    if !home.is_absolute() {
        return Err(invalid("HOME must be absolute"));
    }
    let default = |path: &str| home.join(path).to_string_lossy().into_owned();
    let nonempty = |key: &str, fallback: String| {
        caller
            .get(key)
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or(fallback)
    };
    let provided = |key: &str, fallback: String| caller.get(key).cloned().unwrap_or(fallback);
    let mut values: BTreeMap<_, _> = [
        ("STREAMS_STREAM_ID", stream.to_owned()),
        ("HOME", home.to_string_lossy().into_owned()),
        (
            "XDG_CONFIG_HOME",
            nonempty("XDG_CONFIG_HOME", default(".config")),
        ),
        (
            "XDG_DATA_HOME",
            nonempty("XDG_DATA_HOME", default(".local/share")),
        ),
        (
            "XDG_STATE_HOME",
            nonempty("XDG_STATE_HOME", default(".local/state")),
        ),
        (
            "PI_CODING_AGENT_DIR",
            nonempty("PI_CODING_AGENT_DIR", default(".pi/agent")),
        ),
        ("CODEX_HOME", provided("CODEX_HOME", default(".codex"))),
        (
            "CLAUDE_CONFIG_DIR",
            provided("CLAUDE_CONFIG_DIR", default(".claude")),
        ),
        ("PATH", provided("PATH", "/bin:/usr/bin".into())),
        ("HERDR_BIN_PATH", nonempty("HERDR_BIN_PATH", "herdr".into())),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect();
    for key in ["HERDR_SOCKET_PATH", "PI_OFFLINE"] {
        if let Some(value) = caller.get(key) {
            values.insert(key.into(), value.clone());
        }
    }
    if values.values().any(|value| value.contains('\0')) {
        return Err(invalid("invalid launch environment"));
    }
    Ok(values)
}
