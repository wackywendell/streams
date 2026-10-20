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
    pub target: ConflictTarget,
    pub origin: Origin,
    pub fields: Vec<String>,
    pub reason: String,
}

/// Conflict presentation includes physical routing without changing logical identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "TargetData", into = "TargetData")]
pub struct ConflictTarget {
    key: RecordKey,
    stream_id: Option<String>,
}

impl ConflictTarget {
    pub fn from_record(record: &Record) -> Self {
        Self {
            key: record.key(),
            stream_id: match record {
                Record::Session(session) => Some(session.stream_id.clone()),
                _ => None,
            },
        }
    }

    pub fn key(&self) -> &RecordKey {
        &self.key
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetData {
    kind: RecordKind,
    id: String,
    host_id: Option<String>,
    stream_id: Option<String>,
}

impl From<ConflictTarget> for TargetData {
    fn from(target: ConflictTarget) -> Self {
        Self {
            kind: target.key.kind(),
            id: target.key.id().into(),
            host_id: target.key.host_id().map(str::to_owned),
            stream_id: target.stream_id,
        }
    }
}

impl TryFrom<TargetData> for ConflictTarget {
    type Error = String;

    fn try_from(data: TargetData) -> Result<Self, Self::Error> {
        let key = match (data.kind, data.host_id, data.stream_id.as_deref()) {
            (RecordKind::Host, None, None) => RecordKey::Host { id: data.id },
            (RecordKind::Stream, None, None) => RecordKey::Stream { id: data.id },
            (RecordKind::Session, Some(host_id), Some(stream_id)) => {
                crate::model::check_text(&host_id, "session host id", false)
                    .map_err(|e| e.to_string())?;
                crate::model::check_text(stream_id, "session stream id", false)
                    .map_err(|e| e.to_string())?;
                RecordKey::Session {
                    host_id,
                    id: data.id,
                }
            }
            _ => return Err("only session conflict targets require host_id and stream_id".into()),
        };
        crate::model::check_text(key.id(), "record id", false).map_err(|e| e.to_string())?;
        Ok(Self {
            key,
            stream_id: data.stream_id,
        })
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
                target: ConflictTarget::from_record(operation.after()),
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
