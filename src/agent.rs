//! Captured agent requests become validated domain intent, never a refreshed base.

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    codec::{load_json, record_from_data},
    model::{RecordKind, ValidationError, check_text},
    operations::{Actor, Operation, filter_automatic_changes, update},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedInput {
    kind: RecordKind,
    record: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRequest {
    operation_id: String,
    catalog_source: String,
    kind: RecordKind,
    actor: Actor,
    before: Value,
    changes: Map<String, Value>,
    ownership_token: Option<String>,
    #[serde(default)]
    inputs: Vec<CapturedInput>,
}

/// Parse the Pi/harness update wire contract without looking up newer records.
///
/// The application supplies its current physical catalog-source fingerprint. This
/// fence does not replace the live conversation-lineage checks in the Pi extension.
/// Only explicitly allowed fields enter domain normalization; automatic requests
/// also discard edits to text already marked manual in the captured record.
pub fn parse_update(source: &str, catalog_source: &str) -> Result<Operation, ValidationError> {
    let request: UpdateRequest = serde_json::from_value(load_json(source)?)
        .map_err(|e| ValidationError::new(format!("invalid agent update: {e}")))?;
    check_text(&request.catalog_source, "catalog_source", false)?;
    if request.catalog_source != catalog_source {
        return Err(ValidationError::new(
            "catalog source changed since capture; inspect the current catalog before making a new request",
        ));
    }
    let allowed: &[&str] = match (request.kind, request.actor) {
        (RecordKind::Stream, Actor::User) => &[
            "title",
            "title_mode",
            "goal",
            "goal_short",
            "goal_mode",
            "status",
            "archived",
            "documents",
            "issues",
            "prs",
        ],
        (RecordKind::Stream, Actor::Automatic) => &["title", "goal", "goal_short"],
        (RecordKind::Session, Actor::User) => &[
            "title",
            "title_mode",
            "status",
            "state",
            "state_short",
            "remaining",
        ],
        (RecordKind::Session, Actor::Automatic) => {
            &["title", "status", "state", "state_short", "remaining"]
        }
        (RecordKind::Host, _) => {
            return Err(ValidationError::new(
                "agent updates require stream or session kind",
            ));
        }
    };
    for field in request.changes.keys() {
        if !allowed.contains(&field.as_str()) {
            return Err(ValidationError::new(format!(
                "unknown agent update field: {field}"
            )));
        }
    }
    let before = record_from_data(request.kind, &request.before)?;
    let changes = if request.actor == Actor::Automatic {
        filter_automatic_changes(&before, &request.changes)
    } else {
        request.changes
    };
    let inputs = request
        .inputs
        .iter()
        .map(|input| record_from_data(input.kind, &input.record))
        .collect::<Result<Vec<_>, _>>()?;
    update(
        request.operation_id,
        &before,
        &changes,
        request.actor,
        request.ownership_token,
        inputs,
    )
}
