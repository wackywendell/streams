//! Event eligibility only: no clock, file format, agent, or network dependencies.

use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumString, VariantNames};

use super::Seconds;

/// The caller's reason for syncing. Explicit, force and shutdown events bypass cooldowns.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SyncEvent {
    Session,
    Host,
    Stream,
    Start,
    Attach,
    Shutdown,
    Explicit,
    Force,
}

crate::codec::deserialize_string_enum!(SyncEvent);

/// Publication scheduling timestamps, separate from read-refresh freshness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    last_sync: Option<Seconds>,
    last_push: Option<Seconds>,
    retry_after: Seconds,
}

impl Timing {
    pub fn new(
        last_sync: Option<Seconds>,
        last_push: Option<Seconds>,
        retry_after: Seconds,
    ) -> Self {
        Self {
            last_sync,
            last_push,
            retry_after,
        }
    }
}

/// Independent read/publication intervals and bounded effect deadlines and attempts.
/// Construction and deserialization reject nonfinite durations and unbounded attempts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PolicyData")]
pub struct Policy {
    stream_seconds: f64,
    session_seconds: f64,
    cooldown_seconds: f64,
    timeout_seconds: f64,
    attempts: usize,
}

#[derive(Deserialize)]
struct PolicyData {
    stream_seconds: f64,
    session_seconds: f64,
    cooldown_seconds: f64,
    timeout_seconds: f64,
    attempts: usize,
}

impl TryFrom<PolicyData> for Policy {
    type Error = String;

    fn try_from(data: PolicyData) -> Result<Self, Self::Error> {
        Self::new(
            data.stream_seconds,
            data.session_seconds,
            data.cooldown_seconds,
            data.timeout_seconds,
            data.attempts,
        )
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            stream_seconds: 300.0,
            session_seconds: 3600.0,
            cooldown_seconds: 60.0,
            timeout_seconds: 30.0,
            attempts: 3,
        }
    }
}

impl Policy {
    pub fn new(
        stream_seconds: f64,
        session_seconds: f64,
        cooldown_seconds: f64,
        timeout_seconds: f64,
        attempts: usize,
    ) -> Result<Self, String> {
        for v in [
            stream_seconds,
            session_seconds,
            cooldown_seconds,
            timeout_seconds,
        ] {
            if !v.is_finite() || v < 0.0 {
                return Err("sync intervals must be finite nonnegative numbers".to_string());
            }
        }
        if timeout_seconds == 0.0 || !(1..=3).contains(&attempts) {
            return Err("sync needs a positive timeout and one to three attempts".to_string());
        }
        Ok(Self {
            stream_seconds,
            session_seconds,
            cooldown_seconds,
            timeout_seconds,
            attempts,
        })
    }

    pub fn stream_seconds(&self) -> f64 {
        self.stream_seconds
    }

    pub fn session_seconds(&self) -> f64 {
        self.session_seconds
    }

    pub fn cooldown_seconds(&self) -> f64 {
        self.cooldown_seconds
    }

    pub fn timeout_seconds(&self) -> f64 {
        self.timeout_seconds
    }

    pub fn attempts(&self) -> usize {
        self.attempts
    }

    /// Refresh on the read interval unless cooling down; forced reads bypass both.
    pub fn read_due(
        &self,
        fetched_at: Option<Seconds>,
        now: Seconds,
        retry_after: Seconds,
        force: bool,
    ) -> bool {
        if force {
            return true;
        }
        if now.get() < retry_after.get() {
            return false;
        }
        match fetched_at {
            None => true,
            Some(ts) => now.get() - ts.get() >= self.session_seconds,
        }
    }

    /// Use the shorter interval for stream intent, even when triggered by a session event.
    pub fn due(
        &self,
        event: SyncEvent,
        timing: Timing,
        now: Seconds,
        pending_stream: bool,
    ) -> bool {
        if matches!(
            event,
            SyncEvent::Shutdown | SyncEvent::Explicit | SyncEvent::Force
        ) {
            return true;
        }
        if now.get() < timing.retry_after.get() {
            return false;
        }
        if matches!(event, SyncEvent::Session | SyncEvent::Host) && !pending_stream {
            return match timing.last_push {
                None => true,
                Some(ts) => now.get() - ts.get() >= self.session_seconds,
            };
        }
        match timing.last_sync {
            None => true,
            Some(ts) => now.get() - ts.get() >= self.stream_seconds,
        }
    }
}
