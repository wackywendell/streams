//! Pure domain calculations and record edit rules, independent of persistence.

pub mod edits;

use crate::model::{Session, Status, Stream};

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
/// with read-only age decay (`cutoff_ms`) matching what `catalog tidy` persists.
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
