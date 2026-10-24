//! Local agent capture and validated intent, never inferred enrollment or a refreshed base.

mod capture;
mod context;
pub use capture::{CaptureTarget, capture_inputs};
mod enrollment;
mod preview;
mod record_context;
pub(crate) mod record_update;
pub use context::{AgentContext, ContextError, ContextRequest, context, session_id_for};
pub use enrollment::{EnrollmentRequest, StreamProposal, effort_id_for};
pub use preview::preview;
pub use record_context::{RecordContext, RecordContextError, record_context};
pub use record_update::{RecordUpdate, parse_record_update};

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::{
    codec::{load_json, objects, record_from_data, require_object},
    model::{RecordKind, ValidationError, check_text},
    operations::{Actor, Operation, agent_field_allowed, filter_automatic_changes, update},
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
    #[serde(default, deserialize_with = "objects")]
    inputs: Vec<CapturedInput>,
}

/// Parse the Pi/harness update wire contract without looking up newer records.
///
/// The application supplies its current physical catalog-source fingerprint. This
/// fence does not replace the live conversation-lineage checks in the Pi extension.
/// Only explicitly allowed fields enter domain normalization; automatic requests
/// also discard edits to text already marked manual in the captured record.
pub fn parse_update(source: &str, catalog_source: &str) -> Result<Operation, ValidationError> {
    let value = load_json(source)?;
    require_object(&value, "agent update")?;
    let request: UpdateRequest = serde_json::from_value(value)
        .map_err(|e| ValidationError::new(format!("invalid agent update: {e}")))?;
    check_text(&request.catalog_source, "catalog_source", false)?;
    if request.catalog_source != catalog_source {
        return Err(ValidationError::new(
            "catalog source changed since capture; inspect the current catalog before making a new request",
        ));
    }
    if request.actor == Actor::Native {
        return Err(ValidationError::new(
            "agent updates cannot use native actor",
        ));
    }
    if request.kind == RecordKind::Host {
        return Err(ValidationError::new(
            "agent updates require stream or session kind",
        ));
    }
    for field in request.changes.keys() {
        if !agent_field_allowed(request.kind, request.actor, field) {
            return Err(ValidationError::new(format!(
                "unknown agent update field: {field}"
            )));
        }
    }
    let before = record_from_data(request.kind, &request.before)?;
    let changes = if request.actor == Actor::Automatic {
        filter_automatic_changes(
            &before,
            &request.changes,
            crate::model::NamingMode::Automatic,
        )
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
