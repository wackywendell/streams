//! Strict physical field mapping shared by JSON-compatible encoders and YAML.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use crate::model::{Host, Record, Session, Stream, ValidationError};

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

pub fn record_to_data(record: &Record) -> Value {
    match record {
        Record::Stream(s) => serde_json::to_value(s),
        Record::Session(s) => serde_json::to_value(s),
        Record::Host(h) => serde_json::to_value(h),
    }
    .expect("record fields are JSON-compatible")
}

pub fn record_from_data(kind: &str, value: &Value) -> Result<Record, ValidationError> {
    let record = match kind {
        "stream" => serde_json::from_value::<Stream>(value.clone()).map(Record::Stream),
        "session" => serde_json::from_value::<Session>(value.clone()).map(Record::Session),
        "host" => serde_json::from_value::<Host>(value.clone()).map(Record::Host),
        _ => return Err(ValidationError::new(format!("unknown record kind: {kind}"))),
    }
    .map_err(|error| ValidationError::new(error.to_string()))?;
    record.validated()
}
