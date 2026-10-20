//! Managed Git records: locked local saves and isolated replay candidates.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use crate::model::{ValidationError, check_text};
use crate::operations::{Authority, DomainOutcome, Operation, Reconciliation, reconcile};
use crate::storage::{Snapshot, StorageError};
use crate::yaml_store::{YamlStore, YamlTransaction};

use super::envelope::{BOOTSTRAP, Origin, encode_operation};
use super::git_io::{Git, GitError, GitHook};
use super::history::{History, read_history, records};
use super::local::{authority_for, check_producer};
use super::outcomes::{LocalStatus, Result as SyncResult, SharingStatus};
use super::policy::{Policy, SyncEvent, Timing};
use super::protocol::{ProtocolMachine, publication::Recovery};
use super::replay::{ReplayConflict, replay};
use super::state::{
    GitConfig, PublicationAttempt, PublicationMarker, RepairRequired, State, StateHook,
    SyncMetadata,
};
use super::{Freshness, ReadResult};

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

fn repair(error: impl std::fmt::Display) -> RepairRequired {
    RepairRequired::new(error.to_string())
}

/// Managed operation commits with separate remote publication and local recovery fences.
pub struct GitCatalog {
    pub(super) root: PathBuf,
    pub(super) store: YamlStore,
    pub(super) state: State,
    pub(super) git: Git,
    pub(super) host_id: String,
    pub(super) policy: Policy,
    pub(super) clock: Arc<dyn Fn() -> f64 + Send + Sync>,
}

impl std::fmt::Debug for GitCatalog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitCatalog")
            .field("root", &self.root)
            .field("store", &self.store)
            .field("state", &self.state)
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
        Policy::new(
            policy.stream_seconds,
            policy.session_seconds,
            policy.cooldown_seconds,
            policy.timeout_seconds,
            policy.attempts,
        )
        .map_err(ValidationError::new)?;
        check_text(&host_id, "host_id", false)?;
        let root = std::path::absolute(root)
            .map_err(|e| ValidationError::new(format!("cannot resolve catalog root: {e}")))?;
        Ok(Self {
            store: YamlStore::new(&root),
            state: State::new(&root),
            git: Git::new(&root, policy.timeout_seconds),
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

    pub fn state(&self) -> &State {
        &self.state
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
        self.state = self.state.clone().with_hook(hook);
    }

    pub fn set_clock(&mut self, clock: Arc<dyn Fn() -> f64 + Send + Sync>) {
        self.clock = clock;
    }

    pub(super) fn with_lock<T>(
        &self,
        f: impl FnOnce(&mut YamlTransaction) -> Result<T, CatalogError>,
    ) -> Result<T, CatalogError> {
        self.store.with_transaction(|tx| Ok(f(tx)))?
    }

    /// Initialize an empty directory from a remote or a new bootstrap; failed setup stays fenced.
    pub fn initialize(self, remote: Option<String>, branch: &str) -> Result<Self, RepairRequired> {
        let store = self.store.clone();
        store
            .with_transaction(|_tx| {
                Ok((|| -> Result<(), CatalogError> {
                    if let Some(ref remote) = remote {
                        check_text(remote, "remote", false)?;
                    }
                    check_text(branch, "branch", false)?;
                    if let Some(config) = self.state.config()? {
                        if config.remote != remote
                            || config.host != self.host_id
                            || config.branch != branch
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
                    for entry in fs::read_dir(&self.root)? {
                        let name = entry?.file_name();
                        if name != ".catalog.lock" && name != ".catalog-sync" {
                            return Err(repair(
                                "initialization requires a new empty catalog directory",
                            )
                            .into());
                        }
                    }
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
                    let config =
                        GitConfig::new(remote.clone(), branch, &self.host_id).map_err(repair)?;
                    self.state.write_config(&config)?;
                    let remote_tip = if remote.is_some() {
                        self.fetch()?
                    } else {
                        None
                    };
                    let tip = if let Some(tip) = remote_tip {
                        read_history(&self.git, Some(&tip))?;
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
                    read_history(&self.git, Some(&tip))?;
                    self.state.clear("save.json")?;
                    Ok(())
                })())
            })
            .map_err(repair)?
            .map_err(repair)?;
        Ok(self)
    }

    pub(super) fn config(&self) -> Result<GitConfig, RepairRequired> {
        let config = self
            .state
            .config()?
            .filter(|c| c.host == self.host_id)
            .ok_or_else(|| repair("catalog is not initialized for this host"))?;
        let head = self
            .git
            .text(&["symbolic-ref", "--quiet", "HEAD"], None, None, &[0])
            .map_err(repair)?;
        if head != format!("refs/heads/{}", config.branch) {
            return Err(repair("managed catalog branch was changed"));
        }
        match &config.remote {
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
                if fetch != *remote || push != *remote || (!mirror.is_empty() && mirror != "false")
                {
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
        if config.remote.is_none() && (metadata != SyncMetadata::default() || publication.is_some())
        {
            return Err(repair(
                "local Git catalog has remote synchronization state; inspect before continuing",
            )
            .into());
        }
        if let Some(ref publication) = publication {
            let attempt = publication.attempt();
            if attempt.branch != config.branch || Some(&attempt.remote) != config.remote.as_ref() {
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
        for name in [".catalog.lock", ".catalog-staging", ".catalog-sync"] {
            if self.root.join(name).is_symlink() {
                return Err(repair(format!("symlinked catalog machinery: {name}")).into());
            }
        }
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
        let history = read_history(&self.git, Some(&self.git.head()?))?;
        self.state
            .validate_fingerprints(&history.fingerprints, true)?;
        Ok(history)
    }

    pub(super) fn saved(&self, tx: &YamlTransaction) -> Result<(History, Snapshot), CatalogError> {
        let history = self.history()?;
        let snapshot = tx.read()?;
        if records(&snapshot.catalog) != records(&history.catalog) {
            return Err(repair("working records disagree with supported history").into());
        }
        Ok((history, snapshot))
    }

    /// Read durable local records only after validating managed history and recovery fences.
    pub fn snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.with_lock(|tx| {
            self.ready()?;
            Ok(self.saved(tx)?.1)
        })
        .map_err(repair)
    }

    pub(super) fn authorities(
        &self,
        local: &History,
    ) -> Result<BTreeMap<String, Authority>, CatalogError> {
        let remote = read_history(&self.git, self.state.metadata()?.confirmed.as_deref())?;
        let mut authorities = local.authorities.clone();
        authorities.extend(remote.authorities);
        Ok(authorities)
    }

    /// Combine confirmed ownership with provenance for locally created streams.
    pub fn authority(&self, stream: &str) -> Result<Option<Authority>, RepairRequired> {
        self.with_lock(|_tx| {
            self.ready()?;
            Ok(self.authorities(&self.history()?)?.get(stream).cloned())
        })
        .map_err(repair)
    }

    /// Commit one accepted record and its original intent before issuing its retry receipt.
    pub fn save(
        &self,
        operation: &Operation,
        origin: Option<&Origin>,
    ) -> Result<SyncResult, ValidationError> {
        check_producer(operation, &self.host_id)?;
        let mut origin = origin.cloned().unwrap_or_default();
        origin
            .entry("host".into())
            .or_insert_with(|| self.host_id.clone());
        encode_operation(operation, Some(&origin))?;
        let mut sharing = SharingStatus::Blocked;
        let outcome = self.with_lock(|tx| {
            let config = self.state.config()?;
            sharing = if config.is_some_and(|c| c.remote.is_none()) {
                SharingStatus::LocalOnly
            } else {
                SharingStatus::Pending
            };
            self.ready()?;
            let (history, snapshot) = self.saved(tx)?;
            let mut machine = ProtocolMachine::observe(&self.state, 1)?;
            let previous = history.operation(&operation.id);
            let remembered = self.state.seen(operation)?;
            if let Some(previous) = previous
                && &previous.operation != operation
            {
                return Err(repair(format!(
                    "operation ID reused with different intent: {}",
                    operation.id
                ))
                .into());
            }
            if previous.is_some() || remembered {
                return Ok(SyncResult::new(sharing, LocalStatus::Unchanged)
                    .with_domain(Reconciliation::no_change(
                        &operation.id,
                        "operation already saved",
                    ))
                    .with_local_rev(history.tip));
            }
            let auth = authority_for(operation, &self.authorities(&history)?);
            let result = reconcile(&snapshot.catalog, operation, auth.as_ref());
            if result.outcome != DomainOutcome::Changes {
                if result.outcome == DomainOutcome::NoChange {
                    self.state.remember(operation)?;
                }
                return Ok(SyncResult::new(sharing, LocalStatus::Unchanged)
                    .with_domain(result)
                    .with_local_rev(history.tip));
            }
            let permit = machine.begin_save(
                operation,
                result.record.as_ref().unwrap(),
                &snapshot,
                history.tip.as_deref(),
            )?;
            let effects = (|| -> Result<String, CatalogError> {
                let message = encode_operation(permit.operation(), Some(&origin))?;
                self.state.write_record(
                    "save.json",
                    &json!({
                        "start": permit.start_tip(), "operation": message,
                    }),
                )?;
                tx.save(permit.record(), permit.expected())?;
                let path = self.store.record_path(permit.record());
                let path = path.to_string_lossy();
                self.git.run(&["add", "--", &path], None, None, &[0])?;
                let tree = self.git.text(&["write-tree"], None, None, &[0])?;
                let commit = self.git.commit(&tree, &message, permit.start_tip())?;
                let branch = format!("refs/heads/{}", self.config()?.branch);
                let start = permit
                    .start_tip()
                    .ok_or_else(|| repair("managed catalog has no saved tip"))?;
                self.git
                    .run(&["update-ref", &branch, &commit, start], None, None, &[0])?;
                self.clean()?;
                let verified = read_history(&self.git, Some(&commit))?;
                if records(&tx.read()?.catalog) != records(&verified.catalog) {
                    return Err(repair("saved records disagree with committed history").into());
                }
                self.state.remember(permit.operation())?;
                self.state.clear("save.json")?;
                Ok(commit)
            })();
            match effects {
                Ok(commit) => {
                    permit.complete()?;
                    Ok(SyncResult::new(sharing, LocalStatus::Committed)
                        .with_domain(result)
                        .with_local_rev(Some(commit)))
                }
                Err(error) => {
                    let _ = permit.fail();
                    Err(error)
                }
            }
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

    pub(super) fn fetch(&self) -> Result<Option<String>, CatalogError> {
        let config = self.config()?;
        if config.remote.is_none() {
            return Err(repair("local Git catalog has no remote to fetch").into());
        }
        let branch = format!("refs/heads/{}", config.branch);
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
        let history = read_history(&self.git, tip.as_deref())?;
        let mut metadata = self.state.metadata()?;
        if let Some(previous) = &metadata.confirmed
            && (tip.is_none() || !self.git.ancestor(previous, tip.as_deref().unwrap())?)
        {
            return Err(repair("confirmed remote history was removed or replaced").into());
        }
        if tip.is_some() {
            self.state
                .validate_fingerprints(&history.fingerprints, true)?;
        }
        metadata.confirmed = tip.clone();
        metadata.fetched_at = Some((self.clock)());
        metadata.refresh_retry_after = 0.0;
        metadata.refresh_error.clear();
        self.state.write_metadata(&metadata)?;
        Ok(tip)
    }

    /// Fetch when due and project intent without pushing, adopting or granting authority.
    pub fn read(&self, refresh: bool) -> Result<ReadResult, RepairRequired> {
        self.with_lock(|tx| {
            let config = self.ready()?;
            let (local, snapshot) = self.saved(tx)?;
            if config.remote.is_none() {
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
            let metadata = self.state.metadata()?;
            let mut freshness = Freshness::Cached;
            if self
                .policy
                .read_due(
                    metadata.fetched_at,
                    (self.clock)(),
                    metadata.refresh_retry_after,
                    refresh,
                )
                .map_err(|e| CatalogError::Validation(ValidationError::new(e)))?
            {
                match self.fetch() {
                    Ok(_) => freshness = Freshness::Refreshed,
                    Err(CatalogError::Git(error)) => {
                        let mut metadata = self.state.metadata()?;
                        metadata.refresh_retry_after =
                            (self.clock)() + self.policy.cooldown_seconds;
                        metadata.refresh_error = error.to_string();
                        self.state.write_metadata(&metadata)?;
                        freshness = Freshness::Failed;
                    }
                    Err(error) => return Err(error),
                }
            }
            let metadata = self.state.metadata()?;
            let remote = read_history(&self.git, metadata.confirmed.as_deref())?;
            let projected = replay(&local, &remote)?;
            let confirmed = if metadata.fetched_at.is_some() {
                Some(YamlStore::decode_snapshot(&remote.files)?)
            } else {
                None
            };
            Ok(ReadResult {
                catalog: if projected.conflicts.is_empty() {
                    projected.catalog
                } else {
                    snapshot.catalog.clone()
                },
                local: snapshot,
                confirmed,
                freshness,
                fetched_at: metadata.fetched_at,
                conflicts: projected.conflicts,
                detail: metadata.refresh_error,
            })
        })
        .map_err(repair)
    }

    /// Force fetch-only inspection without advancing publication clocks.
    pub fn inspect_remote(&self) -> Result<Snapshot, RepairRequired> {
        self.with_lock(|_tx| {
            if self.ready()?.remote.is_none() {
                return Err(repair("local Git catalog has no remote to inspect").into());
            }
            let tip = self.fetch()?;
            Ok(YamlStore::decode_snapshot(
                &read_history(&self.git, tip.as_deref())?.files,
            )?)
        })
        .map_err(repair)
    }

    /// Read the last validated remote snapshot without fetching or projecting local intent.
    pub fn confirmed_snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.with_lock(|_tx| {
            let tip = self.state.metadata()?.confirmed;
            Ok(YamlStore::decode_snapshot(
                &read_history(&self.git, tip.as_deref())?.files,
            )?)
        })
        .map_err(repair)
    }

    pub(super) fn candidate(
        &self,
        local: &History,
        remote: &History,
    ) -> Result<(String, Vec<ReplayConflict>), CatalogError> {
        let mut base = remote
            .tip
            .as_ref()
            .or(local.root.as_ref())
            .cloned()
            .ok_or_else(|| repair("candidate requires a supported history root"))?;
        let projected = replay(local, remote)?;
        if !projected.conflicts.is_empty() {
            return Ok((base, projected.conflicts));
        }
        let directory = tempfile::Builder::new()
            .prefix("streams-candidate-")
            .tempdir()?;
        let root = directory.path().join("records");
        fs::create_dir(&root)?;
        for (name, contents) in &remote.files {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap())?;
            fs::write(path, contents)?;
        }
        let store = YamlStore::new(&root);
        let index = directory.path().join("index");
        self.git
            .run(&["read-tree", &base], None, Some(&index), &[0])?;
        store.with_transaction(|tx| {
            Ok((|| -> Result<(), CatalogError> {
                for change in projected.changes {
                    let (operation, origin, record) =
                        (change.operation, change.origin, change.record);
                    tx.save(&record, &tx.read()?)?;
                    let path = store.record_path(&record);
                    let bytes = fs::read(root.join(&path))?;
                    let blob = self.git.text(
                        &["hash-object", "-w", "--stdin"],
                        Some(&bytes),
                        None,
                        &[0],
                    )?;
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
            })())
        })??;
        read_history(&self.git, Some(&base))?;
        Ok((base, Vec::new()))
    }

    pub(super) fn retain(&self, revision: &str) -> Result<(), CatalogError> {
        let retained = format!("refs/catalog/retained/{}", uuid::Uuid::new_v4());
        self.git
            .run(&["update-ref", &retained, revision], None, None, &[0])?;
        Ok(())
    }

    pub(super) fn adopt(
        &self,
        old: &str,
        new: &str,
        marker: &PublicationAttempt,
    ) -> Result<(), CatalogError> {
        self.state.write_publication(&PublicationMarker::Adopting(
            marker.adopting(old, new).map_err(repair)?,
        ))?;
        if old != new {
            self.git
                .run(&["read-tree", "-m", "-u", old, new], None, None, &[0])?;
            let branch = format!("refs/heads/{}", self.config()?.branch);
            self.git
                .run(&["update-ref", &branch, new, old], None, None, &[0])?;
        }
        self.clean()?;
        if self.git.head()? != new {
            return Err(repair("adopted branch does not match the intended history").into());
        }
        let verified = read_history(&self.git, Some(new))?;
        for (path, contents) in verified.files {
            if fs::read(self.root.join(path))? != contents {
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
        if let Some(timestamp) = marker.confirmed_at {
            if metadata.last_confirmed_attempt.as_deref() != Some(&marker.candidate)
                || metadata.last_push != Some(timestamp)
            {
                return Err(repair(
                    "publication confirmation disagrees with its scheduling receipt",
                )
                .into());
            }
            return Ok(marker.clone());
        }
        if metadata.last_confirmed_attempt.as_deref() != Some(&marker.candidate) {
            metadata.last_push = Some((self.clock)());
            metadata.last_confirmed_attempt = Some(marker.candidate.clone());
            self.state.write_metadata(&metadata)?;
        }
        let mut confirmed = marker.clone();
        confirmed.confirmed_at = metadata.last_push;
        self.state
            .write_publication(&PublicationMarker::Attempting(confirmed.clone()))?;
        Ok(confirmed)
    }

    /// Synchronize the entire pending batch; reads and local saves never call this implicitly.
    pub fn sync(&self, event: SyncEvent) -> SyncResult {
        let mut sharing = SharingStatus::Blocked;
        let mut had_publication = false;
        let outcome = self.with_lock(|tx| {
            sharing = if self.state.config()?.is_some_and(|c| c.remote.is_none()) {
                SharingStatus::LocalOnly
            } else {
                SharingStatus::Pending
            };
            self.ready()?;
            let (local, _) = self.saved(tx)?;
            if sharing == SharingStatus::LocalOnly {
                return Ok(
                    SyncResult::new(sharing, LocalStatus::Unchanged).with_local_rev(local.tip)
                );
            }
            let metadata = self.state.metadata()?;
            let known = read_history(&self.git, metadata.confirmed.as_deref())?;
            let pending_stream = local.operations.iter().any(|entry| {
                matches!(entry.operation.after, crate::model::Record::Stream(_))
                    && !known.has_operation(&entry.operation.id)
            });
            let timing = Timing::new(metadata.last_sync, metadata.last_push, metadata.retry_after)
                .map_err(repair)?;
            if !self
                .policy
                .due(event, timing, (self.clock)(), pending_stream)
                .map_err(repair)?
            {
                return Ok(
                    SyncResult::new(SharingStatus::Pending, LocalStatus::Unchanged)
                        .with_local_rev(local.tip)
                        .with_remote_rev(known.tip),
                );
            }
            let mut machine = ProtocolMachine::observe(&self.state, self.policy.attempts)?;
            let mut remote = read_history(&self.git, self.fetch()?.as_deref())?;
            let mut pushes = 0;
            let mut recovered = None;
            loop {
                let mut reuse = None;
                if recovered.is_none()
                    && let Some(marker) = self.state.publication()?
                {
                    let attempt = marker.attempt();
                    if let Some(witness) =
                        machine.recover_before_push(self, attempt, &local, &remote)?
                    {
                        had_publication = true;
                        recovered = Some(witness.record()?);
                    } else {
                        reuse = machine.reusable_candidate(self, attempt, &local, &remote)?;
                    }
                }
                let (candidate, conflicts) = if reuse.is_some() {
                    (
                        self.state
                            .publication()?
                            .unwrap()
                            .attempt()
                            .candidate
                            .clone(),
                        Vec::new(),
                    )
                } else {
                    self.candidate(&local, &remote)?
                };
                if !conflicts.is_empty() {
                    return Ok(
                        SyncResult::new(SharingStatus::Blocked, LocalStatus::Unchanged)
                            .with_local_rev(local.tip)
                            .with_remote_rev(remote.tip)
                            .with_conflicts(conflicts)
                            .with_detail("domain conflict blocks the outgoing batch"),
                    );
                }
                if Some(&candidate) == remote.tip.as_ref() {
                    let permit =
                        machine.begin_remote_adoption(self, &local, &remote, recovered.take())?;
                    return match permit.apply() {
                        Ok(revision) => {
                            self.success()?;
                            Ok(
                                SyncResult::new(SharingStatus::Confirmed, LocalStatus::Unchanged)
                                    .with_local_rev(Some(revision))
                                    .with_remote_rev(remote.tip),
                            )
                        }
                        Err(error) => Ok(SyncResult::new(
                            if had_publication {
                                SharingStatus::Confirmed
                            } else {
                                SharingStatus::Blocked
                            },
                            LocalStatus::RepairRequired,
                        )
                        .with_local_rev(local.tip)
                        .with_remote_rev(remote.tip)
                        .with_detail(error.to_string())),
                    };
                }
                if pushes >= self.policy.attempts {
                    self.failure()?;
                    return Ok(
                        SyncResult::new(SharingStatus::Failed, LocalStatus::Unchanged)
                            .with_local_rev(local.tip)
                            .with_remote_rev(remote.tip)
                            .with_detail("competing-push retry budget exhausted"),
                    );
                }
                // A new batch has its own publication receipt; prior confirmation
                // still affects sharing, but cannot authorize this attempt's adoption.
                recovered = None;
                let mut permit = machine.prepare_push(self, &local, &remote, &candidate, reuse)?;
                pushes += 1;
                match permit.push() {
                    Ok(()) => {
                        had_publication = true;
                        let adoption = permit.acknowledge()?.record_confirmation()?;
                        return match adoption.apply() {
                            Ok(revision) => {
                                self.success()?;
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
                            .with_local_rev(local.tip)
                            .with_remote_rev(Some(candidate))
                            .with_detail(error.to_string())),
                        };
                    }
                    Err(CatalogError::Git(push_error)) => {
                        let observed = match self.fetch() {
                            Ok(tip) => read_history(&self.git, tip.as_deref())?,
                            Err(CatalogError::Git(_)) => {
                                permit.leave()?;
                                self.failure()?;
                                return Ok(SyncResult::new(
                                    SharingStatus::Uncertain,
                                    LocalStatus::Unchanged,
                                )
                                .with_local_rev(local.tip)
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
                                recovered = Some(witness.record()?);
                                remote = observed;
                                // Confirmation returns Ready, not Adopting: replay again
                                // against the fetched tip before any checkout mutation.
                            }
                            Recovery::Unconfirmed(permit) => {
                                if observed.tip != remote.tip && pushes < self.policy.attempts {
                                    permit.retry(&observed)?;
                                    remote = observed;
                                } else {
                                    permit.leave()?;
                                    self.failure()?;
                                    return Ok(SyncResult::new(
                                        SharingStatus::Failed,
                                        LocalStatus::Unchanged,
                                    )
                                    .with_local_rev(local.tip)
                                    .with_remote_rev(observed.tip)
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
        });
        match outcome {
            Ok(result) => result,
            Err(error) => {
                let (sharing, local) = if sharing == SharingStatus::LocalOnly {
                    (sharing, LocalStatus::RepairRequired)
                } else if matches!(error, CatalogError::Repair(_)) {
                    (SharingStatus::Blocked, LocalStatus::RepairRequired)
                } else {
                    let _ = self.failure();
                    (SharingStatus::Failed, LocalStatus::Unchanged)
                };
                SyncResult::new(sharing, local).with_detail(error.to_string())
            }
        }
    }

    pub(super) fn success(&self) -> Result<(), CatalogError> {
        let mut metadata = self.state.metadata()?;
        metadata.last_sync = Some((self.clock)());
        metadata.retry_after = 0.0;
        self.state.write_metadata(&metadata)?;
        Ok(())
    }

    pub(super) fn failure(&self) -> Result<(), CatalogError> {
        let mut metadata = self.state.metadata()?;
        metadata.retry_after = (self.clock)() + self.policy.cooldown_seconds;
        self.state.write_metadata(&metadata)?;
        Ok(())
    }
}
