//! Captured, independent reference intent and effect-free reconciliation.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Actor, DomainOutcome, Reconciliation};
use crate::model::{
    Record, Reference, ReferenceRole, ReferenceState, ReferenceTarget, ReferenceValue,
    RepositoryDocument, ResourceKey, Session, Stream, ValidationError, check_text,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutomaticDocumentTarget {
    pub repository_url: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AutomaticReferenceTarget {
    Document(AutomaticDocumentTarget),
    Pr { url: String },
    Issue { url: String },
    Link { url: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AutomaticReferenceValue {
    pub name: String,
    pub purpose: String,
    pub role: ReferenceRole,
    pub state: ReferenceState,
    pub target: AutomaticReferenceTarget,
}

impl AutomaticReferenceValue {
    /// Materialize only runtime-owned recovery hints; omitted ref preserves a saved hint.
    pub fn materialize(&self, captured: Option<&Reference>, session: &Session) -> ReferenceValue {
        let target = match &self.target {
            AutomaticReferenceTarget::Document(document) => {
                let old = captured.and_then(|reference| match &reference.target {
                    ReferenceTarget::Document(document) => Some(document),
                    _ => None,
                });
                let checkout_ref = session
                    .checkout
                    .as_ref()
                    .filter(|checkout| {
                        checkout.repository_url.as_deref() == Some(document.repository_url.as_str())
                    })
                    .map(|checkout| checkout.r#ref.clone());
                ReferenceTarget::Document(RepositoryDocument {
                    repository_url: document.repository_url.clone(),
                    path: document.path.clone(),
                    r#ref: document.r#ref.clone().or_else(|| {
                        if let Some(old) = old {
                            old.r#ref.clone()
                        } else {
                            checkout_ref
                        }
                    }),
                    commit: old.and_then(|old| old.commit.clone()),
                    source_session_id: old.map_or_else(
                        || Some(session.id.clone()),
                        |old| old.source_session_id.clone(),
                    ),
                })
            }
            AutomaticReferenceTarget::Pr { url } => ReferenceTarget::Pr { url: url.clone() },
            AutomaticReferenceTarget::Issue { url } => ReferenceTarget::Issue { url: url.clone() },
            AutomaticReferenceTarget::Link { url } => ReferenceTarget::Link { url: url.clone() },
        };
        ReferenceValue {
            name: self.name.clone(),
            purpose: self.purpose.clone(),
            role: self.role,
            state: self.state,
            target,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ReferenceChange {
    Automatic {
        key: ResourceKey,
        desired: AutomaticReferenceValue,
        next_revision: String,
    },
    UserSet {
        key: ResourceKey,
        desired: ReferenceValue,
        next_revision: String,
    },
    UserDelete {
        key: ResourceKey,
    },
}

impl Serialize for ReferenceChange {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Automatic {
                key,
                desired,
                next_revision,
            } => json!({"key":key,"desired":desired,"next_revision":next_revision}),
            Self::UserSet {
                key,
                desired,
                next_revision,
            } => json!({"key":key,"desired":desired,"next_revision":next_revision}),
            Self::UserDelete { key } => json!({"key":key,"desired":null}),
        }
        .serialize(serializer)
    }
}

impl ReferenceChange {
    fn key(&self) -> &ResourceKey {
        match self {
            Self::Automatic { key, .. } | Self::UserSet { key, .. } | Self::UserDelete { key } => {
                key
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ReferenceAction {
    Add,
    Update,
    Drop,
    Restore,
    Delete,
}
crate::codec::deserialize_string_enum!(ReferenceAction);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceResult {
    pub key: ResourceKey,
    pub action: ReferenceAction,
    pub outcome: DomainOutcome,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct ReferenceChanges {
    operation_id: String,
    before: Stream,
    actor: Actor,
    session: Option<Session>,
    changes: Vec<ReferenceChange>,
}

impl ReferenceChanges {
    pub fn new(
        operation_id: &str,
        before: &Stream,
        actor: Actor,
        captured_session: Option<&Session>,
        changes: Vec<ReferenceChange>,
    ) -> Result<Self, ValidationError> {
        check_text(operation_id, "operation_id", false)?;
        before.validate()?;
        match (actor, captured_session) {
            (Actor::Automatic, Some(session)) => {
                session.validate_portable()?;
                if session.stream_id != before.id || session.host_id != before.owner_host_id {
                    return Err(ValidationError::new(
                        "reference changes require their captured Stream and Session",
                    ));
                }
            }
            (Actor::User, None) => {}
            _ => {
                return Err(ValidationError::new(
                    "reference actor and captured Session disagree",
                ));
            }
        }
        if changes.is_empty() {
            return Err(ValidationError::new("reference_changes must be nonempty"));
        }
        let mut batch_keys = BTreeSet::new();
        for change in &changes {
            let key = change.key();
            key.validate()?;
            let old = before
                .references
                .iter()
                .find(|reference| reference.target.key() == *key);
            let (desired, revision) = match (actor, change) {
                (
                    Actor::Automatic,
                    ReferenceChange::Automatic {
                        desired,
                        next_revision,
                        ..
                    },
                ) => {
                    let desired = desired
                        .materialize(old, captured_session.expect("automatic Session checked"));
                    if desired.target.key() != *key {
                        return Err(ValidationError::new(
                            "automatic reference cannot change resource key",
                        ));
                    }
                    (Some(desired), Some(next_revision))
                }
                (
                    Actor::User,
                    ReferenceChange::UserSet {
                        desired,
                        next_revision,
                        ..
                    },
                ) => (Some(desired.clone()), Some(next_revision)),
                (Actor::User, ReferenceChange::UserDelete { .. }) if old.is_some() => (None, None),
                _ => {
                    return Err(ValidationError::new(
                        "reference change does not match actor or captured presence",
                    ));
                }
            };
            let mut row_keys = BTreeSet::from([key.clone()]);
            if let Some(desired) = desired {
                desired.validate()?;
                let next_key = desired.target.key();
                if old.is_none() && next_key != *key {
                    return Err(ValidationError::new(
                        "reference addition key must match target",
                    ));
                }
                row_keys.insert(next_key);
            }
            if let Some(revision) = revision {
                check_text(revision, "next_revision", false)?;
                if revision != operation_id || old.is_some_and(|old| old.revision == *revision) {
                    return Err(ValidationError::new(
                        "next_revision must be operation_id and differ from captured revision",
                    ));
                }
            }
            for key in row_keys {
                if !batch_keys.insert(key) {
                    return Err(ValidationError::new("reference proposals overlap"));
                }
            }
        }
        Ok(Self {
            operation_id: operation_id.into(),
            before: before.clone(),
            actor,
            session: captured_session.cloned(),
            changes,
        })
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn before(&self) -> &Stream {
        &self.before
    }
    pub fn captured_session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    pub fn apply(
        &self,
        current: &Stream,
        session: Option<&Session>,
    ) -> Result<ReferenceReconciliation, ValidationError> {
        let guard =
            if current.id != self.before.id || current.owner_host_id != self.before.owner_host_id {
                Some("captured Stream identity or ownership changed")
            } else if self.actor == Actor::Automatic
                && !self
                    .session
                    .as_ref()
                    .zip(session)
                    .is_some_and(|(captured, current)| {
                        current.id == captured.id
                            && current.native_identity() == captured.native_identity()
                            && current.stream_id == captured.stream_id
                            && current.previous_stream_ids == captured.previous_stream_ids
                    })
            {
                Some("captured Session identity or membership period changed")
            } else {
                None
            };
        let refused = if self.actor == Actor::Automatic {
            DomainOutcome::Superseded
        } else {
            DomainOutcome::Conflict
        };
        let mut next = current.clone();
        let mut results = Vec::new();
        for change in &self.changes {
            let key = change.key();
            let old = self
                .before
                .references
                .iter()
                .find(|reference| reference.target.key() == *key);
            let actual = current
                .references
                .iter()
                .find(|reference| reference.target.key() == *key);
            let (desired, revision) = match change {
                ReferenceChange::Automatic {
                    desired,
                    next_revision,
                    ..
                } => (
                    Some(
                        desired.materialize(old, self.session.as_ref().expect("validated Session")),
                    ),
                    Some(next_revision),
                ),
                ReferenceChange::UserSet {
                    desired,
                    next_revision,
                    ..
                } => (Some(desired.clone()), Some(next_revision)),
                ReferenceChange::UserDelete { .. } => (None, None),
            };
            let action = match (old, &desired) {
                (None, _) => ReferenceAction::Add,
                (_, None) => ReferenceAction::Delete,
                (Some(old), Some(desired))
                    if old.state == ReferenceState::Dropped
                        && desired.state != ReferenceState::Dropped =>
                {
                    ReferenceAction::Restore
                }
                (Some(old), Some(desired))
                    if old.state != ReferenceState::Dropped
                        && desired.state == ReferenceState::Dropped =>
                {
                    ReferenceAction::Drop
                }
                _ => ReferenceAction::Update,
            };
            let reason = guard
                .or_else(|| match (old, actual) {
                    (Some(_), None) => Some("reference no longer exists"),
                    (Some(old), Some(actual)) if old != actual => {
                        Some("reference changed since capture")
                    }
                    (None, Some(_)) => Some("reference already exists"),
                    _ => None,
                })
                .or_else(|| {
                    (self.actor == Actor::Automatic && action == ReferenceAction::Restore)
                        .then_some("reference is Dropped; explicit restoration is required")
                })
                .or_else(|| {
                    desired.as_ref().and_then(|desired| {
                        let destination = desired.target.key();
                        (destination != *key
                            && current
                                .references
                                .iter()
                                .any(|reference| reference.target.key() == destination))
                        .then_some("replacement target already exists")
                    })
                });
            let outcome = if reason.is_some() {
                refused
            } else if desired.as_ref() == actual.map(Reference::value).as_ref() {
                DomainOutcome::NoChange
            } else {
                let index = next
                    .references
                    .iter()
                    .position(|reference| reference.target.key() == *key);
                match desired {
                    Some(desired) => {
                        let reference = Reference::new(
                            desired,
                            revision.expect("set revision checked").clone(),
                        )?;
                        if let Some(index) = index {
                            next.references[index] = reference;
                        } else {
                            next.references.push(reference);
                        }
                    }
                    None => {
                        next.references
                            .remove(index.expect("delete captured and current presence checked"));
                    }
                }
                DomainOutcome::Changes
            };
            results.push(ReferenceResult {
                key: key.clone(),
                action,
                outcome,
                reason: reason.unwrap_or("").into(),
            });
        }
        next.validate()?;
        let outcome = if results
            .iter()
            .any(|row| row.outcome == DomainOutcome::Changes)
        {
            DomainOutcome::Changes
        } else if results
            .iter()
            .any(|row| row.outcome == DomainOutcome::Conflict)
        {
            DomainOutcome::Conflict
        } else if results
            .iter()
            .any(|row| row.outcome == DomainOutcome::Superseded)
        {
            DomainOutcome::Superseded
        } else {
            DomainOutcome::NoChange
        };
        let conflicts = vec![if guard.is_some() {
            "inputs".into()
        } else {
            "references".into()
        }];
        let reconciliation = match outcome {
            DomainOutcome::Changes => Reconciliation::Changes {
                operation_id: self.operation_id.clone(),
                record: Box::new(Record::Stream(next)),
            },
            DomainOutcome::NoChange => {
                Reconciliation::no_change(&self.operation_id, "no effective reference changes")
            }
            DomainOutcome::Conflict => Reconciliation::Conflict {
                operation_id: self.operation_id.clone(),
                conflicts,
                reason: guard.unwrap_or("reference proposals conflict").into(),
            },
            DomainOutcome::Superseded => Reconciliation::Superseded {
                operation_id: self.operation_id.clone(),
                conflicts,
                reason: guard
                    .unwrap_or("reference proposals were superseded")
                    .into(),
            },
        };
        Ok(ReferenceReconciliation {
            reconciliation,
            reference_results: results,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ReferenceReconciliation {
    pub reconciliation: Reconciliation,
    pub reference_results: Vec<ReferenceResult>,
}

impl ReferenceReconciliation {
    pub fn domain(&self) -> Map<String, Value> {
        json!({"outcome":self.reconciliation.outcome(), "conflicts":self.reconciliation.conflicts(),
            "reason":self.reconciliation.reason(), "reference_results":self.reference_results})
        .as_object()
        .expect("domain object")
        .clone()
    }
}
