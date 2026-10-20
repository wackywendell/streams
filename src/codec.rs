//! Strict physical field mapping shared by JSON-compatible encoders and YAML.

use std::collections::BTreeSet;
use std::fmt;

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

use serde_json::{Map, Value};

use crate::model::{Host, Record, RecordKind, Session, Stream, ValidationError};

pub fn object_fields(
    value: &Value,
    allowed: &[&str],
) -> Result<Map<String, Value>, ValidationError> {
    let Some(obj) = value.as_object() else {
        return Err(ValidationError::new("expected an object with string keys"));
    };
    let allowed_set: BTreeSet<&str> = allowed.iter().copied().collect();
    let unknown: Vec<&str> = obj
        .keys()
        .map(String::as_str)
        .filter(|k| !allowed_set.contains(k))
        .collect();
    if !unknown.is_empty() {
        return Err(ValidationError::new(format!(
            "unknown fields: {}",
            unknown.join(", ")
        )));
    }
    Ok(obj.clone())
}

/// Emit untagged persisted fields, not the derived serde representation of `Record`.
pub fn record_to_data(record: &Record) -> Value {
    match record {
        Record::Stream(s) => serde_json::to_value(s),
        Record::Session(s) => serde_json::to_value(s),
        Record::Host(h) => serde_json::to_value(h),
    }
    .expect("record fields are JSON-compatible")
}

/// Decode a record's wire fields and check its logical constraints before use.
pub fn record_from_data(kind: RecordKind, value: &Value) -> Result<Record, ValidationError> {
    let record = match kind {
        RecordKind::Stream => serde_json::from_value::<Stream>(value.clone()).map(Record::Stream),
        RecordKind::Session => {
            serde_json::from_value::<Session>(value.clone()).map(Record::Session)
        }
        RecordKind::Host => serde_json::from_value::<Host>(value.clone()).map(Record::Host),
    }
    .map_err(|error| ValidationError::new(error.to_string()))?;
    record.validated()
}

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

/// Parse JSON without silently accepting duplicate keys, including in nested objects.
pub fn load_json(value: &str) -> Result<Value, ValidationError> {
    let mut de = serde_json::Deserializer::from_str(value);
    let parsed = StrictJsonSeed
        .deserialize(&mut de)
        .map_err(|e| ValidationError::new(format!("invalid JSON: {e}")))?;
    de.end()
        .map_err(|e| ValidationError::new(format!("invalid JSON: {e}")))?;
    Ok(parsed)
}
