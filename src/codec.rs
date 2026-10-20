//! Strict physical field mapping shared by JSON-compatible encoders and YAML.

use std::collections::BTreeSet;

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
