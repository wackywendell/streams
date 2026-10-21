//! Git subprocesses and cached objects bound to one repository root.
//!
//! Raw commands use a process group and a deadline that includes pipe I/O, even
//! after Git exits while descendants still hold pipes open. Timeout or I/O failure
//! triggers group termination and reaping; an error does not prove no effect occurred.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::process;

use crate::storage::StorageError;

use super::history::History;

/// A Git failure with an exit code when one was observed.
///
/// Missing exit status is treated conservatively as uncertain, including failures
/// before spawning. An exit code alone does not establish remote publication state.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct GitError {
    pub message: String,
    pub code: Option<i32>,
    pub uncertain: bool,
}

impl GitError {
    /// Marks failures without an observed exit code as uncertain.
    pub fn new(message: impl Into<String>, code: Option<i32>) -> Self {
        Self {
            message: message.into(),
            code,
            uncertain: code.is_none(),
        }
    }
}

/// Intercepts [`Git::run`]; `None` falls through to the real subprocess.
///
/// The hook runs under its mutex. Call [`Git::run_raw`] from a hook, not `run`,
/// to avoid recursively acquiring that mutex. Hooks are not deadline-bounded.
pub type GitHook = Arc<
    Mutex<
        dyn FnMut(&Git, &[&str], Option<&[u8]>, Option<&Path>) -> Option<Result<Vec<u8>, GitError>>
            + Send,
    >,
>;

type HistoryCache = Vec<(Option<String>, History)>;

/// A repository handle whose clones share blob and validated-history caches.
///
/// The root cannot be changed through this handle: caches belong to that repository.
/// Callers must not repoint the root path at a different repository while reusing it.
#[derive(Clone)]
pub struct Git {
    root: PathBuf,
    /// Per-subprocess timeout in seconds; commands reject nonpositive or invalid values.
    pub timeout: f64,
    pub(crate) histories: Arc<Mutex<HistoryCache>>,
    blobs: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    hook: Option<GitHook>,
}

impl std::fmt::Debug for Git {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Git").field("root", &self.root).finish()
    }
}

impl Git {
    fn deadline(&self, args: &[&str]) -> Result<Instant, GitError> {
        if args.is_empty() {
            return Err(GitError::new("Git requires at least one argument", None));
        }
        process::deadline(self.timeout, "Git").map_err(|e| GitError::new(e.message, None))
    }

    /// Binds empty caches to `root`; repository access and timeout checks are deferred.
    pub fn new(root: impl AsRef<Path>, timeout: f64) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            timeout,
            histories: Arc::new(Mutex::new(Vec::new())),
            blobs: Arc::new(Mutex::new(BTreeMap::new())),
            hook: None,
        }
    }

    /// Copy only command interception to a new root; object/history caches stay fresh.
    pub(crate) fn fresh_root(&self, root: &Path) -> Self {
        let mut fresh = Self::new(root, self.timeout);
        fresh.hook = self.hook.clone();
        fresh
    }

    /// Returns the configured root, without resolving or validating the path.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Installs an interceptor for commands issued through [`Self::run`].
    pub fn with_hook(mut self, hook: GitHook) -> Self {
        self.hook = Some(hook);
        self
    }

    /// Executes Git without the hook, returning stdout only for `allowed` exit codes.
    ///
    /// Zero has no special treatment: include it explicitly. A signal exit or any
    /// unlisted code is an error. `index` overrides the index for this command only.
    /// Inherited `GIT_*` overrides are removed except global/system config selectors;
    /// normal Git configuration still applies, and terminal prompts are disabled.
    pub fn run_raw(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        index: Option<&Path>,
        allowed: &[i32],
    ) -> Result<Vec<u8>, GitError> {
        let deadline = self.deadline(args)?;
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.root).args(args);
        cmd.env_clear();
        for (k, v) in std::env::vars_os() {
            let k_str = k.to_string_lossy();
            if !k_str.starts_with("GIT_")
                || matches!(
                    k_str.as_ref(),
                    "GIT_CONFIG_GLOBAL" | "GIT_CONFIG_SYSTEM" | "GIT_CONFIG_NOSYSTEM"
                )
            {
                cmd.env(k, v);
            }
        }
        cmd.env("GIT_TERMINAL_PROMPT", "0");
        cmd.env("GIT_LITERAL_PATHSPECS", "1");
        // Status must not refresh the index during read-only catalog inspection.
        cmd.env("GIT_OPTIONAL_LOCKS", "0");
        if let Some(idx) = index {
            cmd.env("GIT_INDEX_FILE", idx);
        }
        let output = process::execute(&mut cmd, input, deadline, &format!("Git {}", args[0]))
            .map_err(|e| GitError::new(e.message, None))?;
        let process::ProcessOutput {
            status,
            stdout,
            stderr,
        } = output;
        let code = status.code();
        if !code.is_some_and(|code| allowed.contains(&code)) {
            let stderr = String::from_utf8_lossy(&stderr).trim().to_string();
            return Err(GitError::new(
                format!("Git {} failed ({status}): {stderr}", args[0]),
                code,
            ));
        }
        Ok(stdout)
    }

    /// Checks arguments and timeout, then offers the command to the hook.
    ///
    /// An intercepted result is returned unchanged, without exit-code filtering.
    /// Otherwise [`Self::run_raw`] starts its own subprocess deadline.
    pub fn run(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        index: Option<&Path>,
        allowed: &[i32],
    ) -> Result<Vec<u8>, GitError> {
        self.deadline(args)?;
        if let Some(ref hook) = self.hook {
            let intercepted = {
                let mut guard = hook.lock().unwrap();
                guard(self, args, input, index)
            };
            if let Some(res) = intercepted {
                return res;
            }
        }
        self.run_raw(args, input, index, allowed)
    }

    /// Runs a command, replacing invalid UTF-8 and trimming surrounding whitespace.
    pub fn text(
        &self,
        args: &[&str],
        input: Option<&[u8]>,
        index: Option<&Path>,
        allowed: &[i32],
    ) -> Result<String, GitError> {
        let out = self.run(args, input, index, allowed)?;
        Ok(String::from_utf8_lossy(&out).trim().to_string())
    }

    /// Resolves `HEAD`; an unborn branch is an error.
    pub fn head(&self) -> Result<String, GitError> {
        self.text(&["rev-parse", "--verify", "HEAD"], None, None, &[0])
    }

    /// Tests ancestry by comparing the merge base to `before` (a full object ID).
    pub fn ancestor(&self, before: &str, after: &str) -> Result<bool, GitError> {
        let out = self.text(&["merge-base", before, after], None, None, &[0, 1])?;
        Ok(out == before)
    }

    /// Reads a revision's UTF-8 paths and blob bytes, caching blobs by object ID.
    ///
    /// Rejects non-regular entries, including symlinks, executable files and submodules;
    /// decoding catalog records is the caller's responsibility.
    pub fn files(&self, revision: &str) -> Result<BTreeMap<String, Vec<u8>>, StorageError> {
        let raw = self
            .run(
                &["ls-tree", "-rz", "--full-tree", revision],
                None,
                None,
                &[0],
            )
            .map_err(|e| {
                if e.uncertain {
                    StorageError::uncertain(e.message)
                } else {
                    StorageError::new(e.message)
                }
            })?;
        let mut paths = BTreeMap::new();
        for entry in raw.split(|&b| b == 0) {
            if entry.is_empty() {
                continue;
            }
            let Some(tab_pos) = entry.iter().position(|&b| b == b'\t') else {
                return Err(StorageError::new("invalid ls-tree entry"));
            };
            let desc = std::str::from_utf8(&entry[..tab_pos])
                .map_err(|e| StorageError::new(e.to_string()))?;
            let path = std::str::from_utf8(&entry[tab_pos + 1..])
                .map_err(|e| StorageError::new(e.to_string()))?;
            let parts: Vec<&str> = desc.split_whitespace().collect();
            if parts.len() != 3 || parts[0] != "100644" || parts[1] != "blob" {
                return Err(StorageError::new(
                    "catalog history contains a non-regular record",
                ));
            }
            paths.insert(path.to_string(), parts[2].to_string());
        }
        let missing: Vec<String> = {
            let blobs = self.blobs.lock().unwrap();
            let unique_oids: BTreeSet<String> = paths.values().cloned().collect();
            unique_oids
                .into_iter()
                .filter(|oid| !blobs.contains_key(oid))
                .collect()
        };
        if !missing.is_empty() {
            let batch_in = format!("{}\n", missing.join("\n"));
            let output = self
                .run(
                    &["cat-file", "--batch"],
                    Some(batch_in.as_bytes()),
                    None,
                    &[0],
                )
                .map_err(|e| {
                    if e.uncertain {
                        StorageError::uncertain(e.message)
                    } else {
                        StorageError::new(e.message)
                    }
                })?;
            let mut offset = 0usize;
            let mut blobs = self.blobs.lock().unwrap();
            for expected in &missing {
                let Some(rel_end) = output[offset..].iter().position(|&b| b == b'\n') else {
                    return Err(StorageError::new("invalid Git blob batch response"));
                };
                let end = offset + rel_end;
                let header = std::str::from_utf8(&output[offset..end])
                    .map_err(|e| StorageError::new(e.to_string()))?;
                let parts: Vec<&str> = header.split_whitespace().collect();
                if parts.len() != 3 || parts[0] != expected || parts[1] != "blob" {
                    return Err(StorageError::new("invalid Git blob batch response"));
                }
                let size: usize = parts[2]
                    .parse()
                    .map_err(|_| StorageError::new("invalid Git blob batch response"))?;
                offset = end + 1;
                if offset + size > output.len() {
                    return Err(StorageError::new("invalid Git blob batch response"));
                }
                blobs.insert(expected.clone(), output[offset..offset + size].to_vec());
                offset += size + 1;
            }
        }
        let blobs = self.blobs.lock().unwrap();
        let mut out = BTreeMap::new();
        for (path, oid) in paths {
            let Some(bytes) = blobs.get(&oid) else {
                return Err(StorageError::new("missing cached Git blob"));
            };
            out.insert(path, bytes.clone());
        }
        Ok(out)
    }

    /// Creates a commit object without moving a ref or changing the working tree.
    ///
    /// Reads `commit.gpgsign` for each call and requests signing when enabled; identity,
    /// signing keys and other behavior come from the current Git configuration.
    pub fn commit(
        &self,
        tree: &str,
        message: &str,
        parent: Option<&str>,
    ) -> Result<String, GitError> {
        let mut args = vec!["commit-tree", tree];
        if let Some(p) = parent {
            args.push("-p");
            args.push(p);
        }
        let signed = self.text(
            &["config", "--bool", "--get", "commit.gpgsign"],
            None,
            None,
            &[0, 1],
        )?;
        if signed == "true" {
            args.push("-S");
        }
        self.text(&args, Some(message.as_bytes()), None, &[0])
    }
}
