//! Snapshot roots retain original creation evidence instead of inventing history.
//!
//! A baseline starts operation history from existing records. Its original creation
//! envelopes and intent fingerprints establish each stream's ownership authority;
//! they need not describe the stream's current, possibly edited snapshot value.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::codec::require_object;
use crate::model::{Catalog, ValidationError, check_text};
use crate::operations::Authority;
use crate::yaml_store::YamlStore;

use super::envelope::{
    check_fingerprints, decode_operation, encode_operation, load_json, operation_digest,
};

/// Commit-message prefix identifying the supported baseline format.
pub const BASELINE: &str = "Streams catalog baseline v1\n";

/// Retained creation envelopes by stream ID and intent digests by operation ID.
///
/// Construction checks field shapes only. [`decode_baseline`] checks this evidence
/// against the snapshot before deriving ownership authorities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Baseline {
    pub host: String,
    pub creations: BTreeMap<String, String>,
    pub fingerprints: BTreeMap<String, String>,
}

impl Baseline {
    /// Checks nonempty identifiers and envelope text, and fingerprint syntax.
    pub fn new(
        host: impl Into<String>,
        creations: BTreeMap<String, String>,
        fingerprints: BTreeMap<String, String>,
    ) -> Result<Self, ValidationError> {
        let host = host.into();
        check_text(&host, "baseline host", false)?;
        for (id, envelope) in &creations {
            check_text(id, "stream ID", false)?;
            check_text(envelope, "creation envelope", false)?;
        }
        check_fingerprints(&fingerprints)?;
        Ok(Self {
            host,
            creations,
            fingerprints,
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BaselineEnvelope {
    version: u64,
    host: String,
    creations: BTreeMap<String, String>,
    fingerprints: BTreeMap<String, String>,
}

/// Encodes the versioned commit message; does not verify snapshot authority.
pub fn encode_baseline(baseline: &Baseline) -> Result<String, ValidationError> {
    let envelope = BaselineEnvelope {
        version: 1,
        host: baseline.host.clone(),
        creations: baseline.creations.clone(),
        fingerprints: baseline.fingerprints.clone(),
    };
    let json = serde_json::to_value(envelope)
        .and_then(|value| serde_json::to_string(&value))
        .map_err(|e| ValidationError::new(format!("cannot encode baseline: {e}")))?;
    Ok(format!("{BASELINE}{json}\n"))
}

/// Validates a baseline message and its snapshot, then derives stream authorities.
///
/// Rejects unsupported versions, duplicate JSON keys, unknown fields, mismatched
/// producers, and missing or noncanonical original creations. Each creation must
/// have a unique operation ID whose fingerprint matches its original intent.
pub fn decode_baseline(
    message: &str,
    files: &BTreeMap<String, Vec<u8>>,
) -> Result<(Baseline, Catalog, BTreeMap<String, Authority>), ValidationError> {
    let Some(body) = message.strip_prefix(BASELINE) else {
        return Err(ValidationError::new("unsupported catalog baseline message"));
    };
    let value = load_json(body)?;
    require_object(&value, "catalog baseline")?;
    let data: BaselineEnvelope = serde_json::from_value(value)
        .map_err(|e| ValidationError::new(format!("invalid catalog baseline: {e}")))?;
    if data.version != 1 {
        return Err(ValidationError::new("unsupported catalog baseline version"));
    }
    let baseline = Baseline::new(data.host, data.creations, data.fingerprints)?;
    let catalog = YamlStore::decode_snapshot(files)
        .map_err(|e| ValidationError::new(e.to_string()))?
        .catalog;
    let creation_keys: BTreeSet<String> = baseline.creations.keys().cloned().collect();
    let stream_ids: BTreeSet<String> = catalog.streams().iter().map(|s| s.id.clone()).collect();
    if creation_keys != stream_ids {
        return Err(ValidationError::new(
            "baseline creations must exactly match snapshot streams",
        ));
    }
    if catalog
        .streams()
        .iter()
        .any(|s| s.owner_host_id != baseline.host)
        || catalog
            .sessions()
            .iter()
            .any(|s| s.host_id != baseline.host)
        || catalog.hosts().iter().any(|h| h.id != baseline.host)
    {
        return Err(ValidationError::new(
            "baseline snapshot producers must belong to its host",
        ));
    }
    let mut authorities = BTreeMap::new();
    let mut creation_ids = BTreeSet::new();
    for (id, envelope) in &baseline.creations {
        let (operation, origin) = decode_operation(envelope)?;
        if &encode_operation(&operation, Some(&origin))? != envelope {
            return Err(ValidationError::new(
                "baseline creation envelope must be canonical",
            ));
        }
        let Some((stream_id, authority)) = operation.created_authority() else {
            return Err(ValidationError::new(
                "baseline creation must create its indexed stream",
            ));
        };
        if &stream_id != id {
            return Err(ValidationError::new(
                "baseline creation must create its indexed stream",
            ));
        }
        if authority.host_id != baseline.host {
            return Err(ValidationError::new(
                "baseline creation owner must match its host",
            ));
        }
        if !creation_ids.insert(operation.id().to_owned()) {
            return Err(ValidationError::new(
                "duplicate baseline creation operation ID",
            ));
        }
        if baseline.fingerprints.get(operation.id()) != Some(&operation_digest(&operation)) {
            return Err(ValidationError::new(
                "baseline creation fingerprint does not match original intent",
            ));
        }
        authorities.insert(id.clone(), authority);
    }
    Ok((baseline, catalog, authorities))
}
