//! Native workspace effects, independent of catalog ownership and launch policy.
//!
//! References are scoped to a frontend and endpoint, not a universal pane tuple.
//! These adapters never enroll conversations, choose checkouts, restore layouts,
//! install plugins or retry uncertain effects.

pub mod hc;
pub mod herdr;
mod socket_io;
mod transport;

use std::collections::BTreeMap;

pub use transport::{CommandEvidence, CommandOutcome, FrontendError, NativeRequest};

/// The built-in frontends. The identity only selects an implementation.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    strum::EnumString,
    strum::AsRefStr,
    strum::Display,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum FrontendId {
    Cmux,
    Herdr,
    Hc,
}

/// Adapter-defined endpoint identity; equality is exact, not GUI focus.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct EndpointId(pub String);

/// Configured frontend policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendMode {
    Auto,
    None,
    Explicit(FrontendId),
}

/// `Auto` defers to configuration, just as an omitted override does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendRequest {
    Auto,
    Explicit(FrontendId),
}

impl std::str::FromStr for FrontendRequest {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "auto" => Ok(Self::Auto),
            id => id
                .parse()
                .map(Self::Explicit)
                .map_err(|_| "invalid frontend identifier; expected auto, herdr, cmux or hc"),
        }
    }
}

/// Resolved policy without any native discovery or caller-workspace inference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontendSelection {
    pub configured: FrontendMode,
    pub requested: Option<FrontendRequest>,
    pub resolved: Option<FrontendId>,
}

/// Policy failures have no native effects or command evidence.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrontendSelectionError {
    #[error("incomplete Herdr caller context; expected HERDR_ENV=1 and HERDR_PANE_ID")]
    IncompleteHerdr,
    #[error("multiple native caller contexts are present; choose an explicit frontend")]
    Ambiguous(Vec<FrontendId>),
}

/// Pure Python-compatible selection policy; explicit modes bypass auto detection.
pub fn resolve_frontend(
    configured: FrontendMode,
    requested: Option<FrontendRequest>,
    context: &BTreeMap<String, String>,
) -> Result<FrontendSelection, FrontendSelectionError> {
    let policy = match &requested {
        Some(FrontendRequest::Explicit(id)) => FrontendMode::Explicit(*id),
        _ => configured.clone(),
    };
    let resolved = match policy {
        FrontendMode::None => None,
        FrontendMode::Explicit(id) => Some(id),
        FrontendMode::Auto => {
            let cmux = context
                .get("CMUX_WORKSPACE_ID")
                .is_some_and(|v| !v.is_empty());
            let herdr = context.get("HERDR_ENV").is_some_and(|v| v == "1");
            let pane = context.get("HERDR_PANE_ID").is_some_and(|v| !v.is_empty());
            let hc = context.get("HC_TERMINAL_ID").is_some_and(|v| !v.is_empty());
            if herdr != pane {
                return Err(FrontendSelectionError::IncompleteHerdr);
            }
            let mut active = Vec::new();
            if cmux {
                active.push(FrontendId::Cmux);
            }
            if herdr {
                active.push(FrontendId::Herdr);
            }
            if hc {
                active.push(FrontendId::Hc);
            }
            match active.len() {
                0 => None,
                1 => active.into_iter().next(),
                _ => return Err(FrontendSelectionError::Ambiguous(active)),
            }
        }
    };
    Ok(FrontendSelection {
        configured,
        requested,
        resolved,
    })
}

pub(crate) fn nonempty(value: &str, name: &str) -> Result<(), FrontendError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err(FrontendError::invalid(format!("invalid {name}")))
    } else {
        Ok(())
    }
}
