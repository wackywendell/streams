//! One domain replay rule for publication candidates and non-publishing read views.

use serde::{Deserialize, Serialize};

use crate::model::{Catalog, Record, RecordKey, RecordKind};
use crate::operations::{Authority, DomainOutcome, Operation, Reconciliation, reconcile};

use super::envelope::{Origin, operation_digest};
use super::history::History;
use super::local::authority_for;
use super::state::RepairRequired;

/// A replayed write retains original intent and provenance alongside its reconciled record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayChange {
    pub operation: Operation,
    pub origin: Origin,
    pub record: Record,
}

/// A domain conflict blocks the batch; it is not a storage or transport failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayConflict {
    pub operation: String,
    #[serde(with = "conflict_target")]
    pub target: RecordKey,
    pub origin: Origin,
    pub fields: Vec<String>,
    pub reason: String,
}

// Conflict JSON historically includes host_id: null for non-session targets.
mod conflict_target {
    use super::*;
    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Target {
        kind: RecordKind,
        id: String,
        host_id: Option<String>,
    }

    pub(super) fn serialize<S: serde::Serializer>(
        key: &RecordKey,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        Target {
            kind: key.kind(),
            id: key.id().into(),
            host_id: key.host_id().map(str::to_owned),
        }
        .serialize(s)
    }

    pub(super) fn deserialize<'de, D: serde::Deserializer<'de>>(
        d: D,
    ) -> Result<RecordKey, D::Error> {
        let Target { kind, id, host_id } = Target::deserialize(d)?;
        match (kind, host_id) {
            (RecordKind::Host, None) => Ok(RecordKey::Host { id }),
            (RecordKind::Stream, None) => Ok(RecordKey::Stream { id }),
            (RecordKind::Session, Some(host_id)) => Ok(RecordKey::Session { host_id, id }),
            _ => Err(serde::de::Error::custom(
                "host_id is required only for session conflict targets",
            )),
        }
    }
}

/// A projected view, plus outgoing changes or the first blocking conflict.
///
/// This value does not mutate either history or establish that changes were published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replay {
    pub catalog: Catalog,
    pub changes: Vec<ReplayChange>,
    pub conflicts: Vec<ReplayConflict>,
}

/// Reconcile unpublished local intent onto the remote view without rebasing Git commits.
/// Divergent creation identities, fingerprints or baseline roots fail closed.
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
    for entry in &local.operations {
        let (operation, origin) = (&entry.operation, &entry.origin);
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
                outcome: DomainOutcome::Conflict,
                record: None,
                conflicts: vec!["identity".to_string()],
                reason: "remote stream has a different creation".to_string(),
            }
        } else {
            let auth = authority_for(operation, &authorities);
            reconcile(&catalog, operation, auth.as_ref())
        };
        if result.outcome == DomainOutcome::Conflict {
            let conflict = ReplayConflict {
                operation: operation.id.clone(),
                target: operation.target(),
                origin: origin.clone(),
                fields: result.conflicts,
                reason: result.reason,
            };
            return Ok(Replay {
                catalog,
                changes,
                conflicts: vec![conflict],
            });
        }
        if result.outcome != DomainOutcome::Changes {
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
        changes.push(ReplayChange {
            operation: operation.clone(),
            origin: origin.clone(),
            record,
        });
    }
    Ok(Replay {
        catalog,
        changes,
        conflicts: Vec::new(),
    })
}
