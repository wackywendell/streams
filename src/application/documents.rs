//! Session directory inspection, document path normalization, and document file lookup.

pub mod canonical;

use std::{
    ffi::OsStr,
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

use crate::model::{RepositoryInfo, Session, SessionDirectory, check_path_key};

use super::ApplicationError;

/// Result of locating a Session document on the local host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DocumentLocation {
    Path { path: PathBuf },
    Unavailable { reason: String },
}

impl DocumentLocation {
    pub fn available_path(&self) -> Option<&Path> {
        match self {
            Self::Path { path } => Some(path.as_path()),
            Self::Unavailable { .. } => None,
        }
    }

    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Path { .. } => None,
            Self::Unavailable { reason } => Some(reason.as_str()),
        }
    }
}

/// Locate a document reference's file on `local_host` without starting any VCS process.
///
/// 1. Check `key` against the path key rule.
/// 2. If the Session belongs to another host or has no `directory`, return "no directory here".
/// 3. Canonicalize `directory.path` and `directory.path / key`; missing directory gives
///    "no directory here", missing file gives "missing".
/// 4. Verify the resolved file stays inside the resolved directory and is a regular file.
pub fn locate(
    local_host: &str,
    session: &Session,
    key: &str,
) -> Result<DocumentLocation, ApplicationError> {
    check_path_key(key, "document path")?;
    if session.host_id != local_host {
        return Ok(DocumentLocation::Unavailable {
            reason: "no directory here".into(),
        });
    }
    let Some(directory) = &session.directory else {
        return Ok(DocumentLocation::Unavailable {
            reason: "no directory here".into(),
        });
    };
    let dir_path = Path::new(&directory.path);
    let resolved_dir = match fs::canonicalize(dir_path) {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(DocumentLocation::Unavailable {
                reason: "no directory here".into(),
            });
        }
        Err(error) => {
            return Err(ApplicationError::invalid(format!(
                "cannot resolve Session directory {}: {error}",
                dir_path.display()
            )));
        }
    };
    let candidate = resolved_dir.join(key);
    let resolved_file = match fs::canonicalize(&candidate) {
        Ok(path) => path,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(DocumentLocation::Unavailable {
                reason: "missing".into(),
            });
        }
        Err(error) => {
            return Err(ApplicationError::invalid(format!(
                "cannot resolve document {}: {error}",
                candidate.display()
            )));
        }
    };
    if !resolved_file.starts_with(&resolved_dir) {
        return Ok(DocumentLocation::Unavailable {
            reason: "outside the Session's directory".into(),
        });
    }
    let metadata = fs::metadata(&resolved_file).map_err(|error| {
        ApplicationError::invalid(format!(
            "cannot inspect document {}: {error}",
            resolved_file.display()
        ))
    })?;
    if !metadata.is_file() {
        return Ok(DocumentLocation::Unavailable {
            reason: "not a file".into(),
        });
    }
    Ok(DocumentLocation::Path {
        path: resolved_file,
    })
}

/// Normalize a user-supplied document path (relative or absolute) into a Session path key.
pub fn normalize_document_key(
    session_directory: Option<&SessionDirectory>,
    raw: &str,
) -> Result<String, ApplicationError> {
    if raw.contains('\\') {
        return Err(ApplicationError::invalid(
            "document path must use forward slashes",
        ));
    }
    let path = Path::new(raw);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(ApplicationError::invalid(format!(
            "document path {raw:?} cannot contain '..'"
        )));
    }
    if !path.is_absolute() {
        check_path_key(raw, "document path")?;
        return Ok(raw.to_owned());
    }
    let Some(dir) = session_directory else {
        return Err(ApplicationError::invalid(format!(
            "absolute document path {raw:?} requires the Session to have a directory; set --directory, save an artifact, or add a link"
        )));
    };
    let dir_path = Path::new(&dir.path);
    if let Ok(remainder) = path.strip_prefix(dir_path) {
        return finish_relative_key(remainder, &dir.path);
    }
    // Resolve the longest existing ancestor of the parent directory while keeping the final
    // filename untouched (supports non-existent files and symlinked filenames).
    let canonical_input = canonicalize_parent_keep_leaf(path);
    if let Some(resolved) = canonical_input {
        if let Ok(remainder) = resolved.strip_prefix(dir_path) {
            return finish_relative_key(remainder, &dir.path);
        }
        if let Ok(canonical_dir) = fs::canonicalize(dir_path)
            && let Ok(remainder) = resolved.strip_prefix(&canonical_dir)
        {
            return finish_relative_key(remainder, &dir.path);
        }
    }
    Err(ApplicationError::invalid(format!(
        "document path {raw:?} is outside the Session's directory {}; save it as an artifact or add a link instead",
        dir.path
    )))
}

fn canonicalize_parent_keep_leaf(path: &Path) -> Option<PathBuf> {
    let file_name = path.file_name()?;
    let parent = path.parent()?;
    let mut current = parent;
    let mut tail: Vec<&OsStr> = Vec::new();
    loop {
        if let Ok(resolved) = fs::canonicalize(current) {
            let mut out = resolved;
            for part in tail.into_iter().rev() {
                out.push(part);
            }
            out.push(file_name);
            return Some(out);
        }
        let part = current.file_name()?;
        tail.push(part);
        current = current.parent()?;
    }
}

fn finish_relative_key(remainder: &Path, dir_path: &str) -> Result<String, ApplicationError> {
    let mut segments = Vec::new();
    for component in remainder.components() {
        match component {
            Component::Normal(part) => {
                let text = part.to_str().ok_or_else(|| {
                    ApplicationError::invalid("document path must be valid UTF-8")
                })?;
                segments.push(text);
            }
            Component::CurDir => {}
            _ => {
                return Err(ApplicationError::invalid(format!(
                    "document path is outside the Session's directory {dir_path}; save it as an artifact or add a link instead"
                )));
            }
        }
    }
    let key = segments.join("/");
    check_path_key(&key, "document path")?;
    Ok(key)
}

/// Output of one VCS subprocess invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VcsOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Injectable VCS runner so directory inspection tests stay deterministic and fast.
pub trait VcsRunner {
    /// Returns `Ok(None)` when `program` is not installed (`ErrorKind::NotFound`).
    fn run(&self, program: &str, args: &[&str]) -> Result<Option<VcsOutput>, ApplicationError>;
}

pub struct SystemVcsRunner;

impl VcsRunner for SystemVcsRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<Option<VcsOutput>, ApplicationError> {
        let output = match Command::new(program).args(args).output() {
            Ok(output) => output,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(ApplicationError::invalid(format!(
                    "failed to run {program}: {error}"
                )));
            }
        };
        Ok(Some(VcsOutput {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }))
    }
}

/// Resolve `path` and inspect its VCS clone once to construct a `SessionDirectory`.
pub fn inspect_directory(path: &Path) -> Result<SessionDirectory, ApplicationError> {
    inspect_directory_with_runner(path, &SystemVcsRunner)
}

fn canonical_directory(path: &Path) -> Result<PathBuf, ApplicationError> {
    let resolved = fs::canonicalize(path).map_err(|error| {
        ApplicationError::invalid(format!(
            "cannot resolve directory {}: {error}",
            path.display()
        ))
    })?;
    if !resolved.is_dir() {
        return Err(ApplicationError::invalid(format!(
            "{} is not a directory",
            resolved.display()
        )));
    }
    canonical::spelling(&resolved).map_err(|error| {
        ApplicationError::invalid(format!(
            "cannot resolve directory {}: {error}",
            path.display()
        ))
    })
}

pub fn inspect_directory_with_runner(
    path: &Path,
    runner: &dyn VcsRunner,
) -> Result<SessionDirectory, ApplicationError> {
    let canonical_dir = canonical_directory(path)?;
    let dir_str = canonical_dir
        .to_str()
        .ok_or_else(|| ApplicationError::invalid("directory path must be valid UTF-8"))?
        .to_owned();

    let repository = if let Some(repo) = inspect_jj(&canonical_dir, &dir_str, runner)? {
        repo
    } else {
        inspect_git(&canonical_dir, &dir_str, runner)?
    };

    let session_dir = SessionDirectory {
        path: dir_str,
        repository,
    };
    session_dir.validate()?;
    Ok(session_dir)
}

/// Returns `Ok(Some(opt_repo))` when `dir` is inside a jj workspace (`opt_repo` is `None` when
/// the workspace has no git remote), or `Ok(None)` when `dir` is not a jj workspace (or `jj` is
/// unavailable) so callers fall through to Git inspection.
fn inspect_jj(
    canonical_dir: &Path,
    dir_str: &str,
    runner: &dyn VcsRunner,
) -> Result<Option<Option<RepositoryInfo>>, ApplicationError> {
    let Some(root_out) = runner.run(
        "jj",
        &["--ignore-working-copy", "--repository", dir_str, "root"],
    )?
    else {
        return Ok(None);
    };
    if root_out.status != 0 {
        let lower = root_out.stderr.to_ascii_lowercase();
        if lower.contains("no jj repo") || lower.contains("there is no jj repo") {
            return Ok(None);
        }
        return Err(ApplicationError::invalid(format!(
            "jj root failed for {dir_str}: {}",
            root_out.stderr.trim()
        )));
    }
    let root_text = root_out.stdout.trim();
    let repo_root =
        canonical_directory(Path::new(root_text)).unwrap_or_else(|_| PathBuf::from(root_text));
    let within = compute_within(canonical_dir, &repo_root)?;

    let remotes_out = runner
        .run(
            "jj",
            &[
                "--ignore-working-copy",
                "--repository",
                dir_str,
                "git",
                "remote",
                "list",
            ],
        )?
        .ok_or_else(|| ApplicationError::invalid("jj disappeared during inspection"))?;
    if remotes_out.status != 0 {
        let lower = remotes_out.stderr.to_ascii_lowercase();
        if lower.contains("not a git") || lower.contains("no git") {
            return Ok(Some(None));
        }
        return Err(ApplicationError::invalid(format!(
            "jj git remote list failed for {dir_str}: {}",
            remotes_out.stderr.trim()
        )));
    }
    let Some(url) = pick_jj_remote(&remotes_out.stdout) else {
        return Ok(Some(None));
    };

    let branch_out = runner
        .run(
            "jj",
            &[
                "--ignore-working-copy",
                "--repository",
                dir_str,
                "log",
                "-r",
                "heads(::@ & bookmarks())",
                "--no-graph",
                "-T",
                "local_bookmarks.map(|b| b.name()).join(\"\\n\") ++ \"\\n\"",
            ],
        )?
        .ok_or_else(|| ApplicationError::invalid("jj disappeared during inspection"))?;
    if branch_out.status != 0 {
        return Err(ApplicationError::invalid(format!(
            "jj bookmark lookup failed for {dir_str}: {}",
            branch_out.stderr.trim()
        )));
    }
    let bookmarks: Vec<&str> = branch_out
        .stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let branch = if bookmarks.len() == 1 {
        Some(bookmarks[0].to_owned())
    } else {
        None
    };

    Ok(Some(Some(RepositoryInfo {
        url,
        within,
        branch,
    })))
}

fn pick_jj_remote(output: &str) -> Option<String> {
    let mut first = None;
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let Some(name) = parts.next() else {
            continue;
        };
        let Some(url) = parts.next() else {
            continue;
        };
        if name == "origin" {
            return Some(url.to_owned());
        }
        if first.is_none() {
            first = Some(url.to_owned());
        }
    }
    first
}

fn inspect_git(
    canonical_dir: &Path,
    dir_str: &str,
    runner: &dyn VcsRunner,
) -> Result<Option<RepositoryInfo>, ApplicationError> {
    let Some(root_out) = runner.run("git", &["-C", dir_str, "rev-parse", "--show-toplevel"])?
    else {
        return Ok(None);
    };
    if root_out.status != 0 {
        let lower = root_out.stderr.to_ascii_lowercase();
        if lower.contains("not a git repository") {
            return Ok(None);
        }
        return Err(ApplicationError::invalid(format!(
            "git rev-parse failed for {dir_str}: {}",
            root_out.stderr.trim()
        )));
    }
    let root_text = root_out.stdout.trim();
    let repo_root =
        canonical_directory(Path::new(root_text)).unwrap_or_else(|_| PathBuf::from(root_text));
    let within = compute_within(canonical_dir, &repo_root)?;

    let url_out = runner
        .run(
            "git",
            &["-C", dir_str, "config", "--get", "remote.origin.url"],
        )?
        .ok_or_else(|| ApplicationError::invalid("git disappeared during inspection"))?;
    let url = if url_out.status == 0 {
        let trimmed = url_out.stdout.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        trimmed.to_owned()
    } else if url_out.status == 1 && url_out.stderr.trim().is_empty() {
        // No origin remote configured.
        return Ok(None);
    } else {
        return Err(ApplicationError::invalid(format!(
            "git remote lookup failed for {dir_str}: {}",
            url_out.stderr.trim()
        )));
    };

    let branch_out = runner
        .run(
            "git",
            &["-C", dir_str, "symbolic-ref", "--short", "-q", "HEAD"],
        )?
        .ok_or_else(|| ApplicationError::invalid("git disappeared during inspection"))?;
    let branch = if branch_out.status == 0 {
        let trimmed = branch_out.stdout.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    } else if branch_out.status == 1 && branch_out.stderr.trim().is_empty() {
        // Detached HEAD.
        None
    } else {
        return Err(ApplicationError::invalid(format!(
            "git branch lookup failed for {dir_str}: {}",
            branch_out.stderr.trim()
        )));
    };

    Ok(Some(RepositoryInfo {
        url,
        within,
        branch,
    }))
}

fn compute_within(dir: &Path, repo_root: &Path) -> Result<Option<String>, ApplicationError> {
    let relative = dir.strip_prefix(repo_root).map_err(|_| {
        ApplicationError::invalid(format!(
            "directory {} is not inside repository root {}",
            dir.display(),
            repo_root.display()
        ))
    })?;
    let mut parts = Vec::new();
    for component in relative.components() {
        if let Component::Normal(part) = component {
            let text = part
                .to_str()
                .ok_or_else(|| ApplicationError::invalid("repository path must be valid UTF-8"))?;
            parts.push(text);
        }
    }
    if parts.is_empty() {
        Ok(None)
    } else {
        let joined = parts.join("/");
        check_path_key(&joined, "directory.repository.within")?;
        Ok(Some(joined))
    }
}
