//! Independent domain, local-save, and publication facts for every caller.

use serde::Serialize;
use strum::{AsRefStr, Display, EnumString, VariantNames};

/// What happened to local persistence, independently of domain acceptance or sharing.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum LocalStatus {
    Unchanged,
    Persisted,
    Committed,
    RepairRequired,
}

/// Publication evidence only; confirmed publication can coexist with local repair.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum SharingStatus {
    LocalOnly,
    Pending,
    Confirmed,
    Blocked,
    Failed,
    Uncertain,
}

crate::codec::deserialize_string_enum!(LocalStatus, SharingStatus);

/// Orthogonal outcome dimensions; never infer acceptance or rollback from sharing alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Result {
    pub sharing: SharingStatus,
    pub local: LocalStatus,
    pub domain: Option<()>,
    pub local_revision: Option<String>,
    pub remote_revision: Option<String>,
    pub detail: String,
    pub conflicts: Vec<String>,
}

impl Result {
    pub fn new(sharing: SharingStatus, local: LocalStatus) -> Self {
        Self {
            sharing,
            local,
            domain: None,
            local_revision: None,
            remote_revision: None,
            detail: String::new(),
            conflicts: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn requires_repair(&self) -> bool {
        self.local == LocalStatus::RepairRequired
    }

    pub fn needs_attention(&self) -> bool {
        self.requires_repair()
            || matches!(
                self.sharing,
                SharingStatus::Blocked | SharingStatus::Failed | SharingStatus::Uncertain
            )
    }
}
