//! Local native-name provenance. Pure policy grants no native effect authority.
//! Hold the existing workspace guard through observation, verified effect and persistence.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use unicode_general_category::{GeneralCategory, get_general_category};

use super::{Result, digest, invalid, native_state::NativeStateGuard};
use crate::file_io;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SurfaceKind {
    Workspace,
    Tab,
}

impl<'de> Deserialize<'de> for SurfaceKind {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "workspace" => Ok(Self::Workspace),
            "tab" => Ok(Self::Tab),
            _ => Err(serde::de::Error::custom("invalid naming surface kind")),
        }
    }
}

// Declaration order is JSON.stringify(validateScope(...)) order, hence filename identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NameScope {
    store_path: String,
    socket: String,
    workspace_id: String,
    binding_nonce: String,
    kind: SurfaceKind,
    surface_id: String,
}

fn whitespace(c: char) -> bool {
    matches!(c, '\t'..='\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}')
}
fn text(value: &str, nonempty: bool) -> Result<()> {
    if (nonempty && value.trim_matches(whitespace).is_empty())
        || value.chars().any(|c| {
            matches!(
                get_general_category(c),
                GeneralCategory::Control
                    | GeneralCategory::Format
                    | GeneralCategory::Surrogate
                    | GeneralCategory::LineSeparator
                    | GeneralCategory::ParagraphSeparator
            )
        })
    {
        return Err(invalid("invalid naming text"));
    }
    Ok(())
}
fn token(value: &str, surface: bool) -> Result<()> {
    text(value, true)?;
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || (surface && b == b':'))
    {
        return Err(invalid("invalid naming identity token"));
    }
    Ok(())
}
fn absolute(value: &str) -> Result<()> {
    text(value, true)?;
    // POSIX normalize preserves a single trailing slash but removes repeated slashes and dots.
    if !value.starts_with('/')
        || (value != "/"
            && value[1..]
                .trim_end_matches('/')
                .split('/')
                .any(|s| s.is_empty() || s == "." || s == ".."))
        || value.ends_with("//")
    {
        return Err(invalid("invalid naming absolute path"));
    }
    Ok(())
}
impl NameScope {
    fn validate(&self) -> Result<()> {
        absolute(&self.store_path)?;
        absolute(&self.socket)?;
        token(&self.workspace_id, false)?;
        token(&self.binding_nonce, false)?;
        token(&self.surface_id, true)
    }
    pub fn store_path(&self) -> &str {
        &self.store_path
    }
    pub fn socket(&self) -> &str {
        &self.socket
    }
    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }
    pub fn binding_nonce(&self) -> &str {
        &self.binding_nonce
    }
    pub fn kind(&self) -> SurfaceKind {
        self.kind
    }
    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }
}
impl<'de> Deserialize<'de> for NameScope {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        struct Wire {
            store_path: String,
            socket: String,
            workspace_id: String,
            binding_nonce: String,
            kind: SurfaceKind,
            surface_id: String,
        }
        let w: Wire = crate::codec::object(d)?;
        let scope = Self {
            store_path: w.store_path,
            socket: w.socket,
            workspace_id: w.workspace_id,
            binding_nonce: w.binding_nonce,
            kind: w.kind,
            surface_id: w.surface_id,
        };
        scope.validate().map_err(serde::de::Error::custom)?;
        Ok(scope)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NameOwner {
    terminal_id: String,
    nonce: String,
}
impl NameOwner {
    pub fn new(terminal_id: impl Into<String>, nonce: impl Into<String>) -> Result<Self> {
        let owner = Self {
            terminal_id: terminal_id.into(),
            nonce: nonce.into(),
        };
        token(&owner.terminal_id, false)?;
        token(&owner.nonce, false)?;
        Ok(owner)
    }
    pub fn terminal_id(&self) -> &str {
        &self.terminal_id
    }
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
}
impl<'de> Deserialize<'de> for NameOwner {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "camelCase")]
        struct Wire {
            terminal_id: String,
            nonce: String,
        }
        let w: Wire = crate::codec::object(d)?;
        Self::new(w.terminal_id, w.nonce).map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    owner: NameOwner,
    last_applied: String,
}
impl Applied {
    pub fn new(owner: NameOwner, last_applied: impl Into<String>) -> Result<Self> {
        let last_applied = last_applied.into();
        text(&last_applied, true)?;
        Ok(Self {
            owner,
            last_applied,
        })
    }
    pub fn owner(&self) -> &NameOwner {
        &self.owner
    }
    pub fn last_applied(&self) -> &str {
        &self.last_applied
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameProvenance {
    Automatic {
        scope: NameScope,
        applied: Applied,
    },
    Manual {
        scope: NameScope,
        previous: Option<Applied>,
    },
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ProvenanceWire {
    version: u32,
    scope: NameScope,
    mode: String,
    #[serde(deserialize_with = "crate::codec::required_nullable")]
    owner: Option<NameOwner>,
    #[serde(deserialize_with = "crate::codec::required_nullable")]
    last_applied: Option<String>,
}
impl NameProvenance {
    pub fn scope(&self) -> &NameScope {
        match self {
            Self::Automatic { scope, .. } | Self::Manual { scope, .. } => scope,
        }
    }
}
impl Serialize for NameProvenance {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        let (mode, applied) = match self {
            Self::Automatic { applied, .. } => ("auto", Some(applied)),
            Self::Manual { previous, .. } => ("manual", previous.as_ref()),
        };
        ProvenanceWire {
            version: 1,
            scope: self.scope().clone(),
            mode: mode.into(),
            owner: applied.map(|a| a.owner.clone()),
            last_applied: applied.map(|a| a.last_applied.clone()),
        }
        .serialize(s)
    }
}
impl<'de> Deserialize<'de> for NameProvenance {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let w: ProvenanceWire = crate::codec::object(d)?;
        let parsed = || -> Result<Self> {
            if w.version != 1 {
                return Err(invalid("invalid naming version"));
            }
            let previous = match (w.owner, w.last_applied) {
                (Some(owner), Some(name)) => Some(Applied::new(owner, name)?),
                (None, None) => None,
                _ => return Err(invalid("incomplete naming owner/applied pair")),
            };
            match (w.mode.as_str(), previous) {
                ("auto", Some(applied)) => Ok(Self::Automatic {
                    scope: w.scope,
                    applied,
                }),
                ("manual", previous) => Ok(Self::Manual {
                    scope: w.scope,
                    previous,
                }),
                _ => Err(invalid("invalid naming mode/applied pair")),
            }
        };
        parsed().map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NameIntent {
    #[default]
    Automatic,
    Explicit,
}
pub struct NameInput<'a> {
    pub scope: &'a NameScope,
    pub provenance: Option<&'a NameProvenance>,
    pub owner: &'a NameOwner,
    pub current_label: &'a str,
    pub desired_name: &'a str,
    pub default_label_proven: bool,
    pub intent: NameIntent,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameDecision {
    Noop,
    Adopt(NameProvenance),
    Manual(NameProvenance),
    Rename {
        name: String,
        after_success: NameProvenance,
    },
}
/// Caller proves eligibility, binding/context/tuple and exact default label under the guard.
/// Persist Manual immediately; Adopt/Rename only after fresh matching observation/verified success.
pub fn decide_name(input: NameInput<'_>) -> Result<NameDecision> {
    let NameInput {
        scope,
        provenance,
        owner,
        current_label: current,
        desired_name: desired,
        default_label_proven,
        intent,
    } = input;
    text(current, false)?;
    text(desired, true)?;
    if provenance.is_some_and(|p| p.scope() != scope) {
        return Err(invalid("naming scope mismatch"));
    }
    let automatic = || NameProvenance::Automatic {
        scope: scope.clone(),
        applied: Applied {
            owner: owner.clone(),
            last_applied: desired.into(),
        },
    };
    let rename = || NameDecision::Rename {
        name: desired.into(),
        after_success: automatic(),
    };
    if intent == NameIntent::Explicit {
        return Ok(if current == desired {
            NameDecision::Adopt(automatic())
        } else {
            rename()
        });
    }
    if let Some(prior @ NameProvenance::Manual { .. }) = provenance {
        return Ok(NameDecision::Manual(prior.clone()));
    }
    if provenance.is_none() && current == desired {
        return Ok(NameDecision::Adopt(automatic()));
    }
    let previous = provenance.and_then(|p| match p {
        NameProvenance::Automatic { applied, .. } => Some(applied),
        _ => None,
    });
    let drift = match previous {
        Some(applied) => current != applied.last_applied,
        None => {
            !(default_label_proven
                && current
                    .trim_matches(whitespace)
                    .bytes()
                    .all(|b| b.is_ascii_digit()))
        }
    };
    if drift {
        return Ok(NameDecision::Manual(NameProvenance::Manual {
            scope: scope.clone(),
            previous: previous.cloned(),
        }));
    }
    Ok(if current == desired {
        NameDecision::Noop
    } else {
        rename()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameSnapshot {
    pub path: PathBuf,
    pub revision: String,
    pub record: NameProvenance,
}
pub struct NameStore;
fn private(meta: &fs::Metadata) -> Result<()> {
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err(invalid("naming state must be private and user-owned"));
    }
    Ok(())
}
fn directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        return Err(invalid("naming state requires a real directory"));
    }
    private(&meta)
}
fn cancelled(signal: &AtomicBool) -> Result<()> {
    if signal.load(Ordering::Acquire) {
        return Err(invalid("naming publication cancelled"));
    }
    Ok(())
}
impl NameStore {
    pub fn scope(
        guard: &NativeStateGuard<'_>,
        socket: &str,
        workspace_id: &str,
        binding_nonce: &str,
        kind: SurfaceKind,
        surface_id: &str,
    ) -> Result<NameScope> {
        directory(guard.directory())?;
        let root = fs::canonicalize(guard.directory())?;
        let scope = NameScope {
            store_path: root
                .to_str()
                .ok_or_else(|| invalid("naming root must be UTF-8"))?
                .into(),
            socket: socket.into(),
            workspace_id: workspace_id.into(),
            binding_nonce: binding_nonce.into(),
            kind,
            surface_id: surface_id.into(),
        };
        scope.validate()?;
        Ok(scope)
    }
    pub fn prepare_scope(
        guard: &NativeStateGuard<'_>,
        socket: &str,
        workspace_id: &str,
        binding_nonce: &str,
        kind: SurfaceKind,
        surface_id: &str,
    ) -> Result<NameScope> {
        guard.prepare_directory()?;
        Self::scope(guard, socket, workspace_id, binding_nonce, kind, surface_id)
    }
    fn root(guard: &NativeStateGuard<'_>, scope: &NameScope) -> Result<PathBuf> {
        directory(guard.directory())?;
        if fs::canonicalize(guard.directory())?.to_str() != Some(scope.store_path.as_str()) {
            return Err(invalid("naming canonical root changed"));
        }
        directory(Path::new(&scope.store_path))?;
        Ok(Path::new(&scope.store_path).join(".native-names"))
    }
    pub fn path(guard: &NativeStateGuard<'_>, scope: &NameScope) -> Result<PathBuf> {
        let root = Self::root(guard, scope)?;
        Ok(root.join(format!(
            "{}.json",
            digest(&serde_json::to_vec(scope).expect("scope JSON"))
        )))
    }
    /// No creation on read. Ancestors above the root are trusted; same-user directory
    /// swaps are outside this path-based boundary. Never reenter the workspace flock.
    pub fn read(guard: &NativeStateGuard<'_>, scope: &NameScope) -> Result<Option<NameSnapshot>> {
        let cache = Self::root(guard, scope)?;
        match directory(&cache) {
            Err(super::RuntimeError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            result => result?,
        }
        let path = Self::path(guard, scope)?;
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
            return Err(invalid("naming cache must be a regular file"));
        }
        private(&meta)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let record: NameProvenance = file_io::decode_json(&bytes)?;
        if record.scope() != scope {
            return Err(invalid("naming scope mismatch"));
        }
        Ok(Some(NameSnapshot {
            path,
            revision: digest(&bytes),
            record,
        }))
    }
    /// Raw-byte CAS under the caller's flock, not CAS against uncooperating writers.
    /// Native success followed by cache failure is partial progress, never rollback proof.
    pub fn write(
        guard: &NativeStateGuard<'_>,
        scope: &NameScope,
        record: &NameProvenance,
        expected: Option<&str>,
        signal: &AtomicBool,
    ) -> Result<NameSnapshot> {
        if record.scope() != scope {
            return Err(invalid("naming scope mismatch"));
        }
        if expected.is_some_and(|s| {
            s.len() != 64
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }) {
            return Err(invalid("invalid naming revision"));
        }
        cancelled(signal)?;
        let cache = Self::root(guard, scope)?;
        let current = Self::read(guard, scope)?;
        if current.as_ref().map(|s| s.revision.as_str()) != expected {
            return Err(invalid("naming revision conflict; reread before retrying"));
        }
        if let Some(current) = current.as_ref().filter(|s| &s.record == record) {
            cancelled(signal)?;
            return Ok(current.clone());
        }
        match fs::DirBuilder::new().mode(0o700).create(&cache) {
            Ok(()) => {
                file_io::sync_directory(&cache)?;
                file_io::sync_directory(Path::new(scope.store_path()))?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        directory(&cache)?;
        let path = Self::path(guard, scope)?;
        let mut bytes = serde_json::to_vec(record).expect("provenance JSON");
        bytes.push(b'\n');
        let mut temp = tempfile::NamedTempFile::new_in(&cache)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        // Recheck the exact bytes before publication; no-clobber link closes initial-create races.
        if Self::read(guard, scope)?
            .as_ref()
            .map(|s| s.revision.as_str())
            != expected
        {
            return Err(invalid("naming revision conflict; reread before retrying"));
        }
        cancelled(signal)?;
        if current.is_some() {
            fs::rename(temp.path(), &path)?;
            drop(temp); // The temporary name no longer exists after rename.
        } else {
            fs::hard_link(temp.path(), &path)?;
            temp.close().map_err(|e| {
                crate::storage::StorageError::after_write(crate::storage::StorageError::new(
                    e.to_string(),
                ))
            })?;
        }
        file_io::sync_directory(&cache).map_err(crate::storage::StorageError::after_write)?;
        Ok(NameSnapshot {
            path,
            revision: digest(&bytes),
            record: record.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::native_state::NativeState;
    use super::*;
    use std::ffi::CString;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn scope() -> NameScope {
        serde_json::from_str(r#"{"storePath":"/state/é","socket":"/tmp/herdr.sock","workspaceId":"ws-1","bindingNonce":"binding_1","kind":"tab","surfaceId":"tab:1"}"#).unwrap()
    }
    fn owner() -> NameOwner {
        NameOwner::new("term_1", "nonce-1").unwrap()
    }
    fn automatic(scope: &NameScope, name: &str) -> NameProvenance {
        NameProvenance::Automatic {
            scope: scope.clone(),
            applied: Applied::new(owner(), name).unwrap(),
        }
    }
    fn decision(
        prior: Option<&NameProvenance>,
        current: &str,
        desired: &str,
        proof: bool,
        intent: NameIntent,
    ) -> NameDecision {
        decide_name(NameInput {
            scope: &scope(),
            provenance: prior,
            owner: &NameOwner::new("next", "next").unwrap(),
            current_label: current,
            desired_name: desired,
            default_label_proven: proof,
            intent,
        })
        .unwrap()
    }
    fn locked<'a>(state: &'a NativeState, socket: &Path) -> NativeStateGuard<'a> {
        state
            .lock(socket, "ws-1", 0.0, &AtomicBool::new(false))
            .unwrap()
    }
    fn local_scope(guard: &NativeStateGuard<'_>) -> NameScope {
        NameStore::prepare_scope(
            guard,
            "/tmp/herdr.sock",
            "ws-1",
            "binding_1",
            SurfaceKind::Tab,
            "tab:1",
        )
        .unwrap()
    }

    #[test]
    fn exact_wire_and_utf8_key_fixture() {
        let scope = scope();
        let json = r#"{"storePath":"/state/é","socket":"/tmp/herdr.sock","workspaceId":"ws-1","bindingNonce":"binding_1","kind":"tab","surfaceId":"tab:1"}"#;
        assert_eq!(serde_json::to_string(&scope).unwrap(), json);
        assert_eq!(
            digest(json.as_bytes()),
            "ca4f98064db61e9725a77e0af48414ea26b514127aa1c5916b9479e2f8e733ce"
        );
        let record = automatic(&scope, "Résumé");
        let wire = format!(
            r#"{{"version":1,"scope":{json},"mode":"auto","owner":{{"terminalId":"term_1","nonce":"nonce-1"}},"lastApplied":"Résumé"}}"#
        );
        assert_eq!(serde_json::to_string(&record).unwrap(), wire);
        assert_eq!(
            serde_json::from_str::<NameProvenance>(&wire).unwrap(),
            record
        );
        let manual = NameProvenance::Manual {
            scope,
            previous: None,
        };
        assert_eq!(
            serde_json::to_string(&manual).unwrap(),
            format!(
                r#"{{"version":1,"scope":{json},"mode":"manual","owner":null,"lastApplied":null}}"#
            )
        );
    }
    #[test]
    fn strict_schema_and_original_text_rules() {
        let base = serde_json::to_string(&automatic(&scope(), "Name")).unwrap();
        for bad in [
            base.replace("\"version\":1", "\"version\":1,\"version\":1"),
            base.replace("\"version\":1", "\"version\":2"),
            base.replace("\"mode\":\"auto\"", "\"mode\":{\"auto\":null}"),
            base.replace("\"kind\":\"tab\"", "\"kind\":{\"tab\":null}"),
            base.replace("\"lastApplied\":\"Name\"", "\"lastApplied\":null"),
            base.replace(
                "\"lastApplied\":\"Name\"",
                "\"extra\":1,\"lastApplied\":\"Name\"",
            ),
            base.replace(",\"lastApplied\":\"Name\"", ""),
            base.replace(
                "\"storePath\":\"/state/é\"",
                "\"storePath\":\"/state/../é\"",
            ),
            base.replace("\"terminalId\":\"term_1\"", "\"terminalId\":\"term:1\""),
            base.replace("\"lastApplied\":\"Name\"", "\"lastApplied\":\"bad\\u200b\""),
        ] {
            assert!(
                serde_json::from_str::<NameProvenance>(&bad).is_err(),
                "{bad}"
            );
        }
        assert!(serde_json::from_str::<NameOwner>(r#"["term","nonce"]"#).is_err());
        assert!(serde_json::from_str::<NameScope>(r#"[]"#).is_err());
        for path in ["relative", "//", "/a//b", "/a//", "/a/./b", "/a/../b"] {
            assert!(absolute(path).is_err(), "{path}");
        }
        for path in ["/", "/a/", "/a/b", "/é"] {
            absolute(path).unwrap();
        }
        for bad in ["", " ", "x\u{85}", "x\u{200d}", "x\u{2028}", "x\u{2029}"] {
            assert!(Applied::new(owner(), bad).is_err());
        }
        assert!(NameOwner::new("bad:token", "n").is_err());
    }
    #[test]
    fn policy_matrix_and_owner_handoff() {
        use NameIntent::{Automatic as Auto, Explicit};
        let prior = automatic(&scope(), "Old");
        assert!(matches!(
            decision(None, "Desired", "Desired", false, Auto),
            NameDecision::Adopt(_)
        ));
        assert!(matches!(
            decision(None, "1", "Desired", true, Auto),
            NameDecision::Rename { .. }
        ));
        assert!(matches!(
            decision(None, "", "Desired", true, Auto),
            NameDecision::Rename { .. }
        ));
        for (label, proof) in [("1", false), ("user", true), ("١", true)] {
            assert!(matches!(
                decision(None, label, "Desired", proof, Auto),
                NameDecision::Manual(NameProvenance::Manual { previous: None, .. })
            ));
        }
        let drift = decision(Some(&prior), "Desired", "Desired", true, Auto);
        let NameDecision::Manual(manual) = drift else {
            panic!("drift must pin even when matching desired")
        };
        assert_eq!(
            decision(Some(&manual), "Old", "Desired", true, Auto),
            NameDecision::Manual(manual.clone())
        );
        assert!(matches!(
            decision(Some(&manual), "Desired", "Desired", false, Explicit),
            NameDecision::Adopt(_)
        ));
        assert!(matches!(
            decision(Some(&manual), "user", "Desired", false, Explicit),
            NameDecision::Rename { .. }
        ));
        assert_eq!(
            decision(Some(&prior), "Old", "Old", false, Auto),
            NameDecision::Noop
        );
        let NameDecision::Rename {
            after_success: NameProvenance::Automatic { applied, .. },
            ..
        } = decision(Some(&prior), "Old", "New", false, Auto)
        else {
            panic!("managed handoff must rename")
        };
        assert_eq!(applied.owner().terminal_id(), "next");
        assert!(matches!(
            decision(Some(&prior), "user", "New", true, Auto),
            NameDecision::Manual(NameProvenance::Manual {
                previous: Some(_),
                ..
            })
        ));
    }
    #[test]
    fn missing_read_cas_raw_bytes_noop_and_cancellation() {
        let temp = tempfile::tempdir().unwrap();
        let state = NativeState::new(temp.path().join("state")).unwrap();
        let guard = locked(&state, &temp.path().join("socket"));
        assert!(
            NameStore::scope(&guard, "/tmp/sock", "ws", "n", SurfaceKind::Workspace, "ws").is_err()
        );
        assert!(!state.directory().exists());
        let scope = local_scope(&guard);
        let cache = state.directory().join(".native-names");
        assert!(NameStore::read(&guard, &scope).unwrap().is_none());
        assert!(!cache.exists());
        let record = automatic(&scope, "One");
        let signal = AtomicBool::new(false);
        let first = NameStore::write(&guard, &scope, &record, None, &signal).unwrap();
        assert!(NameStore::write(&guard, &scope, &record, None, &signal).is_err());
        let formatted = format!("  {}  \n", serde_json::to_string_pretty(&record).unwrap());
        fs::write(&first.path, &formatted).unwrap();
        let observed = NameStore::read(&guard, &scope).unwrap().unwrap();
        assert_eq!(observed.revision, digest(formatted.as_bytes()));
        let mtime = fs::metadata(&first.path).unwrap().modified().unwrap();
        let same =
            NameStore::write(&guard, &scope, &record, Some(&observed.revision), &signal).unwrap();
        assert_eq!(same, observed);
        assert_eq!(fs::read(&first.path).unwrap(), formatted.as_bytes());
        assert_eq!(
            fs::metadata(&first.path).unwrap().modified().unwrap(),
            mtime
        );
        assert!(NameStore::write(&guard, &scope, &record, Some(&first.revision), &signal).is_err());
        signal.store(true, Ordering::Release);
        assert!(
            NameStore::write(
                &guard,
                &scope,
                &automatic(&scope, "Two"),
                Some(&observed.revision),
                &signal
            )
            .is_err()
        );
        signal.store(false, Ordering::Release);
        let next = NameStore::write(
            &guard,
            &scope,
            &automatic(&scope, "Two"),
            Some(&observed.revision),
            &signal,
        )
        .unwrap();
        assert_ne!(next.revision, observed.revision);
        assert_eq!(fs::read_dir(cache).unwrap().count(), 1);
    }
    #[test]
    fn canonical_ancestor_aliases_share_keys_and_retargeting_fails() {
        let temp = tempfile::tempdir().unwrap();
        let actual = temp.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let other = temp.path().join("other");
        fs::create_dir(&other).unwrap();
        let alias = temp.path().join("alias");
        symlink(&actual, &alias).unwrap();
        let state = NativeState::new(alias.join("state")).unwrap();
        let guard = locked(&state, &temp.path().join("socket"));
        let scope = local_scope(&guard);
        assert_eq!(
            scope.store_path(),
            fs::canonicalize(actual.join("state"))
                .unwrap()
                .to_str()
                .unwrap()
        );
        let mut trailing = scope.clone();
        trailing.store_path.push('/');
        assert!(NameStore::read(&guard, &trailing).is_err());
        fs::create_dir(other.join("state")).unwrap();
        fs::set_permissions(other.join("state"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_file(&alias).unwrap();
        symlink(&other, &alias).unwrap();
        assert!(NameStore::read(&guard, &scope).is_err());
        assert!(
            NameStore::write(
                &guard,
                &scope,
                &automatic(&scope, "Name"),
                None,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(!other.join("state/.native-names").exists());
    }
    #[test]
    fn unsafe_cache_leaf_modes_fifo_and_root_changes() {
        let temp = tempfile::tempdir().unwrap();
        let state = NativeState::new(temp.path().join("state")).unwrap();
        let guard = locked(&state, &temp.path().join("socket"));
        let scope = local_scope(&guard);
        let signal = AtomicBool::new(false);
        let record = automatic(&scope, "Name");
        let first = NameStore::write(&guard, &scope, &record, None, &signal).unwrap();
        fs::set_permissions(&first.path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(NameStore::read(&guard, &scope).is_err());
        fs::remove_file(&first.path).unwrap();
        symlink(temp.path().join("missing"), &first.path).unwrap();
        assert!(NameStore::read(&guard, &scope).is_err());
        fs::remove_file(&first.path).unwrap();
        let cpath = CString::new(first.path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
        assert!(NameStore::read(&guard, &scope).is_err());
        fs::remove_file(&first.path).unwrap();
        fs::write(&first.path, b"not JSON").unwrap();
        fs::set_permissions(&first.path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(NameStore::read(&guard, &scope).is_err());
        fs::remove_file(&first.path).unwrap();
        let cache = first.path.parent().unwrap();
        fs::set_permissions(cache, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(NameStore::read(&guard, &scope).is_err());
        fs::set_permissions(cache, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir(cache).unwrap();
        symlink(temp.path(), cache).unwrap();
        assert!(NameStore::read(&guard, &scope).is_err());
        fs::remove_file(cache).unwrap();
        fs::rename(state.directory(), temp.path().join("moved")).unwrap();
        symlink(temp.path().join("moved"), state.directory()).unwrap();
        assert!(NameStore::read(&guard, &scope).is_err());
    }
}
