//! Independent domain, local-save, and publication facts for every caller.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::operations::Reconciliation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalStatus {
    Unchanged,
    Persisted,
    Committed,
    RepairRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SharingStatus {
    LocalOnly,
    Pending,
    Confirmed,
    Blocked,
    Failed,
    Uncertain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Result {
    pub sharing: SharingStatus,
    pub local: LocalStatus,
    pub domain: Option<Reconciliation>,
    pub local_revision: Option<String>,
    pub remote_revision: Option<String>,
    pub detail: String,
    pub conflicts: Vec<Value>,
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

    pub fn with_domain(mut self, domain: Reconciliation) -> Self {
        self.domain = Some(domain);
        self
    }

    pub fn with_local_rev(mut self, rev: Option<String>) -> Self {
        self.local_revision = rev;
        self
    }

    pub fn with_remote_rev(mut self, rev: Option<String>) -> Self {
        self.remote_revision = rev;
        self
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self
    }

    pub fn with_conflicts(mut self, conflicts: Vec<Value>) -> Self {
        self.conflicts = conflicts;
        self
    }

    pub fn accepted(&self) -> bool {
        !self.requires_repair()
            && self
                .domain
                .as_ref()
                .is_some_and(|d| d.outcome == "changes" || d.outcome == "no-change")
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
