//! Durable transport state: parse supported records before making effects.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::model::check_text;
use crate::operations::Operation;

use super::envelope::{check_fingerprints, load_json, operation_digest};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RepairRequired(pub String);

impl RepairRequired {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitConfig {
    #[serde(deserialize_with = "required_nullable")]
    pub remote: Option<String>,
    pub branch: String,
    pub host: String,
}

impl GitConfig {
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

    pub fn adopting(
        &self,
        old: impl Into<String>,
        new: impl Into<String>,
    ) -> Result<AdoptingAttempt, String> {
        AdoptingAttempt::new(self.clone(), old.into(), new.into())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdoptingAttempt {
    #[serde(flatten)]
    pub attempt: PublicationAttempt,
    pub adoption_from: String,
    pub adoption_to: String,
}

impl AdoptingAttempt {
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

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "phase", rename_all = "lowercase")]
pub enum PublicationMarker {
    Attempting(PublicationAttempt),
    Adopting(AdoptingAttempt),
}

impl PublicationMarker {
    pub fn attempt(&self) -> &PublicationAttempt {
        match self {
            PublicationMarker::Attempting(a) => a,
            PublicationMarker::Adopting(a) => &a.attempt,
        }
    }
}

// Flat disk schema keeps phase-specific fields strict without Serde's
// flatten/deny_unknown_fields interaction. Semantic construction happens once.
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

pub type StateHook = Arc<Mutex<dyn FnMut(&str, &str) -> Result<(), RepairRequired> + Send>>;

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
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().join(".catalog-sync"),
            hook: None,
        }
    }

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

    pub fn metadata(&self) -> Result<SyncMetadata, RepairRequired> {
        self.read_record::<SyncMetadata>("sync.json")?
            .unwrap_or_default()
            .validated()
            .map_err(RepairRequired::new)
    }

    pub fn write_metadata(&self, metadata: &SyncMetadata) -> Result<(), RepairRequired> {
        metadata.clone().validated().map_err(RepairRequired::new)?;
        self.write_record("sync.json", metadata)
    }

    pub fn config(&self) -> Result<Option<GitConfig>, RepairRequired> {
        self.read_record::<GitConfig>("config.json")?
            .map(|c| GitConfig::new(c.remote, c.branch, c.host).map_err(RepairRequired::new))
            .transpose()
    }

    pub fn write_config(&self, config: &GitConfig) -> Result<(), RepairRequired> {
        GitConfig::new(config.remote.clone(), &config.branch, &config.host)
            .map_err(RepairRequired::new)?;
        self.write_record("config.json", config)
    }

    pub fn publication(&self) -> Result<Option<PublicationMarker>, RepairRequired> {
        self.read_record::<PublicationData>("publication.json")?
            .map(PublicationData::into_marker)
            .transpose()
    }

    pub fn write_publication(&self, marker: &PublicationMarker) -> Result<(), RepairRequired> {
        // Validate typed fields too: callers can construct or update transport DTOs.
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

    pub fn write(&self, name: &str, mut data: Map<String, Value>) -> Result<(), RepairRequired> {
        self.run_hook("write", name)?;
        self.ensure_directory()?;
        data.insert("version".into(), Value::Number(1.into()));
        let bytes = serde_json::to_vec(&Value::Object(data))
            .map_err(|e| RepairRequired::new(format!("cannot serialize {name}: {e}")))?;
        crate::yaml_store::atomic_write(&self.root.join(name), &bytes, &self.root)
            .map_err(|e| RepairRequired::new(e.to_string()))
    }

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

    pub fn fingerprints(&self) -> Result<BTreeMap<String, String>, RepairRequired> {
        let operations = self
            .read_record::<Receipts>("receipts.json")?
            .map(|r| r.operations)
            .unwrap_or_default();
        check_fingerprints(&operations)
            .map_err(|e| RepairRequired::new(format!("invalid operation retry receipts: {e}")))?;
        Ok(operations)
    }

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

    pub fn remember(&self, operation: &Operation) -> Result<(), RepairRequired> {
        self.run_hook("remember", &operation.id)?;
        let mut incoming = BTreeMap::new();
        incoming.insert(operation.id.clone(), operation_digest(operation));
        self.validate_fingerprints(&incoming, true)
    }

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

    pub fn check_save(&self) -> Result<(), RepairRequired> {
        if self.read("save.json")?.is_some() {
            return Err(RepairRequired::new(format!(
                "interrupted save; inspect {} before further writes",
                self.root.join("save.json").display()
            )));
        }
        Ok(())
    }

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
