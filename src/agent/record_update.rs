//! Versioned captured edits. Old authority-bearing requests are never adapted or replayed.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    codec::{load_json, objects, record_from_data, require_object},
    model::{NamingMode, Record, RecordKind, Session, TitleDefaults, ValidationError, check_text},
    operations::{
        Actor, agent_field_allowed,
        edits::{SessionEdit, StreamEdit},
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
}
impl RecordUpdate {
    pub fn operation_id(&self) -> &str {
        match &self.edit {
            CapturedEdit::Session(edit) => edit.operation_id(),
            CapturedEdit::Stream(edit) => edit.operation_id(),
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
    for field in wire.changes.keys() {
        if !agent_field_allowed(wire.kind, wire.actor, field) {
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
            CapturedEdit::Stream(StreamEdit::new(
                &wire.operation_id,
                &stream,
                &wire.changes,
                wire.actor,
                captured_session,
                wire.title_default,
            )?)
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
