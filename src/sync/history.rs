//! Validate materialized Git history against the original domain operations.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{Catalog, Record, RecordKey};
use crate::operations::{Authority, Operation, reconcile};
use crate::yaml_store::{YamlStore, relative_record_path};

use super::baseline::{BASELINE, Baseline, decode_baseline};
use super::envelope::{BOOTSTRAP, Origin, check_fingerprints, decode_operation, operation_digest};
use super::git_io::Git;
use super::local::authority_for;
use super::state::RepairRequired;

pub fn records(catalog: &Catalog) -> BTreeMap<RecordKey, Record> {
    catalog
        .records()
        .into_iter()
        .map(|r| (r.key(), r))
        .collect()
}

pub type HistoryOperations = Vec<(String, (Operation, Origin))>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub tip: Option<String>,
    pub root: Option<String>,
    pub catalog: Catalog,
    pub files: BTreeMap<String, Vec<u8>>,
    pub authorities: BTreeMap<String, Authority>,
    pub operations: HistoryOperations,
    pub baseline: Option<Baseline>,
    pub fingerprints: BTreeMap<String, String>,
}

impl History {
    pub fn validated(mut self) -> Result<Self, String> {
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
        for (id, (operation, _)) in &self.operations {
            if id != &operation.id {
                return Err("history operation key must match its ID".to_string());
            }
            let digest = operation_digest(operation);
            if let Some(existing) = fps.get(id)
                && existing != &digest
            {
                return Err(format!("inconsistent operation fingerprint: {id}"));
            }
            fps.insert(id.clone(), digest);
        }
        self.fingerprints = fps;
        Ok(self)
    }

    pub fn operation(&self, id: &str) -> Option<&(Operation, Origin)> {
        self.operations
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, v)| v)
    }

    pub fn has_operation(&self, id: &str) -> bool {
        self.operations.iter().any(|(k, _)| k == id)
    }
}

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
    let mut previous: Option<String> = None;
    {
        let cache = git.histories.lock().unwrap();
        for (index, (revision, _, _)) in commits.iter().enumerate() {
            let rev_key = Some((*revision).to_string());
            if let Some((_, cached)) = cache.iter().find(|(k, _)| k == &rev_key) {
                history = cached.clone();
                start = index + 1;
                previous = Some((*revision).to_string());
            }
        }
    }

    for (revision, parent_text, raw_message) in &commits[start..] {
        let parents: Vec<&str> = parent_text.split_whitespace().collect();
        let files = git.files(revision).map_err(|e| e.to_string())?;
        let message = format!("{}\n", raw_message.trim_end());
        if previous.is_none() {
            if !parents.is_empty() {
                return Err("catalog history root must have no parents".to_string());
            }
            if message.starts_with(BASELINE) {
                let (baseline, catalog, authorities) =
                    decode_baseline(&message, &files).map_err(|e| e.to_string())?;
                history = History {
                    tip: Some((*revision).to_string()),
                    root: Some((*revision).to_string()),
                    catalog,
                    files,
                    authorities,
                    operations: Vec::new(),
                    baseline: Some(baseline),
                    fingerprints: BTreeMap::new(),
                }
                .validated()?;
            } else if message == BOOTSTRAP && files.is_empty() {
                history = History {
                    tip: Some((*revision).to_string()),
                    root: Some((*revision).to_string()),
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
            if parents.as_slice() != [previous.as_deref().unwrap()] {
                return Err("catalog history must be linear and non-merged".to_string());
            }
            let (operation, origin) = decode_operation(&message).map_err(|e| e.to_string())?;
            if history.fingerprints.contains_key(&operation.id) {
                return Err(format!(
                    "duplicate operation ID in history: {}",
                    operation.id
                ));
            }
            let auth = authority_for(&operation, &history.authorities);
            let result = reconcile(&history.catalog, &operation, auth.as_ref());
            if result.outcome != "changes" {
                return Err(format!(
                    "operation {} does not explain its commit: {}",
                    operation.id, result.outcome
                ));
            }
            let all_keys: BTreeSet<&String> = files.keys().chain(history.files.keys()).collect();
            let changed_paths: BTreeSet<String> = all_keys
                .into_iter()
                .filter(|k| files.get(*k) != history.files.get(*k))
                .cloned()
                .collect();
            let expected_path = relative_record_path(&operation.target())
                .to_string_lossy()
                .replace('\\', "/");
            let mut expected_set = BTreeSet::new();
            expected_set.insert(expected_path.clone());
            if changed_paths != expected_set || !files.contains_key(&expected_path) {
                return Err(format!(
                    "commit changes do not match operation {}",
                    operation.id
                ));
            }
            let decoded_record = YamlStore::decode_record(&expected_path, &files[&expected_path])
                .map_err(|e| e.to_string())?;
            let actual = history
                .catalog
                .replacing(decoded_record)
                .map_err(|e| e.to_string())?;
            let expected = history
                .catalog
                .replacing(result.record.unwrap())
                .map_err(|e| e.to_string())?;
            if records(&actual) != records(&expected) {
                return Err(format!(
                    "commit changes do not match operation {}",
                    operation.id
                ));
            }
            let mut new_authority = None;
            if let Record::Stream(ref s) = operation.after
                && operation.before.is_none()
            {
                new_authority = Some((
                    s.id.clone(),
                    Authority::new(&s.owner_host_id, &operation.id).map_err(|e| e.to_string())?,
                ));
            }
            let digest = operation_digest(&operation);
            if let Some((id, authority)) = new_authority {
                history.authorities.insert(id, authority);
            }
            history.fingerprints.insert(operation.id.clone(), digest);
            history
                .operations
                .push((operation.id.clone(), (operation, origin)));
            history.tip = Some((*revision).to_string());
            history.catalog = actual;
            history.files = files;
        }
        previous = Some((*revision).to_string());
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
