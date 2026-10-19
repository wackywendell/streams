//! One domain replay rule for publication candidates and non-publishing read views.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::model::{Catalog, Record};
use crate::operations::{Authority, Operation, Reconciliation, reconcile};

use super::envelope::operation_digest;
use super::history::History;
use super::local::authority_for;
use super::state::RepairRequired;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub catalog: Catalog,
    pub changes: Vec<(Operation, BTreeMap<String, String>, Record)>,
    pub conflicts: Vec<Value>,
}

pub fn replay(local: &History, remote: &History) -> Result<Replay, RepairRequired> {
    if (local.baseline.is_some() || remote.baseline.is_some())
        && (local.baseline.is_none()
            || remote.baseline.is_none()
            || local.root.is_none()
            || local.root != remote.root)
    {
        return Err(RepairRequired::new(
            "baseline replay requires an identical shared root",
        ));
    }
    let mut catalog = remote.catalog.clone();
    let mut authorities = remote.authorities.clone();
    let mut changes = Vec::new();
    for (_, (operation, origin)) in &local.operations {
        if let Some(published) = remote.fingerprints.get(&operation.id) {
            if published != &operation_digest(operation) {
                return Err(RepairRequired::new(format!(
                    "remote operation ID has different intent: {}",
                    operation.id
                )));
            }
            continue;
        }
        let result = if operation.before.is_none()
            && let Record::Stream(ref s) = operation.after
            && remote.catalog.stream(&s.id).is_some()
        {
            Reconciliation {
                operation_id: operation.id.clone(),
                outcome: "conflict".to_string(),
                record: None,
                conflicts: vec!["identity".to_string()],
                reason: "remote stream has a different creation".to_string(),
            }
        } else {
            let auth = authority_for(operation, &authorities);
            reconcile(&catalog, operation, auth.as_ref())
        };
        if result.outcome == "conflict" {
            let mut origin_map = Map::new();
            for (k, v) in origin {
                origin_map.insert(k.clone(), Value::String(v.clone()));
            }
            let target = operation.target();
            let conflict = json!({
                "operation": operation.id,
                "target": {
                    "kind": target.kind_str(),
                    "id": target.id(),
                    "host_id": target.host_id(),
                },
                "origin": Value::Object(origin_map),
                "fields": result.conflicts,
                "reason": result.reason,
            });
            return Ok(Replay {
                catalog,
                changes,
                conflicts: vec![conflict],
            });
        }
        if result.outcome != "changes" {
            continue;
        }
        let record = result.record.unwrap();
        catalog = catalog
            .replacing(record.clone())
            .map_err(|e| RepairRequired::new(e.to_string()))?;
        if operation.before.is_none()
            && let Record::Stream(ref s) = operation.after
        {
            authorities.insert(
                s.id.clone(),
                Authority::new(&s.owner_host_id, &operation.id)
                    .map_err(|e| RepairRequired::new(e.to_string()))?,
            );
        }
        changes.push((operation.clone(), origin.clone(), record));
    }
    Ok(Replay {
        catalog,
        changes,
        conflicts: Vec::new(),
    })
}
