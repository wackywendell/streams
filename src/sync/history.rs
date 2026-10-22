//! Validate materialized Git history against the original domain operations.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};

use crate::model::Catalog;
use crate::operations::{Authority, Operation, Reconciliation, reconcile};
use crate::yaml_store::{YamlStore, relative_record_path};

use super::baseline::{BASELINE, Baseline, decode_baseline};
use super::envelope::{BOOTSTRAP, Origin, check_fingerprints, decode_operation, operation_digest};
use super::git_io::Git;
use super::state::RepairRequired;

/// An operation and its provenance, in replay order; its ID lives only in the operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedOperation {
    pub operation: Operation,
    pub origin: Origin,
}

/// A decoded linear catalog history, its provenance and remembered operation fingerprints.
/// Only `read_history` verifies Git ancestry and each operation's exact file changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    tip: Option<String>,
    root: Option<String>,
    catalog: Catalog,
    files: BTreeMap<String, Vec<u8>>,
    authorities: BTreeMap<String, Authority>,
    operations: Vec<CommittedOperation>,
    baseline: Option<Baseline>,
    fingerprints: BTreeMap<String, String>,
}

impl History {
    pub fn tip(&self) -> Option<&str> {
        self.tip.as_deref()
    }

    pub fn root(&self) -> Option<&str> {
        self.root.as_deref()
    }

    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }

    pub fn authorities(&self) -> &BTreeMap<String, Authority> {
        &self.authorities
    }

    pub fn operations(&self) -> &[CommittedOperation] {
        &self.operations
    }

    pub fn baseline(&self) -> Option<&Baseline> {
        self.baseline.as_ref()
    }

    pub fn fingerprints(&self) -> &BTreeMap<String, String> {
        &self.fingerprints
    }

    /// Check and combine fingerprint evidence; this alone does not validate Git commits.
    fn validated(mut self) -> Result<Self, String> {
        let mut fps = self
            .baseline
            .as_ref()
            .map(|b| b.fingerprints.clone())
            .unwrap_or_default();
        check_fingerprints(&self.fingerprints).map_err(|e| e.to_string())?;
        for (id, digest) in &self.fingerprints {
            if let Some(existing) = fps.get(id)
                && existing != digest
            {
                return Err(format!("inconsistent operation fingerprint: {id}"));
            }
            fps.insert(id.clone(), digest.clone());
        }
        for entry in &self.operations {
            let id = entry.operation.id();
            let digest = operation_digest(&entry.operation);
            if let Some(existing) = fps.get(id)
                && existing != &digest
            {
                return Err(format!("inconsistent operation fingerprint: {id}"));
            }
            fps.insert(id.to_owned(), digest);
        }
        self.fingerprints = fps;
        Ok(self)
    }

    /// Apply our catalog validation to already-observed commit facts; no Git or filesystem I/O.
    pub(super) fn append(
        mut self,
        revision: &str,
        parents: &[&str],
        message: &str,
        files: BTreeMap<String, Vec<u8>>,
    ) -> Result<Self, String> {
        if self.tip.is_none() {
            if !parents.is_empty() {
                return Err("catalog history root must have no parents".to_string());
            }
            if message.starts_with(BASELINE) {
                let (baseline, catalog, authorities) =
                    decode_baseline(message, &files).map_err(|e| e.to_string())?;
                self = History {
                    tip: Some(revision.to_owned()),
                    root: Some(revision.to_owned()),
                    catalog,
                    files,
                    authorities,
                    operations: Vec::new(),
                    baseline: Some(baseline),
                    fingerprints: BTreeMap::new(),
                }
                .validated()?;
            } else if message == BOOTSTRAP && files.is_empty() {
                self = History {
                    tip: Some(revision.to_owned()),
                    root: Some(revision.to_owned()),
                    files,
                    ..History::default()
                }
                .validated()?;
            } else {
                return Err(
                    "catalog history must start with an empty bootstrap or validated baseline"
                        .to_string(),
                );
            }
        } else {
            if parents != [self.tip.as_deref().unwrap()] {
                return Err("catalog history must be linear and non-merged".to_string());
            }
            let (operation, origin) = decode_operation(message).map_err(|e| e.to_string())?;
            if self.fingerprints.contains_key(operation.id()) {
                return Err(format!(
                    "duplicate operation ID in history: {}",
                    operation.id()
                ));
            }
            let auth = operation.authority_for(&self.authorities);
            let result = reconcile(&self.catalog, &operation, auth);
            let Reconciliation::Changes { record, .. } = result else {
                return Err(format!(
                    "operation {} does not explain its commit: {}",
                    operation.id(),
                    result.outcome()
                ));
            };
            let all_keys: BTreeSet<&String> = files.keys().chain(self.files.keys()).collect();
            let changed_paths: BTreeSet<String> = all_keys
                .into_iter()
                .filter(|k| files.get(*k) != self.files.get(*k))
                .cloned()
                .collect();
            let expected_path = relative_record_path(operation.after())
                .to_string_lossy()
                .replace('\\', "/");
            let mut expected_set = BTreeSet::new();
            expected_set.insert(expected_path.clone());
            if changed_paths != expected_set || !files.contains_key(&expected_path) {
                return Err(format!(
                    "commit changes do not match operation {}",
                    operation.id()
                ));
            }
            let decoded_record = YamlStore::decode_record(&expected_path, &files[&expected_path])
                .map_err(|e| e.to_string())?;
            if decoded_record != *record {
                return Err(format!(
                    "commit changes do not match operation {}",
                    operation.id()
                ));
            }
            let actual = self
                .catalog
                .replacing(decoded_record)
                .map_err(|e| e.to_string())?;
            let digest = operation_digest(&operation);
            if let Some((id, authority)) = operation.created_authority() {
                self.authorities.insert(id, authority);
            }
            self.fingerprints.insert(operation.id().to_owned(), digest);
            self.operations
                .push(CommittedOperation { operation, origin });
            self.tip = Some(revision.to_owned());
            self.catalog = actual;
            self.files = files;
        }
        Ok(self)
    }

    /// Returns a replayable operation, which need not exist for baseline-only fingerprints.
    pub fn operation(&self, id: &str) -> Option<&CommittedOperation> {
        self.operations
            .iter()
            .find(|entry| entry.operation.id() == id)
    }

    /// Includes compacted operations remembered only by a baseline fingerprint.
    pub fn has_operation(&self, id: &str) -> bool {
        self.fingerprints.contains_key(id)
    }
}

/// Accept only supported linear roots and operation commits consistent with their record edits.
/// Arbitrary Git history is a repair condition, not input to a textual merge or rebase.
pub fn read_history(git: &Git, tip: Option<&str>) -> Result<History, RepairRequired> {
    read_history_inner(git, tip)
        .map_err(|e| RepairRequired::new(format!("unsupported catalog history: {e}")))
}

fn read_history_inner(git: &Git, tip: Option<&str>) -> Result<History, String> {
    let tip_key = tip.map(str::to_string);
    {
        let cache = git.histories.lock().unwrap();
        if let Some((_, h)) = cache.iter().find(|(k, _)| k == &tip_key) {
            return Ok(h.clone());
        }
    }
    let Some(tip_str) = tip else {
        return History::default().validated();
    };
    let raw = git
        .run(
            &[
                "log",
                "-z",
                "--reverse",
                "--topo-order",
                "--format=%H%x00%P%x00%B",
                tip_str,
            ],
            None,
            None,
            &[0],
        )
        .map_err(|e| e.message)?;
    let decoded = String::from_utf8(raw).map_err(|e| e.to_string())?;
    let mut entries: Vec<&str> = decoded.split('\0').collect();
    if entries.last() == Some(&"") {
        entries.pop();
    }
    if !entries.len().is_multiple_of(3) {
        return Err("invalid Git commit metadata".to_string());
    }
    let commits: Vec<(&str, &str, &str)> = entries
        .chunks_exact(3)
        .map(|c| (c[0], c[1], c[2]))
        .collect();

    let mut history = History::default().validated()?;
    let mut start = 0usize;
    {
        let cache = git.histories.lock().unwrap();
        for (index, (revision, _, _)) in commits.iter().enumerate() {
            let rev_key = Some((*revision).to_string());
            if let Some((_, cached)) = cache.iter().find(|(k, _)| k == &rev_key) {
                history = cached.clone();
                start = index + 1;
            }
        }
    }

    for (revision, parent_text, raw_message) in &commits[start..] {
        let parents: Vec<&str> = parent_text.split_whitespace().collect();
        let files = git.files(revision).map_err(|e| e.to_string())?;
        let message = format!("{}\n", raw_message.trim_end());
        history = history.append(revision, &parents, &message, files)?;
    }

    let mut cache = git.histories.lock().unwrap();
    if let Some(pos) = cache.iter().position(|(k, _)| k == &tip_key) {
        cache.remove(pos);
    }
    cache.push((tip_key, history.clone()));
    if cache.len() > 8 {
        cache.remove(0);
    }
    Ok(history)
}
