//! Versioned captured edits. Old authority-bearing requests are never adapted or replayed.

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::{
    codec::{load_json, object, objects, require_object, session_from_data, stream_from_data},
    model::{NamingMode, RecordKind, Session, TitleDefaults, ValidationError, check_text},
    operations::{
        Actor,
        edits::{SessionEdit, StreamEdit},
        patches::{Patch, PatchField},
        references::{
            AutomaticDocumentTarget, AutomaticReferenceTarget, AutomaticReferenceValue,
            ReferenceChange, ReferenceChanges,
        },
    },
    storage::records::RecordAddress,
};

#[derive(Debug, Clone)]
pub struct RecordUpdate {
    pub(crate) target: RecordAddress,
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
    pub fn target(&self) -> &RecordAddress {
        &self.target
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
    kind: RecordKind,
    actor: Actor,
    before: Value,
    changes: Value,
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
            exact_keys(row, &["key", "desired"], &[])?;
            let key = serde_json::from_value(row["key"].clone())
                .map_err(|error| ValidationError::new(format!("invalid reference key: {error}")))?;
            if actor == Actor::User {
                let desired = serde_json::from_value(desired.clone()).map_err(|error| {
                    ValidationError::new(format!("invalid reference value: {error}"))
                })?;
                return Ok(ReferenceChange::UserSet { key, desired });
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
            Ok(ReferenceChange::Automatic { key, desired })
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
    if wire.actor == Actor::Native {
        return Err(ValidationError::new(
            "captured edits require user/automatic Stream or Session intent",
        ));
    }
    let edit = match wire.kind {
        RecordKind::Session => {
            let patch = agent_patch(wire.changes, wire.actor)?;
            let session = session_from_data(&wire.before)?;
            if !wire.inputs.is_empty() {
                return Err(ValidationError::new(
                    "Session edits capture only their own conversation fields",
                ));
            }
            CapturedEdit::Session(SessionEdit::new(
                &session,
                patch,
                wire.actor,
                wire.title_default,
            )?)
        }
        RecordKind::Stream => {
            let references = match wire.changes.as_object() {
                Some(changes) if changes.len() == 1 => changes.get("reference_changes"),
                _ => None,
            }
            .map(|rows| parse_reference_changes(wire.actor, rows))
            .transpose()?;
            let stream = stream_from_data(&wire.before)?;
            let captured_session: Option<Session> = match wire.inputs.as_slice() {
                [] => None,
                [input] if input.kind == RecordKind::Session => {
                    Some(session_from_data(&input.record)?)
                }
                _ => {
                    return Err(ValidationError::new(
                        "Stream edits accept only their single captured Session",
                    ));
                }
            };
            match references {
                Some(changes) => CapturedEdit::References(ReferenceChanges::new(
                    &stream,
                    wire.actor,
                    captured_session.as_ref(),
                    changes,
                )?),
                None => CapturedEdit::Stream(StreamEdit::new(
                    &stream,
                    agent_patch(wire.changes, wire.actor)?,
                    wire.actor,
                    captured_session,
                    wire.title_default,
                )?),
            }
        }
        RecordKind::Host => {
            return Err(ValidationError::new(
                "captured edits require user/automatic Stream or Session intent",
            ));
        }
    };
    let target = match &edit {
        CapturedEdit::Session(edit) => RecordAddress::session(edit.before()),
        CapturedEdit::Stream(edit) => RecordAddress::stream(edit.before()),
        CapturedEdit::References(changes) => RecordAddress::stream(changes.before()),
    };
    Ok(RecordUpdate {
        target,
        edit,
        title_defaults,
    })
}

/// An agent request's field changes, limited to the fields its actor may set through an
/// agent.
fn agent_patch<P: Patch + DeserializeOwned>(
    changes: Value,
    actor: Actor,
) -> Result<P, ValidationError> {
    let patch: P = object(changes)
        .map_err(|error| ValidationError::new(format!("invalid captured edit: {error}")))?;
    if let Some(field) = patch
        .fields()
        .into_iter()
        .find(|field| !field.access().permits(actor, true))
    {
        return Err(ValidationError::new(format!(
            "unknown agent update field: {}",
            field.name()
        )));
    }
    Ok(patch)
}
