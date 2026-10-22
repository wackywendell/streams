//! Durable recovery markers, retry receipts and transport scheduling state.
//!
//! Reads reject duplicate JSON keys and unsupported versions; typed readers also
//! reject unknown fields and invalid phase-specific data. Missing records may have
//! defaults, but corrupt or unreadable records must not be treated as absent.
//!
//! Methods do not acquire a catalog lock. Callers must serialize read/modify/write
//! sequences and stop effects when state needs repair. A failed write or clear may
//! already have changed disk state; do not assume rollback or retry blindly.

#[cfg(test)]
#[path = "state_tests.rs"]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

use crate::codec::required_nullable;
use crate::model::check_text;
use crate::operations::Operation;
use crate::storage::{Journal, StorageError};

use super::envelope::{check_fingerprints, operation_digest};
use super::{Oid, Seconds};

/// State cannot safely support further effects without inspection or repair.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct RepairRequired(pub String);

impl RepairRequired {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

impl From<StorageError> for RepairRequired {
    fn from(error: StorageError) -> Self {
        Self::new(error.to_string())
    }
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
    pub confirmed: Option<Oid>,
    #[serde(deserialize_with = "required_nullable")]
    pub last_sync: Option<Seconds>,
    #[serde(deserialize_with = "required_nullable")]
    pub last_push: Option<Seconds>,
    pub retry_after: Seconds,
    #[serde(deserialize_with = "required_nullable")]
    pub fetched_at: Option<Seconds>,
    /// Candidate whose confirmation established `last_push`, so recovery records
    /// that baseline once rather than advancing it on every retry.
    #[serde(deserialize_with = "required_nullable")]
    pub last_confirmed_attempt: Option<Oid>,
    pub refresh_retry_after: Seconds,
    pub refresh_error: String,
}

impl Default for SyncMetadata {
    fn default() -> Self {
        Self {
            confirmed: None,
            last_sync: None,
            last_push: None,
            retry_after: Seconds::ZERO,
            fetched_at: None,
            last_confirmed_attempt: None,
            refresh_retry_after: Seconds::ZERO,
            refresh_error: String::new(),
        }
    }
}

impl SyncMetadata {
    /// Requires a scheduling baseline for a recorded confirmation identity.
    pub fn validate(&self) -> Result<(), String> {
        if self.last_confirmed_attempt.is_some() && self.last_push.is_none() {
            return Err("confirmation identity requires its scheduling baseline".to_string());
        }
        Ok(())
    }
}

/// Stored transport selection; `None` for `remote` means local-only operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "GitConfigData")]
pub struct GitConfig {
    remote: Option<String>,
    branch: String,
    host: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GitConfigData {
    #[serde(deserialize_with = "required_nullable")]
    remote: Option<String>,
    branch: String,
    host: String,
}

impl TryFrom<GitConfigData> for GitConfig {
    type Error = String;

    fn try_from(data: GitConfigData) -> Result<Self, Self::Error> {
        Self::new(data.remote, data.branch, data.host)
    }
}

impl GitConfig {
    pub fn remote(&self) -> Option<&str> {
        self.remote.as_deref()
    }

    pub fn branch(&self) -> &str {
        &self.branch
    }

    pub fn host(&self) -> &str {
        &self.host
    }

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
/// observations before treating the attempt as published.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PublicationAttempt {
    base: Option<Oid>,
    candidate: Oid,
    local_tip: Oid,
    branch: String,
    remote: String,
    operations: Vec<String>,
    confirmed_at: Option<Seconds>,
}

impl PublicationAttempt {
    pub fn base(&self) -> Option<&Oid> {
        self.base.as_ref()
    }

    pub fn candidate(&self) -> &Oid {
        &self.candidate
    }

    pub fn local_tip(&self) -> &Oid {
        &self.local_tip
    }

    pub fn branch(&self) -> &str {
        &self.branch
    }

    pub fn remote(&self) -> &str {
        &self.remote
    }

    pub fn operations(&self) -> &[String] {
        &self.operations
    }

    pub fn confirmed_at(&self) -> Option<Seconds> {
        self.confirmed_at
    }

    /// Records a finite confirmation timestamp without establishing publication evidence.
    pub fn with_confirmed_at(mut self, confirmed_at: Option<Seconds>) -> Self {
        self.confirmed_at = confirmed_at;
        self
    }

    /// Checks field syntax and unique operation IDs, without inspecting Git history.
    pub fn new(
        base: Option<Oid>,
        candidate: Oid,
        local_tip: Oid,
        branch: impl Into<String>,
        remote: impl Into<String>,
        operations: Vec<String>,
        confirmed_at: Option<Seconds>,
    ) -> Result<Self, String> {
        let branch = branch.into();
        let remote = remote.into();
        check_text(&branch, "branch", false).map_err(|e| e.to_string())?;
        check_text(&remote, "remote", false).map_err(|e| e.to_string())?;
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
    pub fn adopting(&self, old: Oid, new: Oid) -> AdoptingAttempt {
        AdoptingAttempt::new(self.clone(), old, new)
    }
}

/// An attempt plus the old and new tips recorded for local adoption recovery.
///
/// These are transition facts to check against Git, not proof that publication or
/// adoption succeeded. Validated tips do not establish their relationship.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AdoptingAttempt {
    #[serde(flatten)]
    attempt: PublicationAttempt,
    adoption_from: Oid,
    adoption_to: Oid,
}

impl AdoptingAttempt {
    pub fn attempt(&self) -> &PublicationAttempt {
        &self.attempt
    }

    pub fn adoption_from(&self) -> &Oid {
        &self.adoption_from
    }

    pub fn adoption_to(&self) -> &Oid {
        &self.adoption_to
    }

    /// Records validated tips without establishing their relationship to the attempt.
    pub fn new(attempt: PublicationAttempt, adoption_from: Oid, adoption_to: Oid) -> Self {
        Self {
            attempt,
            adoption_from,
            adoption_to,
        }
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
#[derive(strum::EnumString, strum::VariantNames)]
#[strum(serialize_all = "lowercase")]
enum PublicationPhase {
    Attempting,
    Adopting,
}

crate::codec::deserialize_string_enum!(PublicationPhase);

fn present_oid<'de, D>(d: D) -> Result<Option<Oid>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Oid::deserialize(d).map(Some)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicationData {
    phase: PublicationPhase,
    #[serde(deserialize_with = "required_nullable")]
    base: Option<Oid>,
    candidate: Oid,
    local_tip: Oid,
    branch: String,
    remote: String,
    operations: Vec<String>,
    #[serde(deserialize_with = "required_nullable")]
    confirmed_at: Option<Seconds>,
    #[serde(default, deserialize_with = "present_oid")]
    adoption_from: Option<Oid>,
    #[serde(default, deserialize_with = "present_oid")]
    adoption_to: Option<Oid>,
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
            (PublicationPhase::Adopting, Some(old), Some(new)) => Ok(PublicationMarker::Adopting(
                AdoptingAttempt::new(attempt, old, new),
            )),
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

/// Typed recovery state borrowed from the owning transaction's durable objects.
/// Raw names are trusted bookkeeping keys. Reads validate versions, while typed
/// accessors also validate schemas. Callers must hold the owning store's lock.
pub struct State<'tx> {
    journal: &'tx dyn Journal,
    hook: Option<StateHook>,
}

impl std::fmt::Debug for State<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State").finish_non_exhaustive()
    }
}

impl<'tx> State<'tx> {
    /// Borrows objects within the owning catalog transaction.
    pub fn from_journal(journal: &'tx dyn Journal) -> Self {
        Self {
            journal,
            hook: None,
        }
    }

    pub(crate) fn with_optional_hook(mut self, hook: Option<StateHook>) -> Self {
        self.hook = hook;
        self
    }

    /// Installs a hook that may fail an operation before its state effects begin.
    pub fn with_hook(mut self, hook: StateHook) -> Self {
        self.set_hook(hook);
        self
    }

    pub(crate) fn set_hook(&mut self, hook: StateHook) {
        self.hook = Some(hook);
    }

    fn run_hook(&self, op: &str, name: &str) -> Result<(), RepairRequired> {
        if let Some(ref hook) = self.hook {
            let mut guard = hook.lock().unwrap();
            guard(op, name)?;
        }
        Ok(())
    }

    /// Reads an object with a supported version; typed readers check its other fields.
    pub fn read(&self, name: &str) -> Result<Option<Map<String, Value>>, RepairRequired> {
        let Some(obj) = self
            .journal
            .read(name)
            .map_err(|e| RepairRequired::new(e.to_string()))?
        else {
            return Ok(None);
        };
        match obj.get("version") {
            Some(Value::Number(n)) if n.as_i64() == Some(1) => Ok(Some(obj)),
            _ => Err(RepairRequired::new(format!(
                "cannot read {}: unsupported transport state version",
                self.journal.describe(name)
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
                    RepairRequired::new(format!("invalid {}: {e}", self.journal.describe(name)))
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
        let metadata = self
            .read_record::<SyncMetadata>("sync.json")?
            .unwrap_or_default();
        metadata.validate().map_err(RepairRequired::new)?;
        Ok(metadata)
    }

    /// Validates and persists scheduling state without deriving confirmation evidence.
    pub fn write_metadata(&self, metadata: &SyncMetadata) -> Result<(), RepairRequired> {
        metadata.validate().map_err(RepairRequired::new)?;
        self.write_record("sync.json", metadata)
    }

    /// Loads the transport selection; absence is distinct from invalid configuration.
    pub fn config(&self) -> Result<Option<GitConfig>, RepairRequired> {
        self.read_record("config.json")
    }

    /// Persists the validated transport selection.
    pub fn write_config(&self, config: &GitConfig) -> Result<(), RepairRequired> {
        self.write_record("config.json", config)
    }

    /// Loads a recovery marker, rejecting unsupported phases or mismatched adoption fields.
    /// The returned facts still need verification against Git before recovery effects.
    pub fn publication(&self) -> Result<Option<PublicationMarker>, RepairRequired> {
        self.read_record::<PublicationData>("publication.json")?
            .map(PublicationData::into_marker)
            .transpose()
    }

    /// Persists the validated recovery phase.
    pub fn write_publication(&self, marker: &PublicationMarker) -> Result<(), RepairRequired> {
        self.write_record("publication.json", marker)
    }

    /// Establishes the exact adoption fence without rewriting an identical marker.
    pub(crate) fn fence_adoption(&self, adoption: &AdoptingAttempt) -> Result<(), RepairRequired> {
        let marker = PublicationMarker::Adopting(adoption.clone());
        if self.publication()?.as_ref() != Some(&marker) {
            self.write_publication(&marker)?;
        }
        Ok(())
    }

    /// Writes a version-1 object; failure does not imply the old object remains.
    pub fn write(&self, name: &str, mut data: Map<String, Value>) -> Result<(), RepairRequired> {
        self.run_hook("write", name)?;
        data.insert("version".into(), Value::Number(1.into()));
        self.journal
            .write(name, data)
            .map_err(|e| RepairRequired::new(e.to_string()))
    }

    /// Removes a marker if present; failure can leave its durability uncertain.
    pub fn clear(&self, name: &str) -> Result<(), RepairRequired> {
        self.run_hook("clear", name)?;
        self.journal
            .clear(name)
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
        if let Some(digest) = fps.get(operation.id()) {
            if digest != &operation_digest(operation) {
                return Err(RepairRequired::new(format!(
                    "operation ID reused with different intent: {}",
                    operation.id()
                )));
            }
            return Ok(true);
        }
        Ok(false)
    }

    /// Persists an intent receipt, rejecting any prior use of the ID for another intent.
    pub fn remember(&self, operation: &Operation) -> Result<(), RepairRequired> {
        self.run_hook("remember", operation.id())?;
        let mut incoming = BTreeMap::new();
        incoming.insert(operation.id().to_owned(), operation_digest(operation));
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
                self.journal.describe("save.json")
            )));
        }
        Ok(())
    }

    /// Rejects interrupted saves, invalid receipts or bookkeeping incompatible with local mode.
    pub fn check_local(&self) -> Result<(), RepairRequired> {
        self.check_save()?;
        let allowed: BTreeSet<&str> =
            ["cli.json", "local.json", "receipts.json", "save.json"].into();
        for name in self
            .journal
            .names()
            .map_err(|e| RepairRequired::new(e.to_string()))?
        {
            if !allowed.contains(name.as_str()) {
                return Err(RepairRequired::new(
                    "unexpected local catalog bookkeeping; strategy cannot be switched",
                ));
            }
        }
        self.fingerprints()?;
        Ok(())
    }
}
