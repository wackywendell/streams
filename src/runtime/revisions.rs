//! Historical document hints, extracted without snapshotting or changing the checkout.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::checkouts::{CheckoutLocation, relative_path};
use super::{Result, RuntimeError, invalid, text};
use crate::process::{ProcessRequest, ProcessRunner};

/// A cached document location, not authorization to execute a native action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevisionDocument {
    pub checkout_id: String,
    pub root: PathBuf,
    pub file: PathBuf,
    pub revision: String,
}

fn command(
    runner: &dyn ProcessRunner,
    timeout: f64,
    cwd: &Path,
    program: &str,
    args: &[&str],
) -> Result<Vec<u8>> {
    let mut env: std::collections::BTreeMap<_, _> = std::env::vars_os()
        .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
        .collect();
    env.insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
    env.insert("GIT_LITERAL_PATHSPECS".into(), "1".into());
    let prefix: &[&str] = if program == "jj" {
        &[
            "--ignore-working-copy",
            "--at-operation=@",
            "--color=never",
            "--no-pager",
        ]
    } else {
        &[]
    };
    let request = ProcessRequest {
        program: program.into(),
        args: prefix.iter().chain(args).map(Into::into).collect(),
        cwd: Some(cwd.into()),
        input: None,
        env,
        allowed: vec![0],
        timeout,
    };
    let context = || format!("{program} {args:?} in {}", cwd.display());
    let output = runner
        .run(&request)
        .map_err(|e| RuntimeError::Inspection(format!("{}: {e}", context())))?;
    if !output.status.success() {
        return Err(RuntimeError::Inspection(format!(
            "{}: {}: {}",
            context(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(output.stdout)
}

fn private(metadata: &fs::Metadata, path: &Path) -> Result<()> {
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
        return Err(invalid(format!(
            "revision cache must be private and user-owned: {}",
            path.display()
        )));
    }
    Ok(())
}

/// Only aliases above the configured root belong to the trusted OS/user prefix.
fn cache_base(path: &Path) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let parent = absolute
        .parent()
        .ok_or_else(|| invalid("revision cache needs a parent"))?;
    let leaf = absolute
        .file_name()
        .ok_or_else(|| invalid("revision cache needs a name"))?;
    if leaf == "." || leaf == ".." {
        return Err(invalid("revision cache needs a lexical directory name"));
    }
    Ok(crate::config::resolve_path(parent)
        .map_err(|e| invalid(format!("revision cache parent: {e}")))?
        .join(leaf))
}

fn directories(base: &Path, directory: &Path, create: bool) -> Result<()> {
    let relative = directory
        .strip_prefix(base)
        .map_err(|_| invalid("revision cache escape"))?;
    let mut prefix = base.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            prefix.push(component);
        }
        let metadata = match fs::symlink_metadata(&prefix) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                // Parents above base are trusted, but every owned descendant is checked.
                if prefix == base {
                    crate::file_io::ensure_directory(base.parent().expect("cache parent"), true)?;
                }
                match fs::DirBuilder::new().mode(0o700).create(&prefix) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(e) => return Err(e.into()),
                }
                fs::symlink_metadata(&prefix)?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => return Err(e.into()),
        };
        if !metadata.is_dir() {
            return Err(invalid(format!(
                "revision cache directory is not a real directory: {}",
                prefix.display()
            )));
        }
        private(&metadata, &prefix)?;
    }
    Ok(())
}

fn inspect_entry(path: &Path) -> Result<Option<u64>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && !path.is_symlink() => {
            return Ok(None);
        }
        Err(e) => {
            return Err(invalid(format!(
                "cannot read revision cache {}: {e}",
                path.display()
            )));
        }
    };
    let metadata = file.metadata().map_err(|e| {
        invalid(format!(
            "cannot inspect revision cache {}: {e}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(invalid(format!(
            "revision cache entry is not a regular file: {}",
            path.display()
        )));
    }
    private(&metadata, path)?;
    Ok(Some(metadata.len()))
}

fn persist(base: &Path, path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().expect("cache file parent");
    directories(base, parent, true)?;
    // Preserve cache ingress checks even if another extraction populated the leaf.
    inspect_entry(path)?;
    let write_error = |e| {
        invalid(format!(
            "cannot write revision cache {}: {e}",
            path.display()
        ))
    };
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(write_error)?;
    temporary.write_all(content).map_err(write_error)?;
    temporary.as_file().sync_all().map_err(write_error)?;
    temporary.persist(path).map_err(|e| write_error(e.error))?;
    Ok(())
}

pub(super) fn extract(
    checkout: &CheckoutLocation,
    path: &str,
    revision: &str,
    runner: &dyn ProcessRunner,
    timeout: f64,
    cache_root: &Path,
) -> Result<RevisionDocument> {
    relative_path(path, false)?;
    text(revision)?;
    let rev = revision.trim();
    if rev.starts_with('-') {
        return Err(invalid("revision must not start with a dash"));
    }
    crate::process::deadline(timeout, "revision lookup").map_err(|e| invalid(e.to_string()))?;
    // Registry IDs are opaque text on disk; reject unsafe cache components without
    // tightening the registry's accepted wire grammar.
    relative_path(&checkout.id, false)?;
    if checkout.id.contains('/') {
        return Err(invalid("checkout ID cannot be used as a cache component"));
    }
    let base = cache_base(cache_root)?;
    // v2 starts fresh: v1 truncated identities, transformed bytes and missing hints
    // are derived state, not inputs to this cache.
    let checkout_root = base.join("v2").join(&checkout.id);
    directories(&base, &checkout_root, false)?;
    let program = if checkout.directory.join(".jj").is_dir() {
        "jj"
    } else {
        "git"
    };
    let run = |args: &[&str]| command(runner, timeout, &checkout.directory, program, args);
    let output = if program == "jj" {
        run(&["log", "--no-graph", "-r", rev, "-T", "commit_id ++ \"\\n\""])?
    } else {
        run(&["rev-parse", "--verify", &format!("{rev}^{{commit}}")])?
    };
    let resolved = std::str::from_utf8(&output)
        .map_err(|_| invalid("revision resolution must return one full commit ID"))?
        .trim();
    if !matches!(resolved.len(), 40 | 64) || !resolved.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid(
            "revision resolution must return one full commit ID",
        ));
    }
    let target = RevisionDocument {
        checkout_id: checkout.id.clone(),
        root: checkout_root.join(resolved),
        file: checkout_root.join(resolved).join(path),
        revision: revision.into(),
    };
    directories(&base, target.file.parent().expect("cache parent"), false)?;
    if inspect_entry(&target.file)?.is_some() {
        return Ok(target);
    }

    // A successful exact-path listing proves absence; a failed command does not.
    // Quote jj's fileset syntax and disable Git pathspec expansion.
    let fileset = format!(
        "root-file:{}",
        serde_json::to_string(path).expect("path string")
    );
    let kind = if program == "jj" {
        run(&[
            "file",
            "list",
            "-r",
            resolved,
            "-T",
            "file_type ++ \"\\0\"",
            "--",
            &fileset,
        ])?
    } else {
        run(&[
            "ls-tree",
            "-z",
            "--full-tree",
            "--format=%(objecttype)",
            resolved,
            "--",
            path,
        ])?
    };
    if kind.is_empty() {
        return Err(RuntimeError::NotFound(format!(
            "document does not exist in {program}: {path} at {revision}"
        )));
    }
    if kind
        != if program == "jj" {
            b"file\0".as_slice()
        } else {
            b"blob\0".as_slice()
        }
    {
        return Err(invalid(format!(
            "document is not a regular file in {program}: {path} at {revision}"
        )));
    }
    let content = if program == "jj" {
        run(&["file", "show", "-r", resolved, "-T", "\"\"", "--", &fileset])?
    } else {
        run(&["show", &format!("{resolved}:{path}")])?
    };
    persist(&base, &target.file, &content)?;
    Ok(target)
}
