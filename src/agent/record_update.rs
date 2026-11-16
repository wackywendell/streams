//! Versioned captured edits. Old authority-bearing requests are never adapted or replayed.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    codec::{load_json, objects, record_from_data, require_object},
    model::{NamingMode, Record, RecordKind, Session, TitleDefaults, ValidationError, check_text},
    operations::{
        Actor, agent_field_allowed,
        edits::{SessionEdit, StreamEdit},
        references::{
            AutomaticDocumentTarget, AutomaticReferenceTarget, AutomaticReferenceValue,
            ReferenceChange, ReferenceChanges,
        },
    },
    storage::records::{RecordAddress, RecordRequest},
};

#[derive(Debug, Clone)]
pub struct RecordUpdate {
    pub(crate) request: RecordRequest,
    pub(crate) edit: CapturedEdit,
    pub(crate) title_defaults: TitleDefaults,
}
#[derive(Debug, Clone)]
pub(crate) enum CapturedEdit {
    Session(SessionEdit),
    Stream(StreamEdit),
    References(ReferenceChanges),
}
impl RecordUpdate {
    pub fn operation_id(&self) -> &str {
        match &self.edit {
            CapturedEdit::Session(edit) => edit.operation_id(),
            CapturedEdit::Stream(edit) => edit.operation_id(),
            CapturedEdit::References(edit) => edit.operation_id(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    kind: RecordKind,
    record: Value,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    version: u64,
    catalog_source: String,
    operation_id: String,
    kind: RecordKind,
    actor: Actor,
    before: Value,
    changes: Map<String, Value>,
    title_default: NamingMode,
    #[serde(deserialize_with = "objects")]
    inputs: Vec<Input>,
}

fn exact_keys(
    object: &Map<String, Value>,
    required: &[&str],
    optional: &[&str],
) -> Result<(), ValidationError> {
    if required.iter().any(|key| !object.contains_key(*key))
        || object
            .keys()
            .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(ValidationError::new("invalid reference object properties"));
    }
    Ok(())
}

fn parse_reference_changes(
    actor: Actor,
    value: &Value,
) -> Result<Vec<ReferenceChange>, ValidationError> {
    let rows = value
        .as_array()
        .ok_or_else(|| ValidationError::new("reference_changes must be an array"))?;
    rows.iter()
        .map(|value| {
            let row = require_object(value, "reference change")?;
            let desired = row
                .get("desired")
                .ok_or_else(|| ValidationError::new("reference desired is required"))?;
            if desired.is_null() {
                exact_keys(row, &["key", "desired"], &[])?;
                if actor != Actor::User {
                    return Err(ValidationError::new(
                        "automatic reference deletion is invalid",
                    ));
                }
                let key = serde_json::from_value(row["key"].clone()).map_err(|error| {
                    ValidationError::new(format!("invalid reference key: {error}"))
                })?;
                return Ok(ReferenceChange::UserDelete { key });
            }
            exact_keys(row, &["key", "desired", "next_revision"], &[])?;
            let key = serde_json::from_value(row["key"].clone())
                .map_err(|error| ValidationError::new(format!("invalid reference key: {error}")))?;
            let next_revision = row["next_revision"]
                .as_str()
                .ok_or_else(|| ValidationError::new("next_revision must be a string"))?
                .to_owned();
            if actor == Actor::User {
                let desired = serde_json::from_value(desired.clone()).map_err(|error| {
                    ValidationError::new(format!("invalid reference value: {error}"))
                })?;
                return Ok(ReferenceChange::UserSet {
                    key,
                    desired,
                    next_revision,
                });
            }
            let object = require_object(desired, "automatic reference value")?;
            exact_keys(object, &["name", "purpose", "role", "state", "target"], &[])?;
            let target = require_object(&object["target"], "automatic reference target")?;
            let string =
                |object: &Map<String, Value>, key: &str| -> Result<String, ValidationError> {
                    let value = object.get(key).and_then(Value::as_str).ok_or_else(|| {
                        ValidationError::new(format!("reference {key} must be a string"))
                    })?;
                    check_text(value, key, false)?;
                    Ok(value.to_owned())
                };
            let target = match target.get("kind").and_then(Value::as_str) {
                Some("document") => {
                    exact_keys(target, &["kind", "repository_url", "path"], &["ref"])?;
                    AutomaticReferenceTarget::Document(AutomaticDocumentTarget {
                        repository_url: string(target, "repository_url")?,
                        path: string(target, "path")?,
                        r#ref: target
                            .contains_key("ref")
                            .then(|| string(target, "ref"))
                            .transpose()?,
                    })
                }
                Some(kind @ ("pr" | "issue" | "link")) => {
                    exact_keys(target, &["kind", "url"], &[])?;
                    let url = string(target, "url")?;
                    match kind {
                        "pr" => AutomaticReferenceTarget::Pr { url },
                        "issue" => AutomaticReferenceTarget::Issue { url },
                        _ => AutomaticReferenceTarget::Link { url },
                    }
                }
                _ => {
                    return Err(ValidationError::new(
                        "invalid automatic reference target kind",
                    ));
                }
            };
            let desired = AutomaticReferenceValue {
                name: string(object, "name")?,
                purpose: string(object, "purpose")?,
                role: serde_json::from_value(object["role"].clone())
                    .map_err(|error| ValidationError::new(error.to_string()))?,
                state: serde_json::from_value(object["state"].clone())
                    .map_err(|error| ValidationError::new(error.to_string()))?,
                target,
            };
            Ok(ReferenceChange::Automatic {
                key,
                desired,
                next_revision,
            })
        })
        .collect()
}

pub fn parse_record_update(
    source: &str,
    catalog_source: &str,
    title_defaults: TitleDefaults,
) -> Result<RecordUpdate, ValidationError> {
    let value = load_json(source)?;
    require_object(&value, "captured edit")?;
    let wire: Wire = serde_json::from_value(value.clone())
        .map_err(|error| ValidationError::new(format!("invalid captured edit: {error}")))?;
    if wire.version != 2 {
        return Err(ValidationError::new(
            "unsupported captured edit version; capture fresh inputs",
        ));
    }
    check_text(&wire.catalog_source, "catalog_source", false)?;
    if wire.catalog_source != catalog_source {
        return Err(ValidationError::new("catalog source changed since capture"));
    }
    if wire.actor == Actor::Native || wire.kind == RecordKind::Host {
        return Err(ValidationError::new(
            "captured edits require user/automatic Stream or Session intent",
        ));
    }
    let references = wire.kind == RecordKind::Stream
        && wire.changes.len() == 1
        && wire.changes.contains_key("reference_changes");
    for field in wire.changes.keys() {
        if !references && !agent_field_allowed(wire.kind, wire.actor, field) {
            return Err(ValidationError::new(format!(
                "unknown agent update field: {field}"
            )));
        }
    }
    let before = record_from_data(wire.kind, &wire.before)?;
    let target = RecordAddress::for_record(&before);
    let edit = match before {
        Record::Session(session) => {
            if !wire.inputs.is_empty() {
                return Err(ValidationError::new(
                    "Session edits capture only their own conversation fields",
                ));
            }
            CapturedEdit::Session(SessionEdit::new(
                &wire.operation_id,
                &session,
                &wire.changes,
                wire.actor,
                wire.title_default,
            )?)
        }
        Record::Stream(stream) => {
            let captured_session: Option<Session> = match wire.inputs.as_slice() {
                [] => None,
                [input] if input.kind == RecordKind::Session => {
                    let Record::Session(session) = record_from_data(input.kind, &input.record)?
                    else {
                        unreachable!("kind fixes decoding")
                    };
                    Some(session)
                }
                _ => {
                    return Err(ValidationError::new(
                        "Stream edits accept only their single captured Session",
                    ));
                }
            };
            if references {
                let changes =
                    parse_reference_changes(wire.actor, &wire.changes["reference_changes"])?;
                CapturedEdit::References(ReferenceChanges::new(
                    &wire.operation_id,
                    &stream,
                    wire.actor,
                    captured_session.as_ref(),
                    changes,
                )?)
            } else {
                CapturedEdit::Stream(StreamEdit::new(
                    &wire.operation_id,
                    &stream,
                    &wire.changes,
                    wire.actor,
                    captured_session,
                    wire.title_default,
                )?)
            }
        }
        Record::Host(_) => unreachable!("Host edits refused above"),
    };
    // Fingerprint the original wire intent, including fields normalization declines to apply.
    let request = RecordRequest::new(target, wire.operation_id, &value)
        .map_err(|error| ValidationError::new(error.to_string()))?;
    Ok(RecordUpdate {
        request,
        edit,
        title_defaults,
    })
}
