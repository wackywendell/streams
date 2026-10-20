//! Durable recovery markers, retry receipts and transport scheduling state.
//!
//! Reads reject duplicate JSON keys and unsupported versions; typed readers also
//! reject unknown fields and invalid phase-specific data. Missing records may have
//! defaults, but corrupt or unreadable records must not be treated as absent.
//!
//! Methods do not acquire a catalog lock. Callers must serialize read/modify/write
//! sequences and stop effects when state needs repair. A failed write or clear may
//! already have changed disk state; do not assume rollback or retry blindly.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::model::check_text;
use crate::operations::Operation;

use super::envelope::{check_fingerprints, load_json, operation_digest};

/// State cannot safely support further effects without inspection or repair.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RepairRequired(pub String);

impl RepairRequired {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// Checks lowercase SHA-1 or SHA-256 object-ID syntax, not object existence.
pub fn check_oid(value: Option<&str>, nullable: bool) -> Result<(), String> {
    let Some(s) = value else {
        if nullable {
            return Ok(());
        }
        return Err("expected a Git object ID".to_string());
    };
    if (s.len() != 40 && s.len() != 64)
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("expected a Git object ID".to_string());
    }
    Ok(())
}

/// Checks finiteness, allowing absence only when `nullable`; no clock ordering is checked.
pub fn check_timestamp(value: Option<f64>, nullable: bool) -> Result<(), String> {
    let Some(v) = value else {
        if nullable {
            return Ok(());
        }
        return Err("expected a finite timestamp".to_string());
    };
    if !v.is_finite() {
        return Err("expected a finite timestamp".to_string());
    }
    Ok(())
}

// Option normally accepts missing keys in Serde; these nullable fields are required.
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// Independent publication and refresh clocks, expressed in seconds.
///
/// `last_sync` tracks successful sync completion; `last_push` is the publication
/// scheduling baseline. Refresh uses `fetched_at`, `refresh_retry_after` and
/// `refresh_error`, independently of publication's `retry_after`. These timestamps
/// schedule work; none alone proves that a remote accepted a candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyncMetadata {
    #[serde(deserialize_with = "required_nullable")]
    pub confirmed: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub last_sync: Option<f64>,
    #[serde(deserialize_with = "required_nullable")]
    pub last_push: Option<f64>,
    pub retry_after: f64,
    #[serde(deserialize_with = "required_nullable")]
    pub fetched_at: Option<f64>,
    /// Candidate whose confirmation established `last_push`, so recovery records
    /// that baseline once rather than advancing it on every retry.
    pub last_confirmed_attempt: Option<String>,
    #[serde(default)]
    pub refresh_retry_after: f64,
    #[serde(default)]
    pub refresh_error: String,
}

impl Default for SyncMetadata {
    fn default() -> Self {
        Self {
            confirmed: None,
            last_sync: None,
            last_push: None,
            retry_after: 0.0,
            fetched_at: None,
            last_confirmed_attempt: None,
            refresh_retry_after: 0.0,
            refresh_error: String::new(),
        }
    }
}

impl SyncMetadata {
    /// Checks ID and timestamp shapes and requires a baseline for a recorded attempt.
    pub fn validated(self) -> Result<Self, String> {
        check_oid(self.confirmed.as_deref(), true)?;
        check_oid(self.last_confirmed_attempt.as_deref(), true)?;
        for val in [self.last_sync, self.last_push, self.fetched_at] {
            check_timestamp(val, true)?;
        }
        for val in [self.retry_after, self.refresh_retry_after] {
            check_timestamp(Some(val), false)?;
        }
        if self.last_confirmed_attempt.is_some() && self.last_push.is_none() {
            return Err("confirmation identity requires its scheduling baseline".to_string());
        }
        Ok(self)
    }
}

/// Stored transport selection; `None` for `remote` means local-only operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitConfig {
    #[serde(deserialize_with = "required_nullable")]
    pub remote: Option<String>,
    pub branch: String,
    pub host: String,
}

impl GitConfig {
    /// Checks nonempty text, not remote reachability or Git ref-name validity.
    pub fn new(
        remote: Option<String>,
        branch: impl Into<String>,
        host: impl Into<String>,
    ) -> Result<Self, String> {
        let branch = branch.into();
        let host = host.into();
        if let Some(ref r) = remote {
            check_text(r, "remote", false).map_err(|e| e.to_string())?;
        }
        check_text(&branch, "branch", false).map_err(|e| e.to_string())?;
        check_text(&host, "host", false).map_err(|e| e.to_string())?;
        Ok(Self {
            remote,
            branch,
            host,
        })
    }
}

/// Recorded facts about a candidate publication, not verified confirmation evidence.
///
/// IDs, target and intended operations let recovery identify what was attempted.
/// Even `confirmed_at` is stored data: callers must verify history and remote
/// observations before treating the attempt as published. Public fields can be
/// changed after construction, so persistence validates their shapes again.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicationAttempt {
    pub base: Option<String>,
    pub candidate: String,
    pub local_tip: String,
    pub branch: String,
    pub remote: String,
    pub operations: Vec<String>,
    pub confirmed_at: Option<f64>,
}

impl PublicationAttempt {
    /// Checks field syntax and unique operation IDs, without inspecting Git history.
    pub fn new(
        base: Option<String>,
        candidate: impl Into<String>,
        local_tip: impl Into<String>,
        branch: impl Into<String>,
        remote: impl Into<String>,
        operations: Vec<String>,
        confirmed_at: Option<f64>,
    ) -> Result<Self, String> {
        let candidate = candidate.into();
        let local_tip = local_tip.into();
        let branch = branch.into();
        let remote = remote.into();
        check_oid(base.as_deref(), true)?;
        check_oid(Some(&candidate), false)?;
        check_oid(Some(&local_tip), false)?;
        check_text(&branch, "branch", false).map_err(|e| e.to_string())?;
        check_text(&remote, "remote", false).map_err(|e| e.to_string())?;
        check_timestamp(confirmed_at, true)?;
        let mut seen = BTreeSet::new();
        for id in &operations {
            check_text(id, "operation identity", false).map_err(|e| e.to_string())?;
            if !seen.insert(id.clone()) {
                return Err("duplicate publication operation identities".to_string());
            }
        }
        Ok(Self {
            base,
            candidate,
            local_tip,
            branch,
            remote,
            operations,
            confirmed_at,
        })
    }

    /// Adds the planned local-tip transition without verifying that it is safe.
    pub fn adopting(
        &self,
        old: impl Into<String>,
        new: impl Into<String>,
    ) -> Result<AdoptingAttempt, String> {
        AdoptingAttempt::new(self.clone(), old.into(), new.into())
    }
}

/// An attempt plus the old and new tips recorded for local adoption recovery.
///
/// These are transition facts to check against Git, not proof that publication or
/// adoption succeeded. Construction checks tip syntax only, not their relationship.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdoptingAttempt {
    #[serde(flatten)]
    pub attempt: PublicationAttempt,
    pub adoption_from: String,
    pub adoption_to: String,
}

impl AdoptingAttempt {
    /// Checks adoption-tip syntax; does not revalidate the supplied attempt.
    pub fn new(
        attempt: PublicationAttempt,
        adoption_from: impl Into<String>,
        adoption_to: impl Into<String>,
    ) -> Result<Self, String> {
        let adoption_from = adoption_from.into();
        let adoption_to = adoption_to.into();
        check_oid(Some(&adoption_from), false)?;
        check_oid(Some(&adoption_to), false)?;
        Ok(Self {
            attempt,
            adoption_from,
            adoption_to,
        })
    }
}

/// Durable recovery phase; disk parsing requires exactly the fields for that phase.
///
/// Retain the marker until recovery has verified and completed the relevant effects.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum PublicationMarker {
    Attempting(PublicationAttempt),
    Adopting(AdoptingAttempt),
}

impl PublicationMarker {
    /// Returns the common publication facts regardless of recovery phase.
    pub fn attempt(&self) -> &PublicationAttempt {
        match self {
            PublicationMarker::Attempting(a) => a,
            PublicationMarker::Adopting(a) => &a.attempt,
        }
    }
}

// A flat disk schema avoids Serde's flatten/deny_unknown_fields interaction,
// so phase-specific fields can be rejected before constructing a marker.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum PublicationPhase {
    Attempting,
    Adopting,
}

fn present_string<'de, D>(d: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    String::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationData {
    phase: PublicationPhase,
    #[serde(deserialize_with = "required_nullable")]
    base: Option<String>,
    candidate: String,
    local_tip: String,
    branch: String,
    remote: String,
    operations: Vec<String>,
    confirmed_at: Option<f64>,
    #[serde(default, deserialize_with = "present_string")]
    adoption_from: Option<String>,
    #[serde(default, deserialize_with = "present_string")]
    adoption_to: Option<String>,
}

impl PublicationData {
    fn into_marker(self) -> Result<PublicationMarker, RepairRequired> {
        let attempt = PublicationAttempt::new(
            self.base,
            self.candidate,
            self.local_tip,
            self.branch,
            self.remote,
            self.operations,
            self.confirmed_at,
        )
        .map_err(RepairRequired::new)?;
        match (self.phase, self.adoption_from, self.adoption_to) {
            (PublicationPhase::Attempting, None, None) => {
                Ok(PublicationMarker::Attempting(attempt))
            }
            (PublicationPhase::Adopting, Some(old), Some(new)) => {
                AdoptingAttempt::new(attempt, old, new)
                    .map(PublicationMarker::Adopting)
                    .map_err(RepairRequired::new)
            }
            _ => Err(RepairRequired::new(
                "publication phase does not match adoption evidence",
            )),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipts {
    operations: BTreeMap<String, String>,
}

/// Failure-injection hook called before a named write, clear or receipt update.
/// Runs under its mutex; it must not recursively invoke hooked methods.
pub type StateHook = Arc<Mutex<dyn FnMut(&str, &str) -> Result<(), RepairRequired> + Send>>;

/// Access to `.catalog-sync` records; clones do not provide transaction isolation.
///
/// Names passed to raw record methods must be trusted bookkeeping filenames.
/// Typed accessors validate their schema; raw reads validate only JSON and version.
#[derive(Clone)]
pub struct State {
    pub root: PathBuf,
    hook: Option<StateHook>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State").field("root", &self.root).finish()
    }
}

impl State {
    /// Selects the catalog's `.catalog-sync` directory without creating it yet.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().join(".catalog-sync"),
            hook: None,
        }
    }

    /// Installs a hook that may fail an operation before its state effects begin.
    pub fn with_hook(mut self, hook: StateHook) -> Self {
        self.hook = Some(hook);
        self
    }

    fn run_hook(&self, op: &str, name: &str) -> Result<(), RepairRequired> {
        if let Some(ref hook) = self.hook {
            let mut guard = hook.lock().unwrap();
            guard(op, name)?;
        }
        Ok(())
    }

    fn ensure_directory(&self) -> Result<(), RepairRequired> {
        if self.root.is_symlink() {
            return Err(RepairRequired::new("symlinked transport state"));
        }
        crate::yaml_store::ensure_directory(&self.root, true)
            .map_err(|e| RepairRequired::new(e.to_string()))
    }

    /// Reads a version-1 JSON object, rejecting duplicate keys and symlinked state.
    ///
    /// Ensures the state directory exists; returns `None` when the path existence
    /// check reports absence. Record-specific fields are checked by typed readers,
    /// not here, and reads do not provide an atomic view across multiple files.
    pub fn read(&self, name: &str) -> Result<Option<Map<String, Value>>, RepairRequired> {
        self.ensure_directory()?;
        let path = self.root.join(name);
        if path.is_symlink() {
            return Err(RepairRequired::new(format!(
                "symlinked transport state: {}",
                path.display()
            )));
        }
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)
            .map_err(|e| RepairRequired::new(format!("cannot read {}: {e}", path.display())))?;
        let val = load_json(&text)
            .map_err(|e| RepairRequired::new(format!("cannot read {}: {e}", path.display())))?;
        let Some(obj) = val.as_object() else {
            return Err(RepairRequired::new(format!(
                "cannot read {}: unsupported transport state version",
                path.display()
            )));
        };
        match obj.get("version") {
            Some(Value::Number(n)) if n.as_i64() == Some(1) => Ok(Some(obj.clone())),
            _ => Err(RepairRequired::new(format!(
                "cannot read {}: unsupported transport state version",
                path.display()
            ))),
        }
    }

    pub(crate) fn read_record<T: DeserializeOwned>(
        &self,
        name: &str,
    ) -> Result<Option<T>, RepairRequired> {
        self.read(name)?
            .map(|mut data| {
                data.remove("version");
                serde_json::from_value(Value::Object(data)).map_err(|e| {
                    RepairRequired::new(format!("invalid {}: {e}", self.root.join(name).display()))
                })
            })
            .transpose()
    }

    pub(crate) fn write_record(
        &self,
        name: &str,
        record: &impl Serialize,
    ) -> Result<(), RepairRequired> {
        let Value::Object(data) = serde_json::to_value(record)
            .map_err(|e| RepairRequired::new(format!("cannot serialize {name}: {e}")))?
        else {
            return Err(RepairRequired::new("transport record must be an object"));
        };
        self.write(name, data)
    }

    /// Loads validated scheduling metadata, using defaults only when the file is absent.
    pub fn metadata(&self) -> Result<SyncMetadata, RepairRequired> {
        self.read_record::<SyncMetadata>("sync.json")?
            .unwrap_or_default()
            .validated()
            .map_err(RepairRequired::new)
    }

    /// Validates and persists scheduling state without deriving confirmation evidence.
    pub fn write_metadata(&self, metadata: &SyncMetadata) -> Result<(), RepairRequired> {
        metadata.clone().validated().map_err(RepairRequired::new)?;
        self.write_record("sync.json", metadata)
    }

    /// Loads the transport selection; absence is distinct from invalid configuration.
    pub fn config(&self) -> Result<Option<GitConfig>, RepairRequired> {
        self.read_record::<GitConfig>("config.json")?
            .map(|c| GitConfig::new(c.remote, c.branch, c.host).map_err(RepairRequired::new))
            .transpose()
    }

    /// Validates text fields and persists the transport selection.
    pub fn write_config(&self, config: &GitConfig) -> Result<(), RepairRequired> {
        GitConfig::new(config.remote.clone(), &config.branch, &config.host)
            .map_err(RepairRequired::new)?;
        self.write_record("config.json", config)
    }

    /// Loads a recovery marker, rejecting unsupported phases or mismatched adoption fields.
    /// The returned facts still need verification against Git before recovery effects.
    pub fn publication(&self) -> Result<Option<PublicationMarker>, RepairRequired> {
        self.read_record::<PublicationData>("publication.json")?
            .map(PublicationData::into_marker)
            .transpose()
    }

    /// Revalidates marker field shapes and persists the recovery phase.
    pub fn write_publication(&self, marker: &PublicationMarker) -> Result<(), RepairRequired> {
        let a = marker.attempt();
        PublicationAttempt::new(
            a.base.clone(),
            &a.candidate,
            &a.local_tip,
            &a.branch,
            &a.remote,
            a.operations.clone(),
            a.confirmed_at,
        )
        .map_err(RepairRequired::new)?;
        if let PublicationMarker::Adopting(a) = marker {
            check_oid(Some(&a.adoption_from), false).map_err(RepairRequired::new)?;
            check_oid(Some(&a.adoption_to), false).map_err(RepairRequired::new)?;
        }
        self.write_record("publication.json", marker)
    }

    /// Writes a version-1 object by atomic replacement and syncs its directory.
    /// Failure may occur after replacement; it does not imply the old record remains.
    pub fn write(&self, name: &str, mut data: Map<String, Value>) -> Result<(), RepairRequired> {
        self.run_hook("write", name)?;
        self.ensure_directory()?;
        data.insert("version".into(), Value::Number(1.into()));
        let bytes = serde_json::to_vec(&Value::Object(data))
            .map_err(|e| RepairRequired::new(format!("cannot serialize {name}: {e}")))?;
        crate::yaml_store::atomic_write(&self.root.join(name), &bytes, &self.root)
            .map_err(|e| RepairRequired::new(e.to_string()))
    }

    /// Removes a marker if present and syncs the directory, including on an absent file.
    /// A sync failure can leave the removal's durability uncertain.
    pub fn clear(&self, name: &str) -> Result<(), RepairRequired> {
        self.run_hook("clear", name)?;
        self.ensure_directory()?;
        let path = self.root.join(name);
        if path.exists() {
            fs::remove_file(&path)
                .map_err(|e| RepairRequired::new(format!("cannot clear {name}: {e}")))?;
        }
        self.sync_dir()
    }

    fn sync_dir(&self) -> Result<(), RepairRequired> {
        crate::yaml_store::sync_directory(&self.root)
            .map_err(|e| RepairRequired::new(e.to_string()))
    }

    /// Loads validated retry receipts keyed by operation ID; absence means no receipts.
    pub fn fingerprints(&self) -> Result<BTreeMap<String, String>, RepairRequired> {
        let operations = self
            .read_record::<Receipts>("receipts.json")?
            .map(|r| r.operations)
            .unwrap_or_default();
        check_fingerprints(&operations)
            .map_err(|e| RepairRequired::new(format!("invalid operation retry receipts: {e}")))?;
        Ok(operations)
    }

    /// Tests whether the same intent was recorded; reused IDs with different intent fail closed.
    pub fn seen(&self, operation: &Operation) -> Result<bool, RepairRequired> {
        let fps = self.fingerprints()?;
        if let Some(digest) = fps.get(&operation.id) {
            if digest != &operation_digest(operation) {
                return Err(RepairRequired::new(format!(
                    "operation ID reused with different intent: {}",
                    operation.id
                )));
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Persists an intent receipt, rejecting any prior use of the ID for another intent.
    pub fn remember(&self, operation: &Operation) -> Result<(), RepairRequired> {
        self.run_hook("remember", &operation.id)?;
        let mut incoming = BTreeMap::new();
        incoming.insert(operation.id.clone(), operation_digest(operation));
        self.validate_fingerprints(&incoming, true)
    }

    /// Checks incoming receipts against saved intent, optionally persisting new entries.
    /// Callers must hold their lock across this read/modify/write sequence.
    pub fn validate_fingerprints(
        &self,
        fingerprints: &BTreeMap<String, String>,
        remember: bool,
    ) -> Result<(), RepairRequired> {
        check_fingerprints(fingerprints).map_err(|e| RepairRequired::new(e.to_string()))?;
        let incoming = fingerprints;
        let mut operations = self.fingerprints()?;
        for (identity, digest) in incoming {
            if let Some(existing) = operations.get(identity)
                && existing != digest
            {
                return Err(RepairRequired::new(format!(
                    "operation ID reused with different intent: {identity}"
                )));
            }
        }
        if remember && incoming.keys().any(|k| !operations.contains_key(k)) {
            operations.extend(
                incoming
                    .iter()
                    .map(|(id, digest)| (id.clone(), digest.clone())),
            );
            self.write_record("receipts.json", &Receipts { operations })?;
        }
        Ok(())
    }

    /// Blocks further writes while an interrupted-save marker remains for inspection.
    pub fn check_save(&self) -> Result<(), RepairRequired> {
        if self.read("save.json")?.is_some() {
            return Err(RepairRequired::new(format!(
                "interrupted save; inspect {} before further writes",
                self.root.join("save.json").display()
            )));
        }
        Ok(())
    }

    /// Rejects interrupted saves, invalid receipts or bookkeeping incompatible with local mode.
    pub fn check_local(&self) -> Result<(), RepairRequired> {
        self.check_save()?;
        let allowed: BTreeSet<&str> =
            ["cli.json", "local.json", "receipts.json", "save.json"].into();
        for entry in fs::read_dir(&self.root)
            .map_err(|e| RepairRequired::new(format!("cannot read {}: {e}", self.root.display())))?
        {
            let entry = entry.map_err(|e| RepairRequired::new(e.to_string()))?;
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if !allowed.contains(name_str.as_ref()) {
                return Err(RepairRequired::new(
                    "unexpected local catalog bookkeeping; strategy cannot be switched",
                ));
            }
        }
        self.fingerprints()?;
        Ok(())
    }
}
