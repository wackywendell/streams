//! Original intent in a versioned commit message, independent of commit hashes.

use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

use crate::codec::{record_from_data, record_to_data};
use crate::model::{RecordKind, ValidationError, check_text};
use crate::operations::{Actor, Operation};

pub const PREFIX: &str = "Streams operation v1\n\n";
pub const BOOTSTRAP: &str = "Streams catalog v1\n";

pub use crate::codec::load_json;

/// Transport provenance kept outside operation identity and reconciliation.
pub type Origin = BTreeMap<String, String>;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedRecord {
    kind: RecordKind,
    record: Value,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationEnvelope {
    version: u64,
    id: String,
    kind: RecordKind,
    #[serde(deserialize_with = "Option::deserialize")]
    before: Option<Value>,
    after: Value,
    actor: Actor,
    #[serde(deserialize_with = "Option::deserialize")]
    ownership_token: Option<String>,
    origin: Origin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    inputs: Vec<CapturedRecord>,
}

/// Emit canonical versioned intent with separately supplied transport provenance.
pub fn encode_operation(
    operation: &Operation,
    origin: Option<&Origin>,
) -> Result<String, ValidationError> {
    let envelope = OperationEnvelope {
        version: 1,
        id: operation.id.clone(),
        kind: operation.after.kind(),
        before: operation.before.as_ref().map(record_to_data),
        after: record_to_data(&operation.after),
        actor: operation.actor,
        ownership_token: operation.ownership_token.clone(),
        origin: origin.cloned().unwrap_or_default(),
        inputs: operation
            .inputs
            .iter()
            .map(|record| CapturedRecord {
                kind: record.kind(),
                record: record_to_data(record),
            })
            .collect(),
    };
    let json = serde_json::to_value(envelope)
        .and_then(|value| serde_json::to_string(&value))
        .map_err(|e| ValidationError::new(format!("cannot encode operation: {e}")))?;
    Ok(format!("{PREFIX}{json}\n"))
}

/// Hash intent with an empty origin so transport provenance cannot change retry identity.
pub fn operation_digest(operation: &Operation) -> String {
    let encoded = encode_operation(operation, None).expect("valid operation encodes");
    let mut hasher = Sha256::new();
    hasher.update(encoded.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Validate fingerprint syntax only, not the existence or intent of an operation.
pub fn check_fingerprints(fingerprints: &BTreeMap<String, String>) -> Result<(), ValidationError> {
    for (id, digest) in fingerprints {
        check_text(id, "operation ID", false)?;
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(ValidationError::new(
                "operation fingerprint must be a lowercase SHA256 digest",
            ));
        }
    }
    Ok(())
}

pub fn decode_fingerprints(value: &Value) -> Result<BTreeMap<String, String>, ValidationError> {
    if !value.is_object() {
        return Err(ValidationError::new("fingerprints must be an object"));
    }
    let fingerprints = serde_json::from_value(value.clone()).map_err(|_| {
        ValidationError::new("operation fingerprint must be a lowercase SHA256 digest")
    })?;
    check_fingerprints(&fingerprints)?;
    Ok(fingerprints)
}

/// Strictly decode the envelope and construct an operation through its validation boundary.
pub fn decode_operation(message: &str) -> Result<(Operation, Origin), ValidationError> {
    let Some(body) = message.strip_prefix(PREFIX) else {
        return Err(ValidationError::new("unsupported operation commit message"));
    };
    let data: OperationEnvelope = serde_json::from_value(load_json(body)?)
        .map_err(|e| ValidationError::new(format!("invalid operation envelope: {e}")))?;
    if data.version != 1 {
        return Err(ValidationError::new(
            "unsupported operation envelope version",
        ));
    }
    let inputs = data
        .inputs
        .into_iter()
        .map(|item| record_from_data(item.kind, &item.record))
        .collect::<Result<Vec<_>, _>>()?;
    let before = data
        .before
        .as_ref()
        .map(|record| record_from_data(data.kind, record))
        .transpose()?;
    let after = record_from_data(data.kind, &data.after)?;
    let operation = Operation::new(
        data.id,
        before,
        after,
        data.actor,
        data.ownership_token,
        inputs,
    )?;
    Ok((operation, data.origin))
}
