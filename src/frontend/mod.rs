//! Native workspace effects, independent of catalog ownership and launch policy.
//!
//! References are scoped to a frontend and endpoint, not a universal pane tuple.
//! These adapters never enroll conversations, choose checkouts, restore layouts,
//! install plugins or retry uncertain effects.

pub mod cmux;
pub mod hc;
pub mod herdr;
mod socket_io;
mod transport;

use std::collections::BTreeMap;
use std::path::PathBuf;

pub use crate::integration::NativeRef;
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

/// Workspace display information, not evidence of catalog association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub reference: NativeRef,
    pub title: String,
    pub cwd: String,
}

/// One prepared submission. Legacy callers retain their established error response;
/// the shared effects still describe accepted work and every unattempted input.
pub struct PreparedOpenOutcome {
    pub effects: crate::integration::OpenEffects,
    pub error: Option<FrontendError>,
}
fn unattempted_open(plan: &crate::integration::PreparedOpen) -> crate::integration::OpenEffects {
    use crate::integration::*;
    OpenEffects {
        launches: plan
            .launches
            .iter()
            .map(|_| Effect::NotApplied {
                reason: "not attempted after an earlier failure".into(),
            })
            .collect(),
        adoptions: plan
            .adopt
            .iter()
            .map(|terminal| AdoptionEffect {
                terminal: terminal.clone(),
                result: Effect::NotApplied {
                    reason: "adoption is unsupported by this adapter".into(),
                },
            })
            .collect(),
        presentation: PresentationResult {
            space: SpaceEvidence::None,
            state: PresentationState::Incomplete {
                reason: "opening did not complete".into(),
            },
        },
        viewer_actions: vec![],
    }
}
fn uncertain(error: &FrontendError) -> bool {
    error
        .commands
        .iter()
        .any(|command| command.outcome == CommandOutcome::Uncertain)
}
fn failed_launch(
    error: &FrontendError,
) -> crate::integration::Effect<crate::integration::NativeRef> {
    if uncertain(error) {
        crate::integration::Effect::Uncertain {
            reason: error.message.clone(),
        }
    } else {
        crate::integration::Effect::NotApplied {
            reason: error.message.clone(),
        }
    }
}
fn failed_presentation(error: &FrontendError) -> crate::integration::PresentationState {
    if uncertain(error) {
        crate::integration::PresentationState::Uncertain {
            reason: error.message.clone(),
        }
    } else {
        crate::integration::PresentationState::Incomplete {
            reason: error.message.clone(),
        }
    }
}

/// Creates a shell workspace only. Session launch belongs to the runtime.
#[derive(Debug, Clone)]
pub struct CreateWorkspace {
    pub title: String,
    pub cwd: PathBuf,
    /// Only explicitly approved launch variables; never the caller's full environment.
    pub environment: BTreeMap<String, String>,
}

/// Caller targeting never falls back to whichever workspace has GUI focus.
pub enum WorkspaceSelector<'a> {
    Caller,
    Explicit(&'a str),
    Exact(&'a NativeRef),
}

/// Implementable, object-safe boundary for common native workspace effects.
///
/// Native presentation and Herdr topology use separate adapter-specific APIs:
/// CMUX descriptions/colors are not Herdr metadata, and CMUX has no attestation
/// or arrangement capability here. Errors retain command/effect evidence.
pub trait WorkspaceFrontend {
    fn frontend_id(&self) -> FrontendId;
    fn endpoint_id(&self) -> EndpointId;
    fn workspaces(&self) -> Result<Vec<Workspace>, FrontendError>;
    fn resolve(&self, selector: WorkspaceSelector<'_>) -> Result<Workspace, FrontendError>;
    fn create(&self, request: &CreateWorkspace) -> Result<Workspace, FrontendError>;
    fn focus(&self, reference: &NativeRef) -> Result<(), FrontendError>;
    fn rename(&self, reference: &NativeRef, title: &str) -> Result<(), FrontendError>;
}

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

/// Resume runs in the calling terminal, so it needs a frontend only to see whether the
/// conversation already runs: hc when selected, otherwise Herdr when it is configured or
/// the caller runs inside it.
pub fn resolve_resume_frontend(
    configured: FrontendMode,
    requested: Option<FrontendRequest>,
    context: &BTreeMap<String, String>,
) -> Result<Option<FrontendId>, FrontendSelectionError> {
    let herdr = configured == FrontendMode::Explicit(FrontendId::Herdr)
        || context.get("HERDR_ENV").is_some_and(|value| value == "1");
    Ok(
        match resolve_frontend(configured, requested, context)?.resolved {
            Some(FrontendId::Hc) => Some(FrontendId::Hc),
            _ if herdr => Some(FrontendId::Herdr),
            _ => None,
        },
    )
}

pub(crate) fn nonempty(value: &str, name: &str) -> Result<(), FrontendError> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        Err(FrontendError::invalid(format!("invalid {name}")))
    } else {
        Ok(())
    }
}

pub(crate) fn check_ref(
    frontend: &dyn WorkspaceFrontend,
    reference: &NativeRef,
) -> Result<(), FrontendError> {
    nonempty(&reference.key, "workspace ID")?;
    if reference.adapter != frontend.frontend_id() || reference.endpoint != frontend.endpoint_id() {
        return Err(FrontendError::invalid(
            "workspace frontend/endpoint mismatch",
        ));
    }
    Ok(())
}
