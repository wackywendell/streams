//! Event eligibility only: no clock, file format, agent, or network dependencies.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Timing {
    pub last_sync: Option<f64>,
    pub last_push: Option<f64>,
    pub retry_after: f64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            last_sync: None,
            last_push: None,
            retry_after: 0.0,
        }
    }
}

impl Timing {
    pub fn new(
        last_sync: Option<f64>,
        last_push: Option<f64>,
        retry_after: f64,
    ) -> Result<Self, String> {
        for val in [last_sync, last_push, Some(retry_after)] {
            if let Some(v) = val
                && !v.is_finite()
            {
                return Err("sync timestamps must be finite numbers".to_string());
            }
        }
        Ok(Self {
            last_sync,
            last_push,
            retry_after,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Policy {
    pub stream_seconds: f64,
    pub session_seconds: f64,
    pub cooldown_seconds: f64,
    pub timeout_seconds: f64,
    pub attempts: usize,
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

    pub fn read_due(
        &self,
        fetched_at: Option<f64>,
        now: f64,
        retry_after: f64,
        force: bool,
    ) -> Result<bool, String> {
        Timing::new(fetched_at, None, retry_after)?;
        if !now.is_finite() {
            return Err("sync timestamp must be a finite number".to_string());
        }
        if force {
            return Ok(true);
        }
        if now < retry_after {
            return Ok(false);
        }
        Ok(match fetched_at {
            None => true,
            Some(ts) => now - ts >= self.session_seconds,
        })
    }

    pub fn due(
        &self,
        event: &str,
        timing: Timing,
        now: f64,
        pending_stream: bool,
    ) -> Result<bool, String> {
        Timing::new(timing.last_sync, timing.last_push, timing.retry_after)?;
        if !now.is_finite() {
            return Err("sync timestamp must be a finite number".to_string());
        }
        if matches!(event, "shutdown" | "explicit" | "force") {
            return Ok(true);
        }
        if !matches!(event, "session" | "host" | "stream" | "start" | "attach") {
            return Err(format!("unknown sync event: {event}"));
        }
        if now < timing.retry_after {
            return Ok(false);
        }
        if matches!(event, "session" | "host") && !pending_stream {
            return Ok(match timing.last_push {
                None => true,
                Some(ts) => now - ts >= self.session_seconds,
            });
        }
        Ok(match timing.last_sync {
            None => true,
            Some(ts) => now - ts >= self.stream_seconds,
        })
    }
}
