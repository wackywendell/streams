//! Managed Git records: locked local saves and isolated replay candidates.

#[cfg(test)]
mod tests;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::file_io::{check_catalog_root, check_empty_catalog_root};
use crate::model::{ValidationError, check_text};
use crate::operations::{Authority, Operation};
use crate::storage::{Snapshot, Transaction};
use crate::yaml_store::{YamlStore, YamlTransaction};

use super::envelope::{BOOTSTRAP, Origin, encode_operation};
use super::git_io::{Git, GitHook};
use super::history::{History, read_history};
use super::outcomes::{LocalStatus, Result as SyncResult, SharingStatus};
use super::policy::{Policy, SyncEvent};
use super::replay::ReplayChange;
use super::state::{GitConfig, RepairRequired, State, StateHook};
use super::{Oid, ReadResult};

pub(crate) use super::coordinator::CatalogError;
use super::coordinator::{Coordinator, Repository};

/// A view constructed only while this catalog's transaction owns its lock.
pub(super) struct LockedCatalog<'tx> {
    pub(super) root: &'tx Path,
    pub(super) store: &'tx YamlStore,
    pub(super) git: &'tx Git,
    pub(super) host_id: &'tx str,
    pub(super) policy: &'tx Policy,
    pub(super) clock: &'tx Arc<dyn Fn() -> f64 + Send + Sync>,
    tx: &'tx YamlTransaction,
    pub(super) state: State<'tx>,
}

impl LockedCatalog<'_> {
    fn coordinator(&self) -> Coordinator<'_> {
        Coordinator {
            io: self,
            state: &self.state,
            host_id: self.host_id,
            policy: self.policy,
            clock: self.clock.as_ref(),
        }
    }

    fn initialize(&self, remote: Option<String>, branch: &str) -> Result<(), CatalogError> {
        if let Some(ref remote) = remote {
            check_text(remote, "remote", false)?;
        }
        check_text(branch, "branch", false)?;
        if let Some(config) = self.state.config()? {
            if config.remote() != remote.as_deref()
                || config.host() != self.host_id
                || config.branch() != branch
            {
                return Err(repair(
                    "catalog configuration does not match this host, branch or remote",
                )
                .into());
            }
            self.ready()?;
            return Ok(());
        }
        self.state.check_save()?;
        check_empty_catalog_root(self.root)?;
        self.state.write_record(
            "save.json",
            &json!({"setup": true, "remote": remote, "branch": branch}),
        )?;
        self.git.run(
            &["init", &format!("--initial-branch={branch}")],
            None,
            None,
            &[0],
        )?;
        let branch_ref = format!("refs/heads/{branch}");
        self.git
            .run(&["check-ref-format", &branch_ref], None, None, &[0])?;
        if let Some(ref remote) = remote {
            self.git
                .run(&["remote", "add", "origin", remote], None, None, &[0])?;
        }
        let config = GitConfig::new(remote.clone(), branch, self.host_id).map_err(repair)?;
        self.state.write_config(&config)?;
        let remote_tip = if remote.is_some() {
            self.fetch()?
        } else {
            None
        };
        let tip = if let Some(tip) = remote_tip {
            read_history(self.git, Some(&tip))?;
            tip
        } else {
            let tree = self.git.text(&["write-tree"], None, None, &[0])?;
            self.git.commit(&tree, BOOTSTRAP, None)?
        };
        self.git
            .run(&["read-tree", "--reset", "-u", &tip], None, None, &[0])?;
        self.git
            .run(&["update-ref", &branch_ref, &tip], None, None, &[0])?;
        self.clean()?;
        read_history(self.git, Some(&tip))?;
        self.state.clear("save.json")?;
        Ok(())
    }

    pub(super) fn transaction(&self) -> &YamlTransaction {
        self.tx
    }

    pub(super) fn clean(&self) -> Result<(), CatalogError> {
        check_catalog_root(self.root)?;
        let entries = self.git.run(
            &[
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignored",
            ],
            None,
            None,
            &[0],
        )?;
        for entry in entries
            .split(|&byte| byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            if entry.len() < 4 {
                return Err(repair("invalid Git status entry").into());
            }
            let path = std::str::from_utf8(&entry[3..]).map_err(repair)?;
            let machinery = path == ".catalog.lock"
                || path.starts_with(".catalog-staging/")
                || path.starts_with(".catalog-sync/");
            if !matches!(&entry[..2], b"??" | b"!!") || !machinery {
                return Err(repair(format!(
                    "unexplained catalog index or working-tree change: {path}"
                ))
                .into());
            }
        }
        Ok(())
    }

    pub(super) fn config(&self) -> Result<GitConfig, RepairRequired> {
        self.coordinator().config()
    }

    pub(super) fn ready(&self) -> Result<GitConfig, CatalogError> {
        self.coordinator().ready()
    }

    pub(super) fn saved(&self) -> Result<(History, Snapshot), CatalogError> {
        self.coordinator().saved()
    }

    pub(super) fn fetch(&self) -> Result<Option<String>, CatalogError> {
        self.coordinator().fetch()
    }
}

impl Repository for LockedCatalog<'_> {
    fn check_layout(&self) -> Result<(), CatalogError> {
        let git_dir = self.root.join(".git");
        if git_dir.is_symlink() || !git_dir.is_dir() {
            return Err(repair("catalog must use its own Git directory").into());
        }
        Ok(())
    }
    fn check_target(&self, config: &GitConfig) -> Result<(), RepairRequired> {
        self.git.check_target(config)
    }

    fn clean(&self) -> Result<(), CatalogError> {
        LockedCatalog::clean(self)
    }
    fn head(&self) -> Result<String, CatalogError> {
        Ok(self.git.head()?)
    }
    fn history(&self, tip: Option<&str>) -> Result<History, CatalogError> {
        Ok(read_history(self.git, tip)?)
    }
    fn ancestor(&self, before: &str, after: &str) -> Result<bool, CatalogError> {
        Ok(self.git.ancestor(before, after)?)
    }
    fn snapshot(&self) -> Result<Snapshot, CatalogError> {
        Ok(self.tx.read()?)
    }
    fn fetch(&self, branch: &str) -> Result<Option<String>, CatalogError> {
        Ok(self.git.fetch_tip(branch)?)
    }
    fn candidate(
        &self,
        local: &History,
        remote: &History,
        changes: Vec<ReplayChange>,
    ) -> Result<String, CatalogError> {
        let mut base = remote
            .tip()
            .or(local.root())
            .map(str::to_owned)
            .ok_or_else(|| repair("candidate requires a supported history root"))?;
        let directory = tempfile::Builder::new()
            .prefix("streams-candidate-")
            .tempdir()?;
        let root = directory.path().join("records");
        fs::create_dir(&root)?;
        for (name, contents) in remote.files() {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, contents)?;
        }
        let store = YamlStore::new(&root);
        let index = directory.path().join("index");
        self.git
            .run(&["read-tree", &base], None, Some(&index), &[0])?;
        store.with_transaction(|tx| -> Result<(), CatalogError> {
            for change in changes {
                let (operation, origin, record) = (change.operation, change.origin, change.record);
                let before = tx.read()?;
                let previous_path = before
                    .catalog
                    .get(&record.key())
                    .map(|prior| store.record_path(&prior));
                tx.save(&record, &before)?;
                let path = store.record_path(&record);
                if let Some(previous) = previous_path.filter(|previous| previous != &path) {
                    self.git.run(
                        &[
                            "update-index",
                            "--force-remove",
                            "--",
                            &previous.to_string_lossy(),
                        ],
                        None,
                        Some(&index),
                        &[0],
                    )?;
                }
                let bytes = fs::read(root.join(&path))?;
                let blob =
                    self.git
                        .text(&["hash-object", "-w", "--stdin"], Some(&bytes), None, &[0])?;
                let cacheinfo = format!("100644,{blob},{}", path.to_string_lossy());
                self.git.run(
                    &["update-index", "--add", "--cacheinfo", &cacheinfo],
                    None,
                    Some(&index),
                    &[0],
                )?;
                let tree = self.git.text(&["write-tree"], None, Some(&index), &[0])?;
                base = self.git.commit(
                    &tree,
                    &encode_operation(&operation, Some(&origin))?,
                    Some(&base),
                )?;
            }
            Ok(())
        })?;
        read_history(self.git, Some(&base))?;
        Ok(base)
    }

    fn retain(&self, revision: &str) -> Result<(), CatalogError> {
        let retained = format!("refs/catalog/retained/{}", uuid::Uuid::new_v4());
        self.git
            .run(&["update-ref", &retained, revision], None, None, &[0])?;
        Ok(())
    }

    fn push(&self, candidate: &Oid, branch: &str) -> Result<(), CatalogError> {
        Ok(self.git.push_candidate(candidate, branch)?)
    }
    fn checkout(&self, old: &Oid, new: &Oid) -> Result<(), CatalogError> {
        let old = old.as_str();
        let new = new.as_str();
        if old != new {
            self.git
                .run(&["read-tree", "-m", "-u", old, new], None, None, &[0])?;
            let branch = format!("refs/heads/{}", self.config()?.branch());
            self.git
                .run(&["update-ref", &branch, new, old], None, None, &[0])?;
        }
        self.clean()?;
        if self.git.head()? != new {
            return Err(repair("adopted branch does not match the intended history").into());
        }
        let verified = read_history(self.git, Some(new))?;
        for (path, contents) in verified.files() {
            if fs::read(self.root.join(path))? != *contents {
                return Err(repair("adopted records disagree with committed history").into());
            }
        }
        Ok(())
    }

    fn commit_record(
        &self,
        record: &crate::model::Record,
        expected: &Snapshot,
        start: Option<&str>,
        message: &str,
    ) -> Result<String, CatalogError> {
        let previous_path = expected
            .catalog
            .get(&record.key())
            .map(|before| self.store.record_path(&before));
        self.tx.save(record, expected)?;
        let path = self.store.record_path(record);
        let mut paths = vec![path.to_string_lossy().into_owned()];
        if let Some(previous) = previous_path.filter(|previous| previous != &path) {
            paths.push(previous.to_string_lossy().into_owned());
        }
        let mut args = vec!["add", "--all", "--"];
        args.extend(paths.iter().map(String::as_str));
        self.git.run(&args, None, None, &[0])?;
        let tree = self.git.text(&["write-tree"], None, None, &[0])?;
        let commit = self.git.commit(&tree, message, start)?;
        let branch = format!("refs/heads/{}", self.config()?.branch());
        let start = start.ok_or_else(|| repair("managed catalog has no saved tip"))?;
        self.git
            .run(&["update-ref", &branch, &commit, start], None, None, &[0])?;
        self.clean()?;
        let verified = read_history(self.git, Some(&commit))?;
        if &self.tx.read()?.catalog != verified.catalog() {
            return Err(repair("saved records disagree with committed history").into());
        }
        Ok(commit)
    }
}

fn repair(error: impl std::fmt::Display) -> RepairRequired {
    RepairRequired::new(error.to_string())
}

/// Managed operation commits with separate remote publication and local recovery fences.
pub struct GitCatalog {
    root: PathBuf,
    store: YamlStore,
    hook: Option<StateHook>,
    git: Git,
    host_id: String,
    policy: Policy,
    clock: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl std::fmt::Debug for GitCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitCatalog")
            .field("root", &self.root)
            .field("store", &self.store)
            .field("git", &self.git)
            .field("host_id", &self.host_id)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl GitCatalog {
    /// Open a handle without creating or adopting a repository; setup is explicit.
    pub fn new(
        root: impl AsRef<Path>,
        host_id: impl Into<String>,
        policy: Policy,
    ) -> Result<Self, ValidationError> {
        let host_id = host_id.into();
        check_text(&host_id, "host_id", false)?;
        let root = std::path::absolute(root)
            .map_err(|e| ValidationError::new(format!("cannot resolve catalog root: {e}")))?;
        Ok(Self {
            store: YamlStore::new(&root),
            hook: None,
            git: Git::new(&root, policy.timeout_seconds()),
            root,
            host_id,
            policy,
            clock: Arc::new(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or_else(|e| -e.duration().as_secs_f64(), |d| d.as_secs_f64())
            }),
        })
    }

    /// Catalog directory fixed when this handle was constructed.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store(&self) -> &YamlStore {
        &self.store
    }

    pub fn git(&self) -> &Git {
        &self.git
    }

    pub fn host_id(&self) -> &str {
        &self.host_id
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Install command observation or fault injection without changing root or timeout.
    pub fn set_git_hook(&mut self, hook: GitHook) {
        self.git = self.git.clone().with_hook(hook);
    }

    /// Install journal fault injection without changing the owning persistence scope.
    pub fn set_state_hook(&mut self, hook: StateHook) {
        self.hook = Some(hook);
    }

    pub fn set_clock(&mut self, clock: Arc<dyn Fn() -> f64 + Send + Sync>) {
        self.clock = clock;
    }

    fn locked<'tx>(&'tx self, tx: &'tx YamlTransaction) -> LockedCatalog<'tx> {
        LockedCatalog {
            root: &self.root,
            store: &self.store,
            git: &self.git,
            host_id: &self.host_id,
            policy: &self.policy,
            clock: &self.clock,
            tx,
            state: State::from_journal(tx.journal()).with_optional_hook(self.hook.clone()),
        }
    }

    pub(super) fn with_lock<T>(
        &self,
        f: impl FnOnce(&LockedCatalog<'_>) -> Result<T, CatalogError>,
    ) -> Result<T, CatalogError> {
        self.store.with_transaction(|tx| f(&self.locked(tx)))
    }

    /// Initialize an empty directory from a remote or a new bootstrap; failed setup stays fenced.
    pub fn initialize(self, remote: Option<String>, branch: &str) -> Result<Self, RepairRequired> {
        self.with_lock(|catalog| catalog.initialize(remote, branch))
            .map_err(repair)?;
        Ok(self)
    }

    /// Read durable local records only after validating managed history and recovery fences.
    pub fn snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.with_lock(|catalog| catalog.coordinator().snapshot())
            .map_err(repair)
    }

    /// Read actual local records and authority without repairing retry evidence.
    pub fn inspect(&self) -> Result<super::Inspection, RepairRequired> {
        self.store
            .with_inspection(|tx| self.locked(tx).coordinator().inspect())
            .map_err(repair)
    }

    /// Combine confirmed ownership with provenance for locally created streams.
    pub fn authority(&self, stream: &str) -> Result<Option<Authority>, RepairRequired> {
        self.with_lock(|catalog| catalog.coordinator().authority(stream))
            .map_err(repair)
    }

    /// Commit one accepted record and its original intent before issuing its retry receipt.
    pub fn save(
        &self,
        operation: &Operation,
        origin: Option<&Origin>,
    ) -> Result<SyncResult, ValidationError> {
        // Preserve validation before attempting to acquire the filesystem lock.
        operation.check_producer(&self.host_id)?;
        encode_operation(operation, origin)?;
        self.with_lock(|catalog| Ok(catalog.coordinator().save(operation, origin)))
            .unwrap_or_else(|error| {
                Ok(
                    SyncResult::new(SharingStatus::Blocked, LocalStatus::RepairRequired)
                        .with_detail(error.to_string()),
                )
            })
    }

    /// Fetch when due and project intent without pushing, adopting or granting authority.
    pub fn read(&self, refresh: bool) -> Result<ReadResult, RepairRequired> {
        self.with_lock(|catalog| catalog.coordinator().read(refresh))
            .map_err(repair)
    }

    /// Force fetch-only inspection without advancing publication clocks.
    pub fn inspect_remote(&self) -> Result<Snapshot, RepairRequired> {
        self.with_lock(|catalog| catalog.coordinator().inspect_remote())
            .map_err(repair)
    }

    /// Read the last validated remote snapshot without fetching or projecting local intent.
    pub fn confirmed_snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.with_lock(|catalog| catalog.coordinator().confirmed_snapshot())
            .map_err(repair)
    }

    /// Synchronize the entire pending batch; reads and local saves never call this implicitly.
    pub fn sync(&self, event: SyncEvent) -> SyncResult {
        self.with_lock(|catalog| Ok(catalog.coordinator().sync(event)))
            .unwrap_or_else(|error| {
                SyncResult::new(SharingStatus::Failed, LocalStatus::Unchanged)
                    .with_detail(error.to_string())
            })
    }
}
