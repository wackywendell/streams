//! Original intent in a versioned commit message, independent of commit hashes.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::codec::{record_from_data, record_to_data};
use crate::model::{ValidationError, check_text};
use crate::operations::Operation;

pub const PREFIX: &str = "Streams operation v1\n\n";
pub const BOOTSTRAP: &str = "Streams catalog v1\n";

struct StrictJsonSeed;

impl<'de> DeserializeSeed<'de> for StrictJsonSeed {
    type Value = Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct StrictVisitor;

        impl<'de> Visitor<'de> for StrictVisitor {
            type Value = Value;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("any valid JSON value without duplicate object keys")
            }

            fn visit_bool<E>(self, value: bool) -> Result<Value, E> {
                Ok(Value::Bool(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Value, E> {
                Ok(Value::Number(value.into()))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Value, E> {
                Ok(Value::Number(value.into()))
            }

            fn visit_f64<E>(self, value: f64) -> Result<Value, E>
            where
                E: de::Error,
            {
                serde_json::Number::from_f64(value)
                    .map(Value::Number)
                    .ok_or_else(|| de::Error::custom("non-finite float in JSON"))
            }

            fn visit_str<E>(self, value: &str) -> Result<Value, E> {
                Ok(Value::String(value.to_owned()))
            }

            fn visit_string<E>(self, value: String) -> Result<Value, E> {
                Ok(Value::String(value))
            }

            fn visit_unit<E>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut vec = Vec::new();
                while let Some(elem) = seq.next_element_seed(StrictJsonSeed)? {
                    vec.push(elem);
                }
                Ok(Value::Array(vec))
            }

            fn visit_map<A>(self, mut map: A) -> Result<Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut values = Map::new();
                while let Some(key) = map.next_key::<String>()? {
                    if values.contains_key(&key) {
                        return Err(de::Error::custom(format!("duplicate JSON field: {key}")));
                    }
                    let val = map.next_value_seed(StrictJsonSeed)?;
                    values.insert(key, val);
                }
                Ok(Value::Object(values))
            }
        }

        deserializer.deserialize_any(StrictVisitor)
    }
}

pub fn load_json(value: &str) -> Result<Value, ValidationError> {
    let mut de = serde_json::Deserializer::from_str(value);
    let parsed = StrictJsonSeed
        .deserialize(&mut de)
        .map_err(|e| ValidationError::new(format!("invalid JSON: {e}")))?;
    de.end()
        .map_err(|e| ValidationError::new(format!("invalid JSON: {e}")))?;
    Ok(parsed)
}

pub type Origin = BTreeMap<String, String>;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedRecord {
    kind: String,
    record: Value,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationEnvelope {
    version: u64,
    id: String,
    kind: String,
    #[serde(deserialize_with = "Option::deserialize")]
    before: Option<Value>,
    after: Value,
    actor: String,
    #[serde(deserialize_with = "Option::deserialize")]
    ownership_token: Option<String>,
    origin: Origin,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    inputs: Vec<CapturedRecord>,
}

pub fn encode_operation(
    operation: &Operation,
    origin: Option<&Origin>,
) -> Result<String, ValidationError> {
    let envelope = OperationEnvelope {
        version: 1,
        id: operation.id.clone(),
        kind: operation.after.kind_str().to_string(),
        before: operation.before.as_ref().map(record_to_data),
        after: record_to_data(&operation.after),
        actor: operation.actor.clone(),
        ownership_token: operation.ownership_token.clone(),
        origin: origin.cloned().unwrap_or_default(),
        inputs: operation
            .inputs
            .iter()
            .map(|record| CapturedRecord {
                kind: record.kind_str().to_string(),
                record: record_to_data(record),
            })
            .collect(),
    };
    let json = serde_json::to_value(envelope)
        .and_then(|value| serde_json::to_string(&value))
        .map_err(|e| ValidationError::new(format!("cannot encode operation: {e}")))?;
    Ok(format!("{PREFIX}{json}\n"))
}

pub fn operation_digest(operation: &Operation) -> String {
    let encoded = encode_operation(operation, None).expect("valid operation encodes");
    let mut hasher = Sha256::new();
    hasher.update(encoded.as_bytes());
    format!("{:x}", hasher.finalize())
}

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
        .map(|item| record_from_data(&item.kind, &item.record))
        .collect::<Result<Vec<_>, _>>()?;
    let before = data
        .before
        .as_ref()
        .map(|record| record_from_data(&data.kind, record))
        .transpose()?;
    let after = record_from_data(&data.kind, &data.after)?;
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
