//! Transport-only publication of host-partitioned records. No operation/history replay.

use super::{
    COMMAND_TIMEOUT_SECONDS,
    git_io::{Git, GitError},
    outcomes::{LocalStatus, Result as SyncResult, SharingStatus},
};
use crate::{
    config::Strategy,
    file_io::{
        atomic_write, decode_json, ensure_directory, existing_write_lock, read_bytes,
        sync_directory, write_json,
    },
    model::check_text,
    storage::{
        StorageError,
        records::{RecordInventory, RecordStore},
    },
    yaml_store::records::{YamlRecordStore, address_from_relative_path, decode_record_tree},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

const CONFIG: &str = ".catalog-local/transport.json";
type Tree = BTreeMap<PathBuf, Vec<u8>>;
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

pub struct RecordSync {
    root: PathBuf,
    host: String,
    selection: Selection,
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

impl RecordSync {
    /// Read only the selected transport's fixed metadata. No Git command or snapshot.
    pub fn open(root: &Path, host: &str) -> Result<Self, StorageError> {
        YamlRecordStore::new(root, host)?.with_reader::<_, StorageError>(|_| Ok(()))?;
        let bytes = read_bytes(&root.join(CONFIG))?
            .ok_or_else(|| invalid("transport is not initialized; run offline catalog cutover"))?;
        let selection: Selection = decode_json(&bytes)?;
        if selection.version != 2 {
            return Err(invalid("unsupported record transport version"));
        }
        check_text(host, "producer", false).map_err(invalid)?;
        if let Mode::Git { remote, branch } = &selection.mode {
            check_text(branch, "branch", false).map_err(invalid)?;
            if let Some(remote) = remote {
                check_text(remote, "remote", false).map_err(invalid)?;
            }
        }
        Ok(Self {
            root: root.into(),
            host: host.into(),
            selection,
            git: Git::new(root, COMMAND_TIMEOUT_SECONDS),
        })
    }

    /// Explicit setup after format initialization or in the private offline cutover candidate.
    pub fn initialize(
        root: &Path,
        host: &str,
        strategy: Strategy,
        remote: Option<String>,
        branch: &str,
    ) -> Result<Self, StorageError> {
        if root.join(CONFIG).exists() {
            let existing = Self::open(root, host)?;
            if existing.strategy() != strategy
                || existing.remote() != remote.as_deref()
                || (strategy == Strategy::Git && existing.branch() != Some(branch))
            {
                return Err(invalid(
                    "catalog transport is already initialized differently",
                ));
            }
            return Ok(existing);
        }
        let records = YamlRecordStore::new(root, host)?.inventory()?;
        for record in &records {
            if crate::storage::records::RecordAddress::for_record(&record.record).host() != host {
                return Err(invalid(
                    "a fresh baseline may contain only this producer's records",
                ));
            }
        }
        let _lock = existing_write_lock(&root.join(".catalog.lock"))?;
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
            Strategy::Git => {
                check_text(branch, "branch", false).map_err(invalid)?;
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
                if let Some(remote) = &remote {
                    check_text(remote, "remote", false).map_err(invalid)?;
                    git.run(&["remote", "add", "origin", remote], None, None, &[0])
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
                Self::tree(&git, &tree)?;
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
                Mode::Git {
                    remote,
                    branch: branch.into(),
                }
            }
        };
        write_json(&root.join(CONFIG), &Selection { version: 2, mode })?;
        drop(_lock);
        Self::open(root, host)
    }
    pub fn strategy(&self) -> Strategy {
        match self.selection.mode {
            Mode::None => Strategy::None,
            Mode::Git { .. } => Strategy::Git,
        }
    }
    pub fn remote(&self) -> Option<&str> {
        match &self.selection.mode {
            Mode::None => None,
            Mode::Git { remote, .. } => remote.as_deref(),
        }
    }
    pub fn branch(&self) -> Option<&str> {
        match &self.selection.mode {
            Mode::None => None,
            Mode::Git { branch, .. } => Some(branch),
        }
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
        let tree = YamlRecordStore::new(&self.root, &self.host)?.read_locked_tree()?;
        decode_record_tree(&tree)?;
        Ok(tree)
    }
    fn checkpoint(
        &self,
        local: &str,
        working: &Tree,
        status: &mut LocalStatus,
    ) -> Result<String, StorageError> {
        let previous = Self::tree(&self.git, local)?;
        for path in changed(&previous, working) {
            if address_from_relative_path(&path)?.host() != self.host {
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
        self.git
            .run(&["read-tree", local], None, Some(&index), &[0])
            .map_err(git_error)?;
        self.git
            .run(
                &["add", "--force", "-A", "--", "hosts"],
                None,
                Some(&index),
                &[0],
            )
            .map_err(git_error)?;
        let tree = self
            .git
            .text(&["write-tree"], None, Some(&index), &[0])
            .map_err(git_error)?;
        if Self::tree(&self.git, &tree)? != *working {
            return Err(invalid("Git filters changed record bytes"));
        }
        let commit = self
            .git
            .text(
                &["commit-tree", &tree, "-p", local],
                Some(b"Save producer records\n"),
                None,
                &[0],
            )
            .map_err(git_error)?;
        *status = LocalStatus::RepairRequired;
        self.advance(&commit, local)?;
        *status = LocalStatus::Committed;
        Ok(commit)
    }
    fn advance(&self, next: &str, previous: &str) -> Result<(), StorageError> {
        let branch = self.branch().expect("Git transport has branch");
        self.git
            .run(
                &[
                    "update-ref",
                    &format!("refs/heads/{branch}"),
                    next,
                    previous,
                ],
                None,
                None,
                &[0],
            )
            .map_err(git_error)?;
        self.git
            .run(&["read-tree", next], None, None, &[0])
            .map_err(git_error)?;
        Ok(())
    }
    fn combine(&self, local: &str, remote: &str) -> Result<(String, Tree), StorageError> {
        let current = Self::tree(&self.git, local)?;
        let incoming = Self::tree(&self.git, remote)?;
        let base = self
            .git
            .text(&["merge-base", local, remote], None, None, &[0, 1])
            .map_err(git_error)?;
        if base == remote {
            return Ok((local.into(), current));
        }
        if base == local {
            return Ok((remote.into(), incoming));
        }
        let base_oid = if base.is_empty() {
            self.git
                .text(&["mktree"], Some(b""), None, &[0])
                .map_err(git_error)?
        } else {
            base
        };
        let base = Self::tree(&self.git, &base_oid)?;
        let local_changes = changed(&base, &current);
        let remote_changes = changed(&base, &incoming);
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
        self.git
            .run(
                &["read-tree", "-m", "--aggressive", &base_oid, local, remote],
                None,
                Some(&index),
                &[0],
            )
            .map_err(git_error)?;
        let tree = self
            .git
            .text(&["write-tree"], None, Some(&index), &[0])
            .map_err(git_error)?;
        if Self::tree(&self.git, &tree)? != combined {
            return Err(invalid(
                "Git tree differs from disjoint-file transport plan",
            ));
        }
        let commit = self
            .git
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
            address_from_relative_path(&path)?;
            crate::yaml_store::records::check_parents(&self.root, &path)?;
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
                    fs::remove_file(&target)
                        .map_err(|error| StorageError::uncertain(error.to_string()))?;
                    sync_directory(target.parent().expect("record parent"))
                        .map_err(StorageError::after_write)?;
                }
            }
        }
        Ok(())
    }
    fn local_head(&self) -> Result<String, StorageError> {
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
        self.git
            .check_target(self.remote(), self.branch().unwrap())
            .map_err(invalid)?;
        let actual = self
            .git
            .text(&["rev-parse", "--show-toplevel"], None, None, &[0])
            .map_err(git_error)?;
        if fs::canonicalize(actual).map_err(invalid)?
            != fs::canonicalize(&self.root).map_err(invalid)?
        {
            return Err(invalid("Git worktree is outside catalog root"));
        }
        self.git.head().map_err(git_error)
    }

    pub fn sync(&self) -> SyncResult {
        if self.strategy() == Strategy::None {
            return match YamlRecordStore::new(&self.root, &self.host)
                .and_then(|store| store.inventory())
            {
                Ok(_) => SyncResult::new(SharingStatus::LocalOnly, LocalStatus::Unchanged),
                Err(error) => SyncResult::new(SharingStatus::Blocked, LocalStatus::RepairRequired)
                    .with_detail(error.to_string()),
            };
        }
        let mut local_status = LocalStatus::Unchanged;
        let mut failure = SharingStatus::Blocked;
        let result = (|| -> Result<SharingStatus, StorageError> {
            let _transport =
                crate::file_io::lock(&self.root.join(".catalog-local/transport.lock"))?;
            let local = {
                let _catalog = existing_write_lock(&self.root.join(".catalog.lock"))?;
                let head = self.local_head()?;
                self.checkpoint(&head, &self.working()?, &mut local_status)?
            };
            if self.remote().is_none() {
                return Ok(SharingStatus::LocalOnly);
            }
            failure = SharingStatus::Failed;
            let reference = format!("refs/heads/{}", self.branch().unwrap());
            let advertised = self
                .git
                .text(
                    &["ls-remote", "--exit-code", "origin", &reference],
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
                    self.git
                        .run(
                            &["fetch", "--quiet", "--no-tags", "origin", &fetch],
                            None,
                            None,
                            &[0],
                        )
                        .map_err(git_error)?;
                    self.git
                        .text(&["rev-parse", "refs/streams/incoming"], None, None, &[0])
                        .map_err(git_error)?
                })
            };
            failure = SharingStatus::Blocked;
            let next = {
                let _catalog = existing_write_lock(&self.root.join(".catalog.lock"))?;
                if self.local_head()? != local {
                    return Err(invalid(
                        "catalog Git tip changed while sync awaited the remote",
                    ));
                }
                let working = self.working()?;
                let local = self.checkpoint(&local, &working, &mut local_status)?;
                match &remote {
                    Some(remote) if remote != &local => {
                        let (next, tree) = self.combine(&local, remote)?;
                        if next != local {
                            local_status = LocalStatus::RepairRequired;
                            self.install(&working, &tree)?;
                            self.advance(&next, &local)?;
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
            failure = SharingStatus::Uncertain;
            self.git
                .run(
                    &["push", "--quiet", "origin", &format!("{next}:{reference}")],
                    None,
                    None,
                    &[0],
                )
                .map_err(git_error)?;
            Ok(SharingStatus::Confirmed)
        })();
        match result {
            Ok(sharing) => SyncResult::new(sharing, local_status),
            Err(error) => SyncResult::new(
                if error.is_uncertain() {
                    SharingStatus::Uncertain
                } else {
                    failure
                },
                local_status,
            )
            .with_detail(error.to_string()),
        }
    }
}
fn changed(before: &Tree, after: &Tree) -> BTreeSet<PathBuf> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
