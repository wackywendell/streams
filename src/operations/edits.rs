//! Pure record-local edit rules for format-6 Streams and Sessions.
//!
//! One request is `{"changes": {...}, "before": {...}?}`.
//! - Without `before`, every value in `changes` must be complete and overwrites the field.
//! - With `before`, keys must match `changes` (at top level and inside `references`), each
//!   value merges onto `before` via modified RFC 7386 (explicit `null` sets the member to
//!   `null`, absent keeps `before`), and each unit writes only when the saved value equals
//!   `before`.
//! - `not_written` lists every requested unit whose saved value after apply differs from the
//!   requested merged value, with its current saved value (`null` for an absent reference).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::codec::{object, require_object};
use crate::model::{
    Goal, ReferenceEntry, Session, SessionDirectory, Status, Stream, TextPair, Title,
    ValidationError, check_token,
};

pub const STREAM_EDITABLE_FIELDS: &[&str] = &["title", "goal"];
pub const SESSION_EDITABLE_FIELDS: &[&str] = &[
    "title",
    "stream_id",
    "status",
    "recap",
    "next",
    "directory",
    "references",
];

/// Parsed edit envelope (`changes` plus optional `before`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditRequest {
    pub changes: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<Map<String, Value>>,
}

impl EditRequest {
    pub fn parse(value: &Value) -> Result<Self, ValidationError> {
        let root = require_object(value, "edit request")?;
        for key in root.keys() {
            if !matches!(key.as_str(), "changes" | "before") {
                return Err(ValidationError::new(format!(
                    "unknown edit request field {key:?}; expected only \"changes\" and optional \"before\""
                )));
            }
        }
        let changes_val = root
            .get("changes")
            .ok_or_else(|| ValidationError::new("edit request requires \"changes\""))?;
        let changes = require_object(changes_val, "edit changes")?.clone();
        let before = match root.get("before") {
            None | Some(Value::Null) => None,
            Some(before_val) => Some(require_object(before_val, "edit before")?.clone()),
        };
        Ok(Self { changes, before })
    }
}

/// Pure result of applying an edit to one Stream or Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditOutcome<T> {
    pub after: T,
    pub not_written: Map<String, Value>,
    pub changed: bool,
    /// Set when `stream_id` was changed on a Session and the caller must verify the destination Stream.
    pub target_stream_id: Option<String>,
}

/// Modified RFC 7386 merge: objects merge member by member, absent members keep `before`,
/// and `null` sets the member to `null` (never deletes the key).
pub fn merge_value(before: &Value, change: &Value) -> Value {
    match (before, change) {
        (Value::Object(before_map), Value::Object(change_map)) => {
            let mut merged = before_map.clone();
            for (key, change_val) in change_map {
                let next = match merged.get(key) {
                    Some(before_val) => merge_value(before_val, change_val),
                    None => change_val.clone(),
                };
                merged.insert(key.clone(), next);
            }
            Value::Object(merged)
        }
        _ => change.clone(),
    }
}

fn check_before_keys(
    changes: &Map<String, Value>,
    before: &Map<String, Value>,
    scope: &str,
) -> Result<(), ValidationError> {
    let change_keys: BTreeSet<&str> = changes.keys().map(String::as_str).collect();
    let before_keys: BTreeSet<&str> = before.keys().map(String::as_str).collect();
    if change_keys != before_keys {
        let extra_before: Vec<_> = before_keys.difference(&change_keys).copied().collect();
        let missing_before: Vec<_> = change_keys.difference(&before_keys).copied().collect();
        return Err(ValidationError::new(format!(
            "{scope} \"before\" keys must match \"changes\" keys (missing in before: {missing_before:?}, extra in before: {extra_before:?})"
        )));
    }
    Ok(())
}

fn decode_strict<T: serde::de::DeserializeOwned>(
    value: &Value,
    field: &str,
) -> Result<T, ValidationError> {
    object::<T, _>(value.clone())
        .map_err(|error| ValidationError::new(format!("invalid value for {field}: {error}")))
}

fn decode_optional_strict<T: serde::de::DeserializeOwned>(
    value: &Value,
    field: &str,
) -> Result<Option<T>, ValidationError> {
    if value.is_null() {
        Ok(None)
    } else {
        decode_strict::<T>(value, field).map(Some)
    }
}

fn target_value(
    changes: &Map<String, Value>,
    before: Option<&Map<String, Value>>,
    field: &str,
) -> Value {
    let change = &changes[field];
    match before {
        Some(before_map) => merge_value(&before_map[field], change),
        None => change.clone(),
    }
}

/// Apply an edit request to a Stream.
pub fn apply_stream(
    current: &Stream,
    request: &EditRequest,
    classify_field: impl Fn(&str) -> Result<(), ValidationError>,
) -> Result<EditOutcome<Stream>, ValidationError> {
    for key in request.changes.keys() {
        classify_field(key)?;
    }
    if let Some(before) = &request.before {
        check_before_keys(&request.changes, before, "stream edit")?;
    }

    // Phase 1: merge & validate every requested unit before mutating anything.
    let mut desired_title: Option<(Title, Value)> = None;
    let mut desired_goal: Option<(Option<Goal>, Value)> = None;

    for key in request.changes.keys() {
        let merged = target_value(&request.changes, request.before.as_ref(), key);
        match key.as_str() {
            "title" => {
                let title: Title = decode_strict(&merged, "title")?;
                title.validate("stream.title")?;
                let canonical = serde_json::to_value(&title).expect("title json");
                desired_title = Some((title, canonical));
            }
            "goal" => {
                let goal: Option<Goal> = decode_optional_strict(&merged, "goal")?;
                if let Some(g) = &goal {
                    g.validate()?;
                }
                let canonical = serde_json::to_value(&goal).expect("goal json");
                desired_goal = Some((goal, canonical));
            }
            other => {
                return Err(ValidationError::new(format!(
                    "Stream field {other:?} is not editable"
                )));
            }
        }
    }

    // Phase 2: compare against current record and apply admitted units.
    let mut after = current.clone();
    let mut not_written = Map::new();

    if let Some((title, desired_val)) = desired_title {
        let current_val = serde_json::to_value(&current.title).expect("title json");
        let admit = request
            .before
            .as_ref()
            .is_none_or(|b| current_val == b["title"]);
        if admit {
            after.title = title;
        }
        let final_val = serde_json::to_value(&after.title).expect("title json");
        if final_val != desired_val {
            not_written.insert("title".into(), final_val);
        }
    }

    if let Some((goal, desired_val)) = desired_goal {
        let current_val = serde_json::to_value(&current.goal).expect("goal json");
        let admit = request
            .before
            .as_ref()
            .is_none_or(|b| current_val == b["goal"]);
        if admit {
            after.goal = goal;
        }
        let final_val = serde_json::to_value(&after.goal).expect("goal json");
        if final_val != desired_val {
            not_written.insert("goal".into(), final_val);
        }
    }

    after.validate()?;
    let changed = after != *current;
    Ok(EditOutcome {
        after,
        not_written,
        changed,
        target_stream_id: None,
    })
}

/// Apply an edit request to a Session.
pub fn apply_session(
    current: &Session,
    request: &EditRequest,
    classify_field: impl Fn(&str) -> Result<(), ValidationError>,
) -> Result<EditOutcome<Session>, ValidationError> {
    for key in request.changes.keys() {
        classify_field(key)?;
    }
    if let Some(before) = &request.before {
        check_before_keys(&request.changes, before, "session edit")?;
    }

    struct DesiredReference {
        key: String,
        entry: ReferenceEntry,
        canonical: Value,
        before: Option<Value>,
    }

    // Phase 1: merge & validate every requested unit before mutating anything.
    let mut desired_title: Option<(Title, Value)> = None;
    let mut desired_stream_id: Option<(String, Value)> = None;
    let mut desired_status: Option<(Status, Value)> = None;
    let mut desired_recap: Option<(Option<TextPair>, Value)> = None;
    let mut desired_next: Option<(Option<TextPair>, Value)> = None;
    let mut desired_directory: Option<(Option<SessionDirectory>, Value)> = None;
    let mut desired_references: Option<Vec<DesiredReference>> = None;

    for key in request.changes.keys() {
        if key == "references" {
            let change_refs = require_object(&request.changes["references"], "references")?;
            let before_refs = match &request.before {
                Some(before_map) => {
                    let map = require_object(&before_map["references"], "before.references")?;
                    check_before_keys(change_refs, map, "references")?;
                    Some(map)
                }
                None => None,
            };
            let mut entries = Vec::with_capacity(change_refs.len());
            for (ref_key, change_entry) in change_refs {
                if change_entry.is_null() {
                    return Err(ValidationError::new(format!(
                        "reference {ref_key:?} cannot be null; set status to \"dropped\" instead"
                    )));
                }
                let before_entry = before_refs.map(|m| m[ref_key].clone());
                let merged = match &before_entry {
                    Some(before_val) => merge_value(before_val, change_entry),
                    None => change_entry.clone(),
                };
                let parsed: ReferenceEntry =
                    decode_strict(&merged, &format!("references[{ref_key:?}]"))?;
                parsed.validate_for_key(ref_key)?;
                let canonical = serde_json::to_value(&parsed).expect("reference entry json");
                entries.push(DesiredReference {
                    key: ref_key.clone(),
                    entry: parsed,
                    canonical,
                    before: before_entry,
                });
            }
            desired_references = Some(entries);
            continue;
        }

        let merged = target_value(&request.changes, request.before.as_ref(), key);
        match key.as_str() {
            "title" => {
                let title: Title = decode_strict(&merged, "title")?;
                title.validate("session.title")?;
                let canonical = serde_json::to_value(&title).expect("title json");
                desired_title = Some((title, canonical));
            }
            "stream_id" => {
                let stream_id: String = serde_json::from_value(merged.clone()).map_err(|e| {
                    ValidationError::new(format!("invalid value for stream_id: {e}"))
                })?;
                check_token(&stream_id, "session.stream_id")?;
                let canonical = Value::String(stream_id.clone());
                desired_stream_id = Some((stream_id, canonical));
            }
            "status" => {
                let status: Status = serde_json::from_value(merged.clone())
                    .map_err(|e| ValidationError::new(format!("invalid value for status: {e}")))?;
                let canonical = serde_json::to_value(status).expect("status json");
                desired_status = Some((status, canonical));
            }
            "recap" => {
                let pair: Option<TextPair> = decode_optional_strict(&merged, "recap")?;
                if let Some(p) = &pair {
                    p.validate("session.recap")?;
                }
                let canonical = serde_json::to_value(&pair).expect("recap json");
                desired_recap = Some((pair, canonical));
            }
            "next" => {
                let pair: Option<TextPair> = decode_optional_strict(&merged, "next")?;
                if let Some(p) = &pair {
                    p.validate("session.next")?;
                }
                let canonical = serde_json::to_value(&pair).expect("next json");
                desired_next = Some((pair, canonical));
            }
            "directory" => {
                let dir: Option<SessionDirectory> = decode_optional_strict(&merged, "directory")?;
                if let Some(d) = &dir {
                    d.validate()?;
                }
                let canonical = serde_json::to_value(&dir).expect("directory json");
                desired_directory = Some((dir, canonical));
            }
            other => {
                return Err(ValidationError::new(format!(
                    "Session field {other:?} is not editable"
                )));
            }
        }
    }

    // Phase 2: compare against current record and apply admitted units.
    let mut after = current.clone();
    let mut not_written = Map::new();
    let mut target_stream_id = None;

    if let Some((title, desired_val)) = desired_title {
        let current_val = serde_json::to_value(&current.title).expect("title json");
        if request
            .before
            .as_ref()
            .is_none_or(|b| current_val == b["title"])
        {
            after.title = title;
        }
        let final_val = serde_json::to_value(&after.title).expect("title json");
        if final_val != desired_val {
            not_written.insert("title".into(), final_val);
        }
    }

    if let Some((stream_id, desired_val)) = desired_stream_id {
        let current_val = Value::String(current.stream_id.clone());
        if request
            .before
            .as_ref()
            .is_none_or(|b| current_val == b["stream_id"])
        {
            if after.stream_id != stream_id {
                target_stream_id = Some(stream_id.clone());
            }
            after.stream_id = stream_id;
        }
        let final_val = Value::String(after.stream_id.clone());
        if final_val != desired_val {
            not_written.insert("stream_id".into(), final_val);
        }
    }

    if let Some((status, desired_val)) = desired_status {
        let current_val = serde_json::to_value(current.status).expect("status json");
        if request
            .before
            .as_ref()
            .is_none_or(|b| current_val == b["status"])
        {
            after.status = status;
        }
        let final_val = serde_json::to_value(after.status).expect("status json");
        if final_val != desired_val {
            not_written.insert("status".into(), final_val);
        }
    }

    if let Some((recap, desired_val)) = desired_recap {
        let current_val = serde_json::to_value(&current.recap).expect("recap json");
        if request
            .before
            .as_ref()
            .is_none_or(|b| current_val == b["recap"])
        {
            after.recap = recap;
        }
        let final_val = serde_json::to_value(&after.recap).expect("recap json");
        if final_val != desired_val {
            not_written.insert("recap".into(), final_val);
        }
    }

    if let Some((next, desired_val)) = desired_next {
        let current_val = serde_json::to_value(&current.next).expect("next json");
        if request
            .before
            .as_ref()
            .is_none_or(|b| current_val == b["next"])
        {
            after.next = next;
        }
        let final_val = serde_json::to_value(&after.next).expect("next json");
        if final_val != desired_val {
            not_written.insert("next".into(), final_val);
        }
    }

    if let Some((directory, desired_val)) = desired_directory {
        let current_val = serde_json::to_value(&current.directory).expect("directory json");
        if request
            .before
            .as_ref()
            .is_none_or(|b| current_val == b["directory"])
        {
            after.directory = directory;
        }
        let final_val = serde_json::to_value(&after.directory).expect("directory json");
        if final_val != desired_val {
            not_written.insert("directory".into(), final_val);
        }
    }

    if let Some(entries) = desired_references {
        let mut ref_not_written = Map::new();
        for DesiredReference {
            key: ref_key,
            entry: parsed,
            canonical: desired_val,
            before: before_entry,
        } in entries
        {
            let current_val = match current.references.get(&ref_key) {
                Some(entry) => serde_json::to_value(entry).expect("reference json"),
                None => Value::Null,
            };
            let admit = match &before_entry {
                Some(expected) => current_val == *expected,
                None => true,
            };
            if admit {
                after.references.insert(ref_key.clone(), parsed);
            }
            let final_val = match after.references.get(&ref_key) {
                Some(entry) => serde_json::to_value(entry).expect("reference json"),
                None => Value::Null,
            };
            if final_val != desired_val {
                ref_not_written.insert(ref_key, final_val);
            }
        }
        if !ref_not_written.is_empty() {
            not_written.insert("references".into(), Value::Object(ref_not_written));
        }
    }

    after.validate()?;
    let changed = after != *current;
    Ok(EditOutcome {
        after,
        not_written,
        changed,
        target_stream_id,
    })
}
