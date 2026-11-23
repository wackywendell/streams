//! Transport-only publication of host-partitioned records. No operation/history replay.

use super::{
    AutoSyncStatus, BackendHealth, COMMAND_TIMEOUT_SECONDS, LastSyncState, RemoteUpdate,
    SyncBackend, TransportConfig,
    git_io::{Git, GitError},
    outcomes::{LocalStatus, Result as SyncResult, SharingStatus},
};
use crate::{
    codec::RESET_HINT,
    config::Strategy,
    file_io::{
        atomic_write, decode_json, ensure_directory, existing_write_lock, read_bytes, read_json,
        sync_directory, write_json,
    },
    file_store::records::{FileRecordStore, catalog_path_host, decode_record_tree},
    model::check_text,
    storage::{StorageError, records::RecordStore},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const CONFIG: &str = ".catalog-local/transport.json";
const LAST_SYNC: &str = ".catalog-local/last-sync.json";
const TRANSPORT_LOCK: &str = ".catalog-local/transport.lock";
type Tree = BTreeMap<PathBuf, Vec<u8>>;
type OidTree = BTreeMap<PathBuf, String>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "lowercase", deny_unknown_fields)]
enum Mode {
    None,
    Git {
        remote: Option<String>,
        branch: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    version: u32,
    mode: Mode,
}

impl Selection {
    fn to_transport_config(&self) -> TransportConfig {
        match &self.mode {
            Mode::None => TransportConfig {
                strategy: Strategy::None,
                remote: None,
                branch: None,
            },
            Mode::Git { remote, branch } => TransportConfig {
                strategy: Strategy::Git,
                remote: remote.clone(),
                branch: Some(branch.clone()),
            },
        }
    }
}

/// Git-backed catalog transport; `.catalog-local/transport.json` is the sole owner of `remote`.
pub struct RecordSync {
    root: PathBuf,
    host: String,
    remote: Option<String>,
    branch: String,
    git: Git,
}

fn invalid(error: impl std::fmt::Display) -> StorageError {
    StorageError::new(error.to_string())
}

fn git_error(error: GitError) -> StorageError {
    if error.uncertain {
        StorageError::uncertain(error.message)
    } else {
        StorageError::new(error.message)
    }
}

fn check_remote(remote: &str) -> Result<(), StorageError> {
    check_text(remote, "remote", false).map_err(invalid)?;
    if remote.starts_with('-') {
        return Err(invalid("remote URL must not start with '-'"));
    }
    Ok(())
}

fn read_selection_locked(root: &Path, host: &str) -> Result<Selection, StorageError> {
    let bytes = read_bytes(&root.join(CONFIG))?
        .ok_or_else(|| invalid(format!("transport is not initialized; {RESET_HINT}")))?;
    let selection: Selection = decode_json(&bytes)?;
    if selection.version != 2 {
        return Err(invalid("unsupported record transport version"));
    }
    check_text(host, "producer", false).map_err(invalid)?;
    if let Mode::Git { remote, branch } = &selection.mode {
        check_text(branch, "branch", false).map_err(invalid)?;
        if let Some(remote) = remote {
            check_remote(remote)?;
        }
    }
    Ok(selection)
}

pub(crate) fn read_transport_config(
    root: &Path,
    host: &str,
) -> Result<TransportConfig, StorageError> {
    FileRecordStore::new(root, host)?.with_reader::<_, StorageError>(|_| Ok(()))?;
    Ok(read_selection_locked(root, host)?.to_transport_config())
}

fn ensure_own_records_locked(store: &FileRecordStore, host: &str) -> Result<(), StorageError> {
    let tree = store.read_locked_tree()?;
    let records = decode_record_tree(&tree)?;
    for record in &records {
        if crate::storage::records::RecordAddress::for_record(record).host() != host {
            return Err(invalid(
                "a fresh baseline may contain only this producer's records",
            ));
        }
    }
    Ok(())
}

fn clear_last_sync(root: &Path) -> Result<(), StorageError> {
    match fs::remove_file(root.join(LAST_SYNC)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(invalid(error)),
    }
}

/// Explicit transport setup after format initialization.
pub(crate) fn initialize(
    root: &Path,
    host: &str,
    strategy: Strategy,
    remote: Option<String>,
    branch: &str,
) -> Result<TransportConfig, StorageError> {
    let store = FileRecordStore::new(root, host)?;
    ensure_directory(&root.join(".catalog-local"), false)?;
    let _transport = crate::file_io::lock(&root.join(TRANSPORT_LOCK))?;
    let _catalog = existing_write_lock(&root.join(".catalog.lock"))?;
    store.verify_locked_format()?;
    if root.join(CONFIG).exists() {
        let existing = read_selection_locked(root, host)?.to_transport_config();
        if existing.strategy != strategy
            || existing.remote != remote
            || (strategy == Strategy::Git && existing.branch.as_deref() != Some(branch))
        {
            return Err(invalid(
                "catalog transport is already initialized differently",
            ));
        }
        return Ok(existing);
    }
    ensure_own_records_locked(&store, host)?;
    if root.join(".git").exists() {
        return Err(invalid(
            "fresh record transport cannot adopt an existing Git repository",
        ));
    }
    let git = Git::new(root, COMMAND_TIMEOUT_SECONDS);
    let mode = match strategy {
        Strategy::None => {
            if remote.is_some() {
                return Err(invalid("local-only catalog cannot have a remote"));
            }
            Mode::None
        }
        Strategy::Git => RecordSync::init_git_repo(root, &git, remote, branch)?,
    };
    let selection = Selection { version: 2, mode };
    write_json(&root.join(CONFIG), &selection)?;
    Ok(selection.to_transport_config())
}

/// Reconfigure an already-initialized catalog's sync transport.
///
/// Supports transitioning `none -> git` (preserving existing single-host records in
/// the initial commit) and updating a `git` catalog's remote. Switching `git -> none`
/// is intentionally unimplemented.
pub(crate) fn configure(
    root: &Path,
    host: &str,
    strategy: Strategy,
    remote: RemoteUpdate,
    branch: Option<&str>,
) -> Result<TransportConfig, StorageError> {
    let store = FileRecordStore::new(root, host)?;
    ensure_directory(&root.join(".catalog-local"), false)?;
    let _transport = crate::file_io::lock(&root.join(TRANSPORT_LOCK))?;
    let _catalog = existing_write_lock(&root.join(".catalog.lock"))?;
    store.verify_locked_format()?;
    let existing = read_selection_locked(root, host)?.to_transport_config();
    match (existing.strategy, strategy) {
        (Strategy::None, Strategy::None) => {
            if matches!(remote, RemoteUpdate::Set(_)) {
                return Err(invalid("local-only catalog cannot have a remote"));
            }
            if branch.is_some() {
                return Err(invalid("local-only catalog cannot set a branch"));
            }
            Ok(existing)
        }
        (Strategy::Git, Strategy::None) => {
            Err(invalid("switching from git to none is not implemented"))
        }
        (Strategy::None, Strategy::Git) => {
            let branch = branch.unwrap_or("main");
            let remote_url = match remote {
                RemoteUpdate::Set(url) => Some(url),
                RemoteUpdate::Keep | RemoteUpdate::Clear => None,
            };
            ensure_own_records_locked(&store, host)?;
            if root.join(".git").exists() {
                return Err(invalid(
                    "fresh record transport cannot adopt an existing Git repository",
                ));
            }
            let git = Git::new(root, COMMAND_TIMEOUT_SECONDS);
            let mode = RecordSync::init_git_repo(root, &git, remote_url, branch)?;
            let selection = Selection { version: 2, mode };
            write_json(&root.join(CONFIG), &selection)?;
            clear_last_sync(root)?;
            Ok(selection.to_transport_config())
        }
        (Strategy::Git, Strategy::Git) => {
            let current_branch = existing
                .branch
                .as_deref()
                .expect("Git transport has branch");
            if let Some(requested_branch) = branch
                && requested_branch != current_branch
            {
                return Err(invalid(format!(
                    "changing catalog branch from {current_branch:?} to {requested_branch:?} is not supported"
                )));
            }
            let next_remote = match remote {
                RemoteUpdate::Keep => existing.remote.clone(),
                RemoteUpdate::Clear => None,
                RemoteUpdate::Set(url) => {
                    check_remote(&url)?;
                    Some(url)
                }
            };
            let current = RecordSync::from_config(
                root,
                host,
                existing.remote.clone(),
                current_branch.to_owned(),
            );
            // Verify repository health before modifying transport configuration.
            current.local_head(&current.git)?;
            let remote_changed = next_remote != existing.remote;
            let selection = Selection {
                version: 2,
                mode: Mode::Git {
                    remote: next_remote,
                    branch: current_branch.to_owned(),
                },
            };
            write_json(&root.join(CONFIG), &selection)?;
            if remote_changed {
                clear_last_sync(root)?;
            }
            Ok(selection.to_transport_config())
        }
    }
}

impl RecordSync {
    pub(crate) fn from_config(
        root: &Path,
        host: &str,
        remote: Option<String>,
        branch: String,
    ) -> Self {
        Self {
            root: root.into(),
            host: host.into(),
            remote,
            branch,
            git: Git::new(root, COMMAND_TIMEOUT_SECONDS),
        }
    }

    /// Read a Git transport's fixed metadata. No Git command or snapshot.
    pub fn open(root: &Path, host: &str) -> Result<Self, StorageError> {
        let config = read_transport_config(root, host)?;
        match config.strategy {
            Strategy::Git => Ok(Self::from_config(
                root,
                host,
                config.remote,
                config.branch.expect("Git transport has branch"),
            )),
            Strategy::None => Err(invalid("catalog uses local-only (none) transport, not git")),
        }
    }

    fn init_git_repo(
        root: &Path,
        git: &Git,
        remote: Option<String>,
        branch: &str,
    ) -> Result<Mode, StorageError> {
        check_text(branch, "branch", false).map_err(invalid)?;
        if let Some(remote) = &remote {
            check_remote(remote)?;
        }
        git.run(
            &["check-ref-format", &format!("refs/heads/{branch}")],
            None,
            None,
            &[0],
        )
        .map_err(git_error)?;
        ensure_directory(&root.join(".catalog-staging"), true)?;
        git.run(
            &["init", "--quiet", "--initial-branch", branch],
            None,
            None,
            &[0],
        )
        .map_err(git_error)?;
        for (key, value) in [
            ("user.name", "Streams"),
            ("user.email", "streams@localhost"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
            ("core.autocrlf", "false"),
            ("core.attributesFile", "/dev/null"),
            ("core.excludesFile", "/dev/null"),
            ("core.fsync", "committed"),
        ] {
            git.run(&["config", "--local", key, value], None, None, &[0])
                .map_err(git_error)?;
        }
        git.run(&["read-tree", "--empty"], None, None, &[0])
            .map_err(git_error)?;
        if root.join("hosts").exists() {
            git.run(&["add", "--force", "-A", "--", "hosts"], None, None, &[0])
                .map_err(git_error)?;
        }
        let tree = git
            .text(&["write-tree"], None, None, &[0])
            .map_err(git_error)?;
        Self::tree(git, &tree)?;
        git.run(
            &[
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "Start record catalog v2",
            ],
            None,
            None,
            &[0],
        )
        .map_err(git_error)?;
        Ok(Mode::Git {
            remote,
            branch: branch.into(),
        })
    }

    pub fn strategy(&self) -> Strategy {
        Strategy::Git
    }
    pub fn remote(&self) -> Option<&str> {
        self.remote.as_deref()
    }
    pub fn branch(&self) -> &str {
        &self.branch
    }

    fn tree(git: &Git, revision: &str) -> Result<Tree, StorageError> {
        let tree = git
            .files(revision)?
            .into_iter()
            .map(|(path, bytes)| (PathBuf::from(path), bytes))
            .collect();
        decode_record_tree(&tree)?;
        Ok(tree)
    }
    fn working(&self) -> Result<Tree, StorageError> {
        let tree = FileRecordStore::new(&self.root, &self.host)?.read_locked_tree()?;
        decode_record_tree(&tree)?;
        Ok(tree)
    }
    fn checkpoint(
        &self,
        git: &Git,
        local: &str,
        working: &Tree,
        status: &mut LocalStatus,
    ) -> Result<String, StorageError> {
        let previous = Self::tree(git, local)?;
        for path in changed(&previous, working) {
            if catalog_path_host(&path)? != self.host {
                return Err(invalid(
                    "outgoing changes include another producer's partition",
                ));
            }
        }
        if &previous == working {
            return Ok(local.into());
        }
        let temporary =
            tempfile::tempdir_in(self.root.join(".catalog-staging")).map_err(invalid)?;
        let index = temporary.path().join("index");
        git.run(&["read-tree", local], None, Some(&index), &[0])
            .map_err(git_error)?;
        git.run(
            &["add", "--force", "-A", "--", "hosts"],
            None,
            Some(&index),
            &[0],
        )
        .map_err(git_error)?;
        let tree = git
            .text(&["write-tree"], None, Some(&index), &[0])
            .map_err(git_error)?;
        if Self::tree(git, &tree)? != *working {
            return Err(invalid("Git filters changed record bytes"));
        }
        let commit = git
            .text(
                &["commit-tree", &tree, "-p", local],
                Some(b"Save producer records\n"),
                None,
                &[0],
            )
            .map_err(git_error)?;
        *status = LocalStatus::RepairRequired;
        self.advance(git, &commit, local)?;
        *status = LocalStatus::Committed;
        Ok(commit)
    }
    fn advance(&self, git: &Git, next: &str, previous: &str) -> Result<(), StorageError> {
        git.run(
            &[
                "update-ref",
                &format!("refs/heads/{}", self.branch),
                next,
                previous,
            ],
            None,
            None,
            &[0],
        )
        .map_err(git_error)?;
        git.run(&["read-tree", next], None, None, &[0])
            .map_err(git_error)?;
        Ok(())
    }
    fn combine(
        &self,
        git: &Git,
        local: &str,
        remote: &str,
    ) -> Result<(String, Tree), StorageError> {
        let current = Self::tree(git, local)?;
        let incoming = Self::tree(git, remote)?;
        let base = git
            .text(&["merge-base", local, remote], None, None, &[0, 1])
            .map_err(git_error)?;
        if base == remote {
            return Ok((local.into(), current));
        }
        if base == local {
            return Ok((remote.into(), incoming));
        }
        let base_oid = if base.is_empty() {
            git.text(&["mktree"], Some(b""), None, &[0])
                .map_err(git_error)?
        } else {
            base
        };
        // Compare changed paths by blob OID so historical merge-base commits are never
        // decoded against current record validation rules.
        let base_oids = git.tree_oids(&base_oid)?;
        let local_oids = git.tree_oids(local)?;
        let remote_oids = git.tree_oids(remote)?;
        let local_changes = changed_oids(&base_oids, &local_oids);
        let remote_changes = changed_oids(&base_oids, &remote_oids);
        if !local_changes.is_disjoint(&remote_changes) {
            return Err(invalid(
                "concurrent changes touch the same record file; resolve explicitly, without text or domain merge",
            ));
        }
        let mut combined = current;
        for path in remote_changes {
            match incoming.get(&path) {
                Some(bytes) => {
                    combined.insert(path, bytes.clone());
                }
                None => {
                    combined.remove(&path);
                }
            }
        }
        decode_record_tree(&combined)?;
        let temporary =
            tempfile::tempdir_in(self.root.join(".catalog-staging")).map_err(invalid)?;
        let index = temporary.path().join("index");
        git.run(
            &["read-tree", "-m", "--aggressive", &base_oid, local, remote],
            None,
            Some(&index),
            &[0],
        )
        .map_err(git_error)?;
        let tree = git
            .text(&["write-tree"], None, Some(&index), &[0])
            .map_err(git_error)?;
        if Self::tree(git, &tree)? != combined {
            return Err(invalid(
                "Git tree differs from disjoint-file transport plan",
            ));
        }
        let commit = git
            .text(
                &["commit-tree", &tree, "-p", local, "-p", remote],
                Some(b"Sync host partitions\n"),
                None,
                &[0],
            )
            .map_err(git_error)?;
        Ok((commit, combined))
    }
    fn install(&self, previous: &Tree, next: &Tree) -> Result<(), StorageError> {
        for path in changed(previous, next) {
            catalog_path_host(&path)?;
            crate::file_store::records::check_parents(&self.root, &path)?;
            let target = self.root.join(&path);
            match fs::symlink_metadata(&target) {
                Ok(metadata) if !metadata.is_file() => {
                    return Err(invalid("record target must be a regular file"));
                }
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(invalid(error));
                }
                _ => {}
            }
            match next.get(&path) {
                Some(bytes) => atomic_write(&target, bytes, &self.root.join(".catalog-staging"))?,
                None => {
                    match fs::remove_file(&target) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => return Err(StorageError::uncertain(error.to_string())),
                    }
                    sync_directory(target.parent().expect("record parent"))
                        .map_err(StorageError::after_write)?;
                }
            }
        }
        Ok(())
    }
    fn local_head(&self, git: &Git) -> Result<String, StorageError> {
        for path in ["MERGE_HEAD", "rebase-merge", "rebase-apply", "index.lock"] {
            if self.root.join(".git").join(path).exists() {
                return Err(invalid(
                    "Git has unfinished external work; inspect it explicitly",
                ));
            }
        }
        ensure_directory(&self.root.join(".git"), false)?;
        if self.root.join(".git/commondir").exists() {
            return Err(invalid("record transport requires its own repository"));
        }
        git.check_target(&self.branch).map_err(invalid)?;
        let actual = git
            .text(&["rev-parse", "--show-toplevel"], None, None, &[0])
            .map_err(git_error)?;
        if fs::canonicalize(actual).map_err(invalid)?
            != fs::canonicalize(&self.root).map_err(invalid)?
        {
            return Err(invalid("Git worktree is outside catalog root"));
        }
        git.head().map_err(git_error)
    }

    pub fn check(&self) -> BackendHealth {
        let mut issues = Vec::new();
        if let Err(error) = self.local_head(&self.git) {
            issues.push(error.to_string());
        }
        let effective_remote =
            self.remote
                .as_deref()
                .and_then(|url| match self.git.effective_remote(url) {
                    Ok(effective) => Some(effective),
                    Err(error) => {
                        issues.push(format!("failed to resolve effective Git remote: {error}"));
                        None
                    }
                });
        BackendHealth {
            strategy: Strategy::Git,
            remote: self.remote.clone(),
            effective_remote,
            branch: Some(self.branch.clone()),
            issues,
        }
    }

    pub fn sync(&self) -> SyncResult {
        self.sync_with_timeout(None)
    }

    pub fn sync_with_timeout(&self, timeout_seconds: Option<f64>) -> SyncResult {
        self.sync_impl(true, timeout_seconds)
            .expect("blocking sync always produces a result")
    }

    pub fn try_sync(&self) -> Option<SyncResult> {
        self.try_sync_with_timeout(None)
    }

    pub fn try_sync_with_timeout(&self, timeout_seconds: Option<f64>) -> Option<SyncResult> {
        self.sync_impl(false, timeout_seconds)
    }

    fn sync_impl(&self, blocking: bool, timeout_seconds: Option<f64>) -> Option<SyncResult> {
        let git = match timeout_seconds {
            Some(secs) => match crate::process::deadline(secs, "catalog sync") {
                Ok(deadline) => self.git.clone().with_deadline(deadline),
                Err(error) => {
                    return Some(
                        SyncResult::new(SharingStatus::Blocked, LocalStatus::Unchanged)
                            .with_detail(error.message),
                    );
                }
            },
            None => self.git.clone(),
        };
        let lock_path = self.root.join(TRANSPORT_LOCK);
        let _transport = if blocking {
            match crate::file_io::lock(&lock_path) {
                Ok(guard) => guard,
                Err(error) => {
                    return Some(
                        SyncResult::new(SharingStatus::Blocked, LocalStatus::Unchanged)
                            .with_detail(error.to_string()),
                    );
                }
            }
        } else {
            match crate::file_io::try_lock(&lock_path) {
                Ok(Some(guard)) => guard,
                Ok(None) => return None,
                Err(error) => {
                    return Some(
                        SyncResult::new(SharingStatus::Blocked, LocalStatus::Unchanged)
                            .with_detail(error.to_string()),
                    );
                }
            }
        };
        let mut local_status = LocalStatus::Unchanged;
        let mut failure = SharingStatus::Blocked;
        let mut in_push = false;
        let result = (|| -> Result<SharingStatus, StorageError> {
            let local = {
                let _catalog = existing_write_lock(&self.root.join(".catalog.lock"))?;
                let head = self.local_head(&git)?;
                self.checkpoint(&git, &head, &self.working()?, &mut local_status)?
            };
            let Some(remote_url) = self.remote.as_deref() else {
                return Ok(SharingStatus::LocalOnly);
            };
            failure = SharingStatus::Failed;
            let reference = format!("refs/heads/{}", self.branch);
            let advertised = git
                .text(
                    &["ls-remote", "--exit-code", "--", remote_url, &reference],
                    None,
                    None,
                    &[0, 2],
                )
                .map_err(git_error)?;
            let remote = if advertised.is_empty() {
                None
            } else {
                let fields: Vec<_> = advertised.split_whitespace().collect();
                if fields.len() != 2 || fields[1] != reference {
                    return Err(invalid("remote advertised an unexpected catalog reference"));
                }
                Some(if fields[0] == local {
                    local.clone()
                } else {
                    let fetch = format!("+{reference}:refs/streams/incoming");
                    git.run(
                        &["fetch", "--quiet", "--no-tags", "--", remote_url, &fetch],
                        None,
                        None,
                        &[0],
                    )
                    .map_err(git_error)?;
                    git.text(&["rev-parse", "refs/streams/incoming"], None, None, &[0])
                        .map_err(git_error)?
                })
            };
            failure = SharingStatus::Blocked;
            let next = {
                let _catalog = existing_write_lock(&self.root.join(".catalog.lock"))?;
                if self.local_head(&git)? != local {
                    return Err(invalid(
                        "catalog Git tip changed while sync awaited the remote",
                    ));
                }
                let working = self.working()?;
                let local = self.checkpoint(&git, &local, &working, &mut local_status)?;
                match &remote {
                    Some(remote) if remote != &local => {
                        let (next, tree) = self.combine(&git, &local, remote)?;
                        if next != local {
                            local_status = LocalStatus::RepairRequired;
                            self.install(&working, &tree)?;
                            self.advance(&git, &next, &local)?;
                            local_status = LocalStatus::Committed;
                        }
                        next
                    }
                    _ => local,
                }
            };
            if remote.as_ref() == Some(&next) {
                return Ok(SharingStatus::Confirmed);
            }
            // Publish this prepared commit, not changes saved while the network is pending.
            // Clean non-zero rejections (such as non-fast-forward or auth failures) report
            // `Failed`; only an exit-less failure (timeout or signal) during push is `Uncertain`.
            failure = SharingStatus::Failed;
            in_push = true;
            git.run(
                &[
                    "push",
                    "--quiet",
                    "--",
                    remote_url,
                    &format!("{next}:{reference}"),
                ],
                None,
                None,
                &[0],
            )
            .map_err(git_error)?;
            Ok(SharingStatus::Confirmed)
        })();
        let mut outcome = match result {
            Ok(sharing) => SyncResult::new(sharing, local_status),
            Err(error) => SyncResult::new(
                if in_push && error.is_uncertain() {
                    SharingStatus::Uncertain
                } else {
                    failure
                },
                local_status,
            )
            .with_detail(error.to_string()),
        };
        if let Err(error) = record_last_sync(&self.root, &outcome, now_ms()) {
            outcome.local = LocalStatus::RepairRequired;
            let note = format!("failed to record last sync state: {error}");
            if outcome.detail.is_empty() {
                outcome.detail = note;
            } else {
                outcome.detail.push_str("; ");
                outcome.detail.push_str(&note);
            }
        }
        Some(outcome)
    }
}

impl SyncBackend for RecordSync {
    fn strategy(&self) -> Strategy {
        RecordSync::strategy(self)
    }

    fn sync_with_timeout(&self, timeout_seconds: Option<f64>) -> SyncResult {
        RecordSync::sync_with_timeout(self, timeout_seconds)
    }

    fn try_sync_with_timeout(&self, timeout_seconds: Option<f64>) -> Option<SyncResult> {
        RecordSync::try_sync_with_timeout(self, timeout_seconds)
    }

    fn check(&self) -> BackendHealth {
        RecordSync::check(self)
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn read_last_sync(root: &Path) -> Result<Option<LastSyncState>, StorageError> {
    read_json(&root.join(LAST_SYNC))
}

fn record_last_sync(root: &Path, outcome: &SyncResult, at_ms: u64) -> Result<(), StorageError> {
    let previous_succeeded = read_last_sync(root)
        .ok()
        .flatten()
        .and_then(|state| state.succeeded_at_ms);
    let needs_attention = outcome.needs_attention();
    let state = LastSyncState {
        attempted_at_ms: at_ms,
        succeeded_at_ms: if needs_attention {
            previous_succeeded
        } else {
            Some(at_ms)
        },
        local: outcome.local,
        sharing: outcome.sharing,
        needs_attention,
        detail: outcome.detail.clone(),
    };
    write_json(&root.join(LAST_SYNC), &state)
}

pub(crate) fn auto_sync_status(
    root: &Path,
    interval_minutes: u32,
    now_ms: u64,
) -> Result<AutoSyncStatus, StorageError> {
    let Some(bytes) = read_bytes(&root.join(CONFIG))? else {
        return Ok(AutoSyncStatus {
            due: false,
            last_sync: None,
        });
    };
    let selection: Selection = decode_json(&bytes)?;
    if selection.version != 2 || matches!(selection.mode, Mode::None) {
        return Ok(AutoSyncStatus {
            due: false,
            last_sync: None,
        });
    }
    let last_sync = read_last_sync(root)?;
    if interval_minutes == 0 {
        return Ok(AutoSyncStatus {
            due: false,
            last_sync,
        });
    }
    let due = match &last_sync {
        None => true,
        Some(last) => {
            let interval_ms = u64::from(interval_minutes).saturating_mul(60_000);
            now_ms.saturating_sub(last.attempted_at_ms) >= interval_ms
        }
    };
    Ok(AutoSyncStatus { due, last_sync })
}

#[cfg(test)]
pub(crate) fn is_auto_sync_due(
    root: &Path,
    interval_minutes: u32,
    now_ms: u64,
) -> Result<bool, StorageError> {
    Ok(auto_sync_status(root, interval_minutes, now_ms)?.due)
}
fn changed(before: &Tree, after: &Tree) -> BTreeSet<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

fn changed_oids(before: &OidTree, after: &OidTree) -> BTreeSet<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
