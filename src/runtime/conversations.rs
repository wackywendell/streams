//! Bounded saved-conversation checks. Filenames are discovery hints, never identity proof.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use crate::codec::object;

use super::{Result, RuntimeError, decode, invalid, text};

const LIMIT: usize = 65536;
const DISCOVERY_LIMIT: usize = 10000;

#[cfg(test)]
thread_local! {
    pub(super) static IDENTITY_FILE_OPENS: std::cell::RefCell<Vec<PathBuf>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
mod tests;

/// Native transcript formats supported by saved identity verification.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    strum::EnumString,
    strum::Display,
    strum::AsRefStr,
    strum::VariantNames,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Harness {
    Pi,
    Codex,
    Claude,
}

/// Native metadata distinguishes an advertised canonical ID from an exact saved path.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    strum::EnumString,
    strum::Display,
    strum::AsRefStr,
    strum::VariantNames,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum NativeIdentityKind {
    Id,
    Path,
}

crate::codec::deserialize_string_enum!(Harness, NativeIdentityKind);

/// Native metadata is only a hint until its harness and saved file are checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeSessionMetadata {
    pub agent: Harness,
    pub kind: NativeIdentityKind,
    pub value: String,
}

/// Canonical native identity with a verified locator when metadata supplied a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeIdentity {
    pub id: String,
    pub locator: Option<PathBuf>,
}

/// Explicit discovery roots keep callers and tests independent of ambient native sessions.
#[derive(Debug, Clone, Default)]
pub struct ConversationRoots {
    pub pi: Vec<PathBuf>,
    pub codex: Vec<PathBuf>,
    pub claude: Vec<PathBuf>,
}

impl ConversationRoots {
    /// Resolve conventional roots from the caller's environment; this does not inspect them.
    pub fn from_environment() -> Result<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| invalid("HOME is unavailable"))?;
        let base = |key, default| std::env::var_os(key).map(PathBuf::from).unwrap_or(default);
        let pi = base("PI_CODING_AGENT_DIR", home.join(".pi/agent"));
        let codex = base("CODEX_HOME", home.join(".codex"));
        let claude = base("CLAUDE_CONFIG_DIR", home.join(".claude"));
        Ok(Self {
            pi: vec![pi.join("sessions")],
            codex: vec![codex.join("sessions"), codex.join("archived_sessions")],
            claude: vec![claude.join("projects")],
        })
    }

    fn for_harness(&self, harness: Harness) -> &[PathBuf] {
        match harness {
            Harness::Pi => &self.pi,
            Harness::Codex => &self.codex,
            Harness::Claude => &self.claude,
        }
    }
}

#[derive(Deserialize)]
struct PiHeader {
    r#type: String,
    id: String,
}
#[derive(Deserialize)]
struct CodexHeader {
    r#type: String,
    #[serde(deserialize_with = "object")]
    payload: CodexPayload,
}
#[derive(Deserialize)]
struct CodexPayload {
    id: String,
}

fn present_identity<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct ClaudeRecord {
    #[serde(rename = "sessionId", default, deserialize_with = "present_identity")]
    session_id: Option<String>,
}

fn identity_prefix(harness: Harness, bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() || bytes.len() > LIMIT && !bytes[..LIMIT].contains(&b'\n') {
        return Err(invalid("missing or oversized conversation header"));
    }
    let mut lines: Vec<_> = bytes.split_inclusive(|byte| *byte == b'\n').collect();
    let id = match harness {
        Harness::Pi => {
            let header: PiHeader = decode(lines[0])?;
            if header.r#type != "session" {
                return Err(invalid("wrong conversation header type"));
            }
            header.id
        }
        Harness::Codex => {
            let header: CodexHeader = decode(lines[0])?;
            if header.r#type != "session_meta" {
                return Err(invalid("wrong conversation header type"));
            }
            header.payload.id
        }
        Harness::Claude => {
            // A trailing partial record is not evidence. Every complete identity-bearing
            // record in the bounded prefix must agree, including records after the first.
            if bytes.len() > LIMIT || !bytes.ends_with(b"\n") {
                lines.pop();
            }
            let mut identities = BTreeSet::new();
            for line in lines {
                let record: ClaudeRecord = decode(line)?;
                if let Some(id) = record.session_id {
                    text(&id)?;
                    identities.insert(id);
                }
            }
            if identities.len() != 1 {
                return Err(invalid("missing or ambiguous Claude sessionId"));
            }
            identities.into_iter().next().expect("one identity")
        }
    };
    text(&id)?;
    Ok(id)
}

fn verified_file(harness: Harness, path: &Path) -> Result<(String, PathBuf)> {
    if !path.is_absolute() {
        return Err(invalid("saved conversation path must be absolute"));
    }
    let canonical = fs::canonicalize(path)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&canonical)?;
    #[cfg(test)]
    IDENTITY_FILE_OPENS.with(|opened| opened.borrow_mut().push(canonical.clone()));
    if !file.metadata()?.is_file() {
        return Err(invalid("saved conversation must be a regular file"));
    }
    let mut prefix = Vec::new();
    file.take((LIMIT + 1) as u64).read_to_end(&mut prefix)?;
    Ok((identity_prefix(harness, &prefix)?, canonical))
}

/// Read at most 65,537 bytes and verify the source-specific identity records.
/// No cwd, title, filename, or catalog association participates in this check.
pub fn file_identity(harness: Harness, path: &Path) -> Result<String> {
    verified_file(harness, path).map(|(id, _)| id)
}

/// Validate native metadata against the expected harness and inspect path-based identities.
pub fn native_identity(
    metadata: &NativeSessionMetadata,
    harness: Harness,
) -> Result<NativeIdentity> {
    if metadata.agent != harness {
        return Err(invalid("native agent_session harness mismatch"));
    }
    text(&metadata.value)?;
    match metadata.kind {
        NativeIdentityKind::Id => Ok(NativeIdentity {
            id: metadata.value.clone(),
            locator: None,
        }),
        NativeIdentityKind::Path => {
            let (id, locator) = verified_file(harness, Path::new(&metadata.value))?;
            Ok(NativeIdentity {
                id,
                locator: Some(locator),
            })
        }
    }
}

struct Discovery<'a> {
    identity: &'a str,
    count: usize,
    matches: BTreeSet<PathBuf>,
}

impl<'a> Discovery<'a> {
    fn new(identity: &'a str) -> Self {
        Self {
            identity,
            count: 0,
            matches: BTreeSet::new(),
        }
    }

    fn candidate(&mut self, path: &Path) -> Result<bool> {
        if path
            .extension()
            .is_none_or(|extension| extension != "jsonl")
        {
            return Ok(false);
        }
        self.count += 1;
        if self.count > DISCOVERY_LIMIT {
            return Err(invalid(
                "conversation discovery limit exceeded; capture an exact saved locator",
            ));
        }
        Ok(path
            .file_name()
            .expect("JSONL filename")
            .to_string_lossy()
            .contains(self.identity))
    }

    fn record(&mut self, found: &str, canonical: PathBuf) {
        if found == self.identity {
            self.matches.insert(canonical);
        }
    }

    fn finish(self) -> Result<PathBuf> {
        if self.matches.len() != 1 {
            let message = format!(
                "saved conversation {:?} unavailable or ambiguous ({} verified files)",
                self.identity,
                self.matches.len()
            );
            return if self.matches.is_empty() {
                Err(RuntimeError::NotFound(message))
            } else {
                Err(RuntimeError::Ambiguous {
                    message,
                    candidates: self
                        .matches
                        .into_iter()
                        .map(|path| path.to_string_lossy().into_owned())
                        .collect(),
                })
            };
        }
        Ok(self.matches.into_iter().next().expect("one verified file"))
    }
}

/// Find exactly one verified saved conversation, or check the exact supplied locator.
/// Discovery is bounded to 10,000 JSONL files; malformed matching hints are errors.
pub fn conversation_file(
    harness: Harness,
    identity: &str,
    locator: Option<&Path>,
    roots: &ConversationRoots,
) -> Result<PathBuf> {
    text(identity)?;
    if let Some(path) = locator {
        let (found, canonical) = verified_file(harness, path)?;
        if found != identity {
            return Err(invalid("saved conversation identity/path mismatch"));
        }
        return Ok(canonical);
    }
    let mut discovery = Discovery::new(identity);
    for root in roots.for_harness(harness) {
        let mut pending = vec![root.clone()];
        while let Some(directory) = pending.pop() {
            let entries = match fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(error)
                    if error.kind() == std::io::ErrorKind::NotFound && directory == *root =>
                {
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let entry = entry?;
                let file_type = entry.file_type()?;
                let path = entry.path();
                // Do not recurse through symlinked directories or chase cycles.
                if file_type.is_dir() {
                    pending.push(path);
                    continue;
                }
                if discovery.candidate(&path)? {
                    let (found, canonical) = verified_file(harness, &path)?;
                    discovery.record(&found, canonical);
                }
            }
        }
    }
    discovery.finish()
}
