//! Managed Git records: locked local saves and isolated replay candidates.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::file_io::{check_catalog_root, check_empty_catalog_root};
use crate::model::{ValidationError, check_text};
use crate::operations::{Authority, DomainOutcome, Operation, Reconciliation, reconcile};
use crate::storage::{Snapshot, StorageError, Transaction};
use crate::yaml_store::{YamlStore, YamlTransaction};

use super::envelope::{BOOTSTRAP, Origin, encode_operation};
use super::git_io::{Git, GitError, GitHook};
use super::history::{History, read_history};
use super::outcomes::{LocalStatus, Result as SyncResult, SharingStatus};
use super::policy::{Policy, SyncEvent, Timing};
use super::protocol::{
    ProtocolMachine,
    publication::{Recovery, RetainedAttempt},
};
use super::replay::{Replay, ReplayChange, replay};
use super::state::{
    GitConfig, PublicationAttempt, PublicationMarker, RepairRequired, State, StateHook,
    SyncMetadata,
};
use super::{Freshness, Oid, ReadResult, Seconds};

#[derive(Debug, thiserror::Error)]
pub(crate) enum CatalogError {
    #[error(transparent)]
    Git(#[from] GitError),
    #[error(transparent)]
    Repair(#[from] RepairRequired),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

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
    pub(super) fn now(&self) -> Result<Seconds, CatalogError> {
        Seconds::try_from((self.clock)()).map_err(|e| repair(e).into())
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

    pub(super) fn config(&self) -> Result<GitConfig, RepairRequired> {
        let config = self
            .state
            .config()?
            .filter(|c| c.host() == self.host_id)
            .ok_or_else(|| repair("catalog is not initialized for this host"))?;
        let head = self
            .git
            .text(&["symbolic-ref", "--quiet", "HEAD"], None, None, &[0])
            .map_err(repair)?;
        if head != format!("refs/heads/{}", config.branch()) {
            return Err(repair("managed catalog branch was changed"));
        }
        match config.remote() {
            None => {
                if !self
                    .git
                    .text(&["remote"], None, None, &[0])
                    .map_err(repair)?
                    .is_empty()
                {
                    return Err(repair(
                        "local Git catalog has an unexpected remote; inspect before continuing",
                    ));
                }
            }
            Some(remote) => {
                let fetch = self
                    .git
                    .text(&["remote", "get-url", "--all", "origin"], None, None, &[0])
                    .map_err(repair)?;
                let push = self
                    .git
                    .text(
                        &["remote", "get-url", "--push", "--all", "origin"],
                        None,
                        None,
                        &[0],
                    )
                    .map_err(repair)?;
                let mirror = self
                    .git
                    .text(
                        &["config", "--get", "remote.origin.mirror"],
                        None,
                        None,
                        &[0, 1],
                    )
                    .map_err(repair)?;
                if fetch != remote || push != remote || (!mirror.is_empty() && mirror != "false") {
                    return Err(repair("managed catalog remote was changed"));
                }
            }
        }
        Ok(config)
    }

    pub(super) fn ready(&self) -> Result<GitConfig, CatalogError> {
        self.state.check_save()?;
        let git_dir = self.root.join(".git");
        if git_dir.is_symlink() || !git_dir.is_dir() {
            return Err(repair("catalog must use its own Git directory").into());
        }
        let config = self.config()?;
        let metadata = self.state.metadata()?;
        let publication = self.state.publication()?;
        if config.remote().is_none()
            && (metadata != SyncMetadata::default() || publication.is_some())
        {
            return Err(repair(
                "local Git catalog has remote synchronization state; inspect before continuing",
            )
            .into());
        }
        if let Some(ref publication) = publication {
            let attempt = publication.attempt();
            if attempt.branch() != config.branch() || Some(attempt.remote()) != config.remote() {
                return Err(repair(
                    "publication marker does not match the configured remote and branch",
                )
                .into());
            }
        }
        if matches!(publication, Some(PublicationMarker::Adopting(_))) {
            return Err(repair(
                "interrupted local adoption; inspect publication.json and retained refs",
            )
            .into());
        }
        self.clean().map_err(repair)?;
        Ok(config)
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

    pub(super) fn history(&self) -> Result<History, CatalogError> {
        let history = read_history(self.git, Some(&self.git.head()?))?;
        self.state
            .validate_fingerprints(history.fingerprints(), true)?;
        Ok(history)
    }

    pub(super) fn saved(&self) -> Result<(History, Snapshot), CatalogError> {
        let history = self.history()?;
        let snapshot = self.tx.read()?;
        if &snapshot.catalog != history.catalog() {
            return Err(repair("working records disagree with supported history").into());
        }
        Ok((history, snapshot))
    }

    pub(super) fn authorities(
        &self,
        local: &History,
    ) -> Result<BTreeMap<String, Authority>, CatalogError> {
        let remote = read_history(
            self.git,
            self.state.metadata()?.confirmed.as_ref().map(Oid::as_str),
        )?;
        let mut authorities = local.authorities().clone();
        authorities.extend(remote.authorities().clone());
        Ok(authorities)
    }

    pub(super) fn fetch(&self) -> Result<Option<String>, CatalogError> {
        let config = self.config()?;
        if config.remote().is_none() {
            return Err(repair("local Git catalog has no remote to fetch").into());
        }
        let branch = format!("refs/heads/{}", config.branch());
        let found = self.git.text(
            &["ls-remote", "--heads", "origin", &branch],
            None,
            None,
            &[0],
        )?;
        let tip = if found.is_empty() {
            None
        } else {
            self.git
                .run(&["fetch", "--no-tags", "origin", &branch], None, None, &[0])?;
            Some(
                self.git
                    .text(&["rev-parse", "--verify", "FETCH_HEAD"], None, None, &[0])?,
            )
        };
        let history = read_history(self.git, tip.as_deref())?;
        let mut metadata = self.state.metadata()?;
        if let Some(previous) = &metadata.confirmed
            && (tip.is_none()
                || !self
                    .git
                    .ancestor(previous.as_str(), tip.as_deref().unwrap())?)
        {
            return Err(repair("confirmed remote history was removed or replaced").into());
        }
        if tip.is_some() {
            self.state
                .validate_fingerprints(history.fingerprints(), true)?;
        }
        metadata.confirmed = tip.clone().map(Oid::try_from).transpose().map_err(repair)?;
        metadata.fetched_at = Some(self.now()?);
        metadata.refresh_retry_after = Seconds::ZERO;
        metadata.refresh_error.clear();
        self.state.write_metadata(&metadata)?;
        Ok(tip)
    }

    pub(super) fn candidate(
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
                tx.save(&record, &tx.read()?)?;
                let path = store.record_path(&record);
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

    pub(super) fn retain(&self, revision: &str) -> Result<(), CatalogError> {
        let retained = format!("refs/catalog/retained/{}", uuid::Uuid::new_v4());
        self.git
            .run(&["update-ref", &retained, revision], None, None, &[0])?;
        Ok(())
    }

    pub(super) fn adopt(
        &self,
        old: &Oid,
        new: &Oid,
        marker: &PublicationAttempt,
    ) -> Result<(), CatalogError> {
        self.state.write_publication(&PublicationMarker::Adopting(
            marker.adopting(old.clone(), new.clone()),
        ))?;
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
        self.state.clear("publication.json")?;
        Ok(())
    }

    pub(super) fn record_confirmation(
        &self,
        marker: &PublicationAttempt,
    ) -> Result<PublicationAttempt, CatalogError> {
        let mut metadata = self.state.metadata()?;
        if let Some(timestamp) = marker.confirmed_at() {
            if metadata.last_confirmed_attempt.as_ref() != Some(marker.candidate())
                || metadata.last_push != Some(timestamp)
            {
                return Err(repair(
                    "publication confirmation disagrees with its scheduling receipt",
                )
                .into());
            }
            return Ok(marker.clone());
        }
        if metadata.last_confirmed_attempt.as_ref() != Some(marker.candidate()) {
            metadata.last_push = Some(self.now()?);
            metadata.last_confirmed_attempt = Some(marker.candidate().clone());
            self.state.write_metadata(&metadata)?;
        }
        let confirmed = marker.clone().with_confirmed_at(metadata.last_push);
        self.state
            .write_publication(&PublicationMarker::Attempting(confirmed.clone()))?;
        Ok(confirmed)
    }

    pub(super) fn success(&self) -> Result<(), CatalogError> {
        let mut metadata = self.state.metadata()?;
        metadata.last_sync = Some(self.now()?);
        metadata.retry_after = Seconds::ZERO;
        self.state.write_metadata(&metadata)?;
        Ok(())
    }

    pub(super) fn failure(&self) -> Result<(), CatalogError> {
        let mut metadata = self.state.metadata()?;
        metadata.retry_after =
            Seconds::try_from(self.now()?.get() + self.policy.cooldown_seconds())
                .map_err(repair)?;
        self.state.write_metadata(&metadata)?;
        Ok(())
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
        self.with_lock(|catalog| {
            catalog.ready()?;
            Ok(catalog.saved()?.1)
        })
        .map_err(repair)
    }

    /// Read actual local records and authority without repairing retry evidence.
    pub fn inspect(&self) -> Result<super::Inspection, RepairRequired> {
        self.store
            .with_inspection(|tx| -> Result<_, CatalogError> {
                let catalog = self.locked(tx);
                catalog.ready()?;
                let history = read_history(&self.git, Some(&self.git.head()?))?;
                catalog
                    .state
                    .validate_fingerprints(history.fingerprints(), false)?;
                let snapshot = tx.read()?;
                if &snapshot.catalog != history.catalog() {
                    return Err(repair("working records disagree with supported history").into());
                }
                let authorities = catalog.authorities(&history)?;
                Ok(super::Inspection {
                    snapshot,
                    authorities,
                })
            })
            .map_err(repair)
    }

    /// Combine confirmed ownership with provenance for locally created streams.
    pub fn authority(&self, stream: &str) -> Result<Option<Authority>, RepairRequired> {
        self.with_lock(|catalog| {
            catalog.ready()?;
            Ok(catalog
                .authorities(&catalog.history()?)?
                .get(stream)
                .cloned())
        })
        .map_err(repair)
    }

    /// Commit one accepted record and its original intent before issuing its retry receipt.
    pub fn save(
        &self,
        operation: &Operation,
        origin: Option<&Origin>,
    ) -> Result<SyncResult, ValidationError> {
        operation.check_producer(&self.host_id)?;
        let mut origin = origin.cloned().unwrap_or_default();
        origin
            .entry("host".into())
            .or_insert_with(|| self.host_id.clone());
        encode_operation(operation, Some(&origin))?;
        let mut sharing = SharingStatus::Blocked;
        let outcome = self.with_lock(|catalog| {
            let config = catalog.state.config()?;
            sharing = if config.is_some_and(|c| c.remote().is_none()) {
                SharingStatus::LocalOnly
            } else {
                SharingStatus::Pending
            };
            catalog.ready()?;
            let (history, snapshot) = catalog.saved()?;
            let mut machine = ProtocolMachine::observe(&catalog.state, 1)?;
            let previous = history.operation(operation.id());
            let remembered = catalog.state.seen(operation)?;
            if let Some(previous) = previous
                && &previous.operation != operation
            {
                return Err(repair(format!(
                    "operation ID reused with different intent: {}",
                    operation.id()
                ))
                .into());
            }
            if previous.is_some() || remembered {
                return Ok(SyncResult::new(sharing, LocalStatus::Unchanged)
                    .with_domain(Reconciliation::no_change(
                        operation.id(),
                        "operation already saved",
                    ))
                    .with_local_rev(history.tip().map(str::to_owned)));
            }
            let authorities = catalog.authorities(&history)?;
            let auth = operation.authority_for(&authorities);
            let result = reconcile(&snapshot.catalog, operation, auth);
            let Reconciliation::Changes { record, .. } = &result else {
                if result.outcome() == DomainOutcome::NoChange {
                    catalog.state.remember(operation)?;
                }
                return Ok(SyncResult::new(sharing, LocalStatus::Unchanged)
                    .with_domain(result)
                    .with_local_rev(history.tip().map(str::to_owned)));
            };
            let message = encode_operation(operation, Some(&origin))?;
            let permit =
                machine.begin_save(&catalog.state, operation, record, &snapshot, history.tip())?;
            let commit = permit.run(&message, |permit| -> Result<String, CatalogError> {
                catalog.tx.save(permit.record(), permit.expected())?;
                let path = catalog.store.record_path(permit.record());
                let path = path.to_string_lossy();
                catalog.git.run(&["add", "--", &path], None, None, &[0])?;
                let tree = catalog.git.text(&["write-tree"], None, None, &[0])?;
                let commit = catalog.git.commit(&tree, &message, permit.start_tip())?;
                let branch = format!("refs/heads/{}", catalog.config()?.branch());
                let start = permit
                    .start_tip()
                    .ok_or_else(|| repair("managed catalog has no saved tip"))?;
                catalog
                    .git
                    .run(&["update-ref", &branch, &commit, start], None, None, &[0])?;
                catalog.clean()?;
                let verified = read_history(catalog.git, Some(&commit))?;
                if &catalog.tx.read()?.catalog != verified.catalog() {
                    return Err(repair("saved records disagree with committed history").into());
                }
                Ok(commit)
            })?;
            Ok(SyncResult::new(sharing, LocalStatus::Committed)
                .with_domain(result)
                .with_local_rev(Some(commit)))
        });
        Ok(match outcome {
            Ok(result) => result,
            Err(error) => SyncResult::new(
                if sharing == SharingStatus::LocalOnly {
                    sharing
                } else {
                    SharingStatus::Blocked
                },
                LocalStatus::RepairRequired,
            )
            .with_detail(error.to_string()),
        })
    }

    /// Fetch when due and project intent without pushing, adopting or granting authority.
    pub fn read(&self, refresh: bool) -> Result<ReadResult, RepairRequired> {
        self.with_lock(|catalog| {
            let config = catalog.ready()?;
            let (local, snapshot) = catalog.saved()?;
            if config.remote().is_none() {
                return Ok(ReadResult {
                    catalog: snapshot.catalog.clone(),
                    local: snapshot,
                    confirmed: None,
                    freshness: Freshness::LocalOnly,
                    fetched_at: None,
                    conflicts: Vec::new(),
                    detail: String::new(),
                });
            }
            let metadata = catalog.state.metadata()?;
            let mut freshness = Freshness::Cached;
            if catalog.policy.read_due(
                metadata.fetched_at,
                catalog.now()?,
                metadata.refresh_retry_after,
                refresh,
            ) {
                match catalog.fetch() {
                    Ok(_) => freshness = Freshness::Refreshed,
                    Err(CatalogError::Git(error)) => {
                        let mut metadata = catalog.state.metadata()?;
                        metadata.refresh_retry_after = Seconds::try_from(
                            catalog.now()?.get() + catalog.policy.cooldown_seconds(),
                        )
                        .map_err(repair)?;
                        metadata.refresh_error = error.to_string();
                        catalog.state.write_metadata(&metadata)?;
                        freshness = Freshness::Failed;
                    }
                    Err(error) => return Err(error),
                }
            }
            let metadata = catalog.state.metadata()?;
            let remote = read_history(catalog.git, metadata.confirmed.as_ref().map(Oid::as_str))?;
            let (projected, conflicts) = match replay(&local, &remote)? {
                Replay::Ready { catalog, .. } => (catalog, Vec::new()),
                Replay::Blocked(conflict) => (snapshot.catalog.clone(), vec![conflict]),
            };
            let confirmed = if metadata.fetched_at.is_some() {
                Some(YamlStore::decode_snapshot(remote.files())?)
            } else {
                None
            };
            Ok(ReadResult {
                catalog: projected,
                local: snapshot,
                confirmed,
                freshness,
                fetched_at: metadata.fetched_at.map(Seconds::get),
                conflicts,
                detail: metadata.refresh_error,
            })
        })
        .map_err(repair)
    }

    /// Force fetch-only inspection without advancing publication clocks.
    pub fn inspect_remote(&self) -> Result<Snapshot, RepairRequired> {
        self.with_lock(|catalog| {
            if catalog.ready()?.remote().is_none() {
                return Err(repair("local Git catalog has no remote to inspect").into());
            }
            let tip = catalog.fetch()?;
            Ok(YamlStore::decode_snapshot(
                read_history(catalog.git, tip.as_deref())?.files(),
            )?)
        })
        .map_err(repair)
    }

    /// Read the last validated remote snapshot without fetching or projecting local intent.
    pub fn confirmed_snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.with_lock(|catalog| {
            let tip = catalog.state.metadata()?.confirmed;
            Ok(YamlStore::decode_snapshot(
                read_history(catalog.git, tip.as_ref().map(Oid::as_str))?.files(),
            )?)
        })
        .map_err(repair)
    }

    /// Synchronize the entire pending batch; reads and local saves never call this implicitly.
    pub fn sync(&self, event: SyncEvent) -> SyncResult {
        let mut sharing = SharingStatus::Blocked;
        let mut had_publication = false;
        let outcome = self.with_lock(|catalog| {
            let outcome = (|| {
                sharing = if catalog
                    .state
                    .config()?
                    .is_some_and(|c| c.remote().is_none())
                {
                    SharingStatus::LocalOnly
                } else {
                    SharingStatus::Pending
                };
                catalog.ready()?;
                let (local, _) = catalog.saved()?;
                if sharing == SharingStatus::LocalOnly {
                    return Ok(SyncResult::new(sharing, LocalStatus::Unchanged)
                        .with_local_rev(local.tip().map(str::to_owned)));
                }
                let metadata = catalog.state.metadata()?;
                let known =
                    read_history(catalog.git, metadata.confirmed.as_ref().map(Oid::as_str))?;
                let pending_stream = local.operations().iter().any(|entry| {
                    matches!(entry.operation.after(), crate::model::Record::Stream(_))
                        && !known.has_operation(entry.operation.id())
                });
                let timing =
                    Timing::new(metadata.last_sync, metadata.last_push, metadata.retry_after);
                if !catalog
                    .policy
                    .due(event, timing, catalog.now()?, pending_stream)
                {
                    return Ok(
                        SyncResult::new(SharingStatus::Pending, LocalStatus::Unchanged)
                            .with_local_rev(local.tip().map(str::to_owned))
                            .with_remote_rev(known.tip().map(str::to_owned)),
                    );
                }
                let mut machine =
                    ProtocolMachine::observe(&catalog.state, catalog.policy.attempts())?;
                let mut remote = read_history(catalog.git, catalog.fetch()?.as_deref())?;
                let mut retained = machine.inspect_retained(catalog, local.tip(), &remote)?;
                had_publication |= matches!(retained, Some(RetainedAttempt::Confirmed(_)));
                loop {
                    let candidate = if let Some(candidate) = retained
                        .as_ref()
                        .and_then(|retained| retained.reusable_candidate(&local, &remote))
                    {
                        candidate.to_owned()
                    } else {
                        match replay(&local, &remote)? {
                            Replay::Ready { changes, .. } => {
                                catalog.candidate(&local, &remote, changes)?
                            }
                            Replay::Blocked(conflict) => {
                                return Ok(SyncResult::new(
                                    SharingStatus::Blocked,
                                    LocalStatus::Unchanged,
                                )
                                .with_local_rev(local.tip().map(str::to_owned))
                                .with_remote_rev(remote.tip().map(str::to_owned))
                                .with_conflicts(vec![conflict])
                                .with_detail("domain conflict blocks the outgoing batch"));
                            }
                        }
                    };
                    if Some(candidate.as_str()) == remote.tip() {
                        let permit = machine.begin_remote_adoption(
                            catalog,
                            &local,
                            &remote,
                            match retained.take() {
                                Some(RetainedAttempt::Confirmed(receipt)) => Some(receipt),
                                _ => None,
                            },
                        )?;
                        return match permit.apply() {
                            Ok(revision) => {
                                catalog.success()?;
                                Ok(SyncResult::new(
                                    SharingStatus::Confirmed,
                                    LocalStatus::Unchanged,
                                )
                                .with_local_rev(Some(revision))
                                .with_remote_rev(remote.tip().map(str::to_owned)))
                            }
                            Err(error) => Ok(SyncResult::new(
                                if had_publication {
                                    SharingStatus::Confirmed
                                } else {
                                    SharingStatus::Blocked
                                },
                                LocalStatus::RepairRequired,
                            )
                            .with_local_rev(local.tip().map(str::to_owned))
                            .with_remote_rev(remote.tip().map(str::to_owned))
                            .with_detail(error.to_string())),
                        };
                    }
                    // A new batch has its own publication receipt; prior confirmation
                    // still affects sharing, but cannot authorize this attempt's adoption.
                    let Some(mut permit) = machine.prepare_push(
                        catalog,
                        &local,
                        &remote,
                        &candidate,
                        retained.take(),
                    )?
                    else {
                        catalog.failure()?;
                        return Ok(
                            SyncResult::new(SharingStatus::Failed, LocalStatus::Unchanged)
                                .with_local_rev(local.tip().map(str::to_owned))
                                .with_remote_rev(remote.tip().map(str::to_owned))
                                .with_detail("competing-push retry budget exhausted"),
                        );
                    };
                    match permit.push() {
                        Ok(()) => {
                            had_publication = true;
                            let adoption = permit.acknowledge()?.record_confirmation()?;
                            return match adoption.apply() {
                                Ok(revision) => {
                                    catalog.success()?;
                                    Ok(SyncResult::new(
                                        SharingStatus::Confirmed,
                                        LocalStatus::Unchanged,
                                    )
                                    .with_local_rev(Some(revision))
                                    .with_remote_rev(Some(candidate)))
                                }
                                Err(error) => Ok(SyncResult::new(
                                    SharingStatus::Confirmed,
                                    LocalStatus::RepairRequired,
                                )
                                .with_local_rev(local.tip().map(str::to_owned))
                                .with_remote_rev(Some(candidate))
                                .with_detail(error.to_string())),
                            };
                        }
                        Err(CatalogError::Git(push_error)) => {
                            let observed = match catalog.fetch() {
                                Ok(tip) => read_history(catalog.git, tip.as_deref())?,
                                Err(CatalogError::Git(_)) => {
                                    permit.leave()?;
                                    catalog.failure()?;
                                    return Ok(SyncResult::new(
                                        SharingStatus::Uncertain,
                                        LocalStatus::Unchanged,
                                    )
                                    .with_local_rev(local.tip().map(str::to_owned))
                                    .with_detail(push_error.to_string()));
                                }
                                Err(error) => {
                                    permit.leave()?;
                                    return Err(error);
                                }
                            };
                            match permit.recover(&observed)? {
                                Recovery::Confirmed(witness) => {
                                    had_publication = true;
                                    retained = Some(RetainedAttempt::Confirmed(witness.record()?));
                                    remote = observed;
                                    // Confirmation returns Ready, not Adopting: replay again
                                    // against the fetched tip before any checkout mutation.
                                }
                                Recovery::Unconfirmed(permit) => {
                                    if observed.tip() != remote.tip() && permit.has_push_budget() {
                                        retained = permit.retry(&observed)?;
                                        had_publication |=
                                            matches!(retained, Some(RetainedAttempt::Confirmed(_)));
                                        remote = observed;
                                    } else {
                                        permit.leave()?;
                                        catalog.failure()?;
                                        return Ok(SyncResult::new(
                                            SharingStatus::Failed,
                                            LocalStatus::Unchanged,
                                        )
                                        .with_local_rev(local.tip().map(str::to_owned))
                                        .with_remote_rev(observed.tip().map(str::to_owned))
                                        .with_detail(push_error.to_string()));
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            permit.leave()?;
                            return Err(error);
                        }
                    }
                }
            })();
            if let Err(ref error) = outcome
                && sharing != SharingStatus::LocalOnly
                && !matches!(error, CatalogError::Repair(_))
            {
                let _ = catalog.failure();
            }
            outcome
        });
        match outcome {
            Ok(result) => result,
            Err(error) => {
                let (sharing, local) = if sharing == SharingStatus::LocalOnly {
                    (sharing, LocalStatus::RepairRequired)
                } else if matches!(error, CatalogError::Repair(_)) {
                    (SharingStatus::Blocked, LocalStatus::RepairRequired)
                } else {
                    (SharingStatus::Failed, LocalStatus::Unchanged)
                };
                SyncResult::new(sharing, local).with_detail(error.to_string())
            }
        }
    }
}
