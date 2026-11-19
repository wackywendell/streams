//! Domain-owned intent and pure reconciliation, independent of persistence.

pub mod edits;
pub mod membership;
pub mod patches;
pub mod references;

use serde::Serialize;
use strum::{AsRefStr, Display, EnumString, VariantNames};

use crate::model::{Record, Session, Status, Stream};

const DAY_MS: u64 = 86_400_000;

/// Compute the epoch-millisecond staleness cutoff for `decay_days` at `now_ms`.
/// Returns `None` when decay is disabled (`decay_days == 0`) or `now_ms` precedes the window.
pub fn decay_cutoff_ms(decay_days: u32, now_ms: u64) -> Option<u64> {
    if decay_days == 0 {
        return None;
    }
    let threshold_ms = u64::from(decay_days).saturating_mul(DAY_MS);
    let cutoff_ms = now_ms.saturating_sub(threshold_ms);
    (cutoff_ms > 0).then_some(cutoff_ms)
}

/// Whether `last_updated_ms` falls strictly before an active staleness cutoff.
pub fn is_stale_ms(last_updated_ms: u64, cutoff_ms: Option<u64>) -> bool {
    cutoff_ms.is_some_and(|cutoff| last_updated_ms > 0 && last_updated_ms < cutoff)
}

/// When a Stream last changed: its own last save or its newest member Session's. Every
/// listing and decay reads this one value.
pub fn stream_activity_ms(stream: &Stream, members: &[&Session]) -> u64 {
    members
        .iter()
        .map(|session| session.updated_at_ms)
        .fold(stream.updated_at_ms, u64::max)
}

/// Derived Stream status: the highest status among its Sessions under
/// `active > blocked > waiting > parked > done > closed` (or `active` when empty),
/// with read-only age decay (`cutoff_ms`) matching what `streams tidy` persists.
pub fn derived_stream_status(
    stream: &Stream,
    members: &[&Session],
    cutoff_ms: Option<u64>,
) -> Status {
    let status = members
        .iter()
        .map(|session| session.status)
        .max_by_key(|status| status.precedence())
        .unwrap_or(Status::Active);
    if status.is_open() && is_stale_ms(stream_activity_ms(stream, members), cutoff_ms) {
        if members.iter().any(|session| session.status == Status::Done) {
            Status::Done
        } else {
            Status::Closed
        }
    } else {
        status
    }
}

/// Who supplied the intent. Automatic writes cannot change lifecycle or manual naming modes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Actor {
    User,
    Automatic,
    /// Library-only verified native membership transitions, never model-generated intent.
    Native,
}

/// Domain acceptance only; it says nothing about persistence or publication.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum DomainOutcome {
    Changes,
    NoChange,
    Superseded,
    Conflict,
}

crate::codec::deserialize_string_enum!(Actor, DomainOutcome);

/// The domain decision, separate from whether a coordinator saved or published it.
/// Only changes carry a record; refusals retain field names and a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciliation {
    Changes {
        record: Box<Record>,
    },
    NoChange {
        reason: String,
    },
    Superseded {
        conflicts: Vec<String>,
        reason: String,
    },
    Conflict {
        conflicts: Vec<String>,
        reason: String,
    },
}

impl Reconciliation {
    pub fn no_change(reason: impl Into<String>) -> Self {
        Self::NoChange {
            reason: reason.into(),
        }
    }

    pub fn outcome(&self) -> DomainOutcome {
        match self {
            Self::Changes { .. } => DomainOutcome::Changes,
            Self::NoChange { .. } => DomainOutcome::NoChange,
            Self::Superseded { .. } => DomainOutcome::Superseded,
            Self::Conflict { .. } => DomainOutcome::Conflict,
        }
    }

    pub fn conflicts(&self) -> &[String] {
        match self {
            Self::Superseded { conflicts, .. } | Self::Conflict { conflicts, .. } => conflicts,
            Self::Changes { .. } | Self::NoChange { .. } => &[],
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::NoChange { reason, .. }
            | Self::Superseded { reason, .. }
            | Self::Conflict { reason, .. } => reason,
            Self::Changes { .. } => "",
        }
    }
}

/// What a captured edit's save reports. Reference saves add a result for each row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditOutcome {
    pub outcome: DomainOutcome,
    pub conflicts: Vec<String>,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_results: Option<Vec<references::ReferenceResult>>,
}

impl EditOutcome {
    pub fn of(decision: &Reconciliation) -> Self {
        Self {
            outcome: decision.outcome(),
            conflicts: decision.conflicts().to_vec(),
            reason: decision.reason().into(),
            reference_results: None,
        }
    }

    /// Accepted outcomes; refusals keep the caller's input for inspection.
    pub fn accepted(&self) -> bool {
        matches!(
            self.outcome,
            DomainOutcome::Changes | DomainOutcome::NoChange
        )
    }
}

impl Serialize for Reconciliation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Decision<'a> {
            outcome: DomainOutcome,
            record: Option<&'a Record>,
            conflicts: &'a [String],
            reason: &'a str,
        }
        Decision {
            outcome: self.outcome(),
            record: match self {
                Self::Changes { record, .. } => Some(record),
                _ => None,
            },
            conflicts: self.conflicts(),
            reason: self.reason(),
        }
        .serialize(serializer)
    }
}
