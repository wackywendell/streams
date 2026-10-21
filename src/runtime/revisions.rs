//! Historical document hints, extracted without snapshotting or changing the checkout.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::checkouts::{CheckoutLocation, relative_path};
use super::{Result, RuntimeError, digest, invalid, text};
use crate::process::{ProcessRequest, ProcessRunner};

/// A cached document location, not authorization to execute a native action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RevisionDocument {
    pub checkout_id: String,
    pub root: PathBuf,
    pub file: PathBuf,
    pub revision: String,
}

fn immutable(value: &str) -> bool {
    value.len() >= 12 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn command(
    runner: &dyn ProcessRunner,
    timeout: f64,
    cwd: &Path,
    program: &str,
    args: &[&str],
) -> std::result::Result<String, String> {
    let mut env: std::collections::BTreeMap<_, _> = std::env::vars_os()
        .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
        .collect();
    env.insert("GIT_OPTIONAL_LOCKS".into(), "0".into());
    let request = ProcessRequest {
        program: program.into(),
        args: args.iter().map(Into::into).collect(),
        cwd: Some(cwd.into()),
        input: None,
        env,
        allowed: vec![0],
        timeout,
    };
    let context = || format!("{program} {args:?} in {}", cwd.display());
    let output = runner
        .run(&request)
        .map_err(|e| format!("{}: {e}", context()))?;
    if !output.status.success() {
        return Err(format!(
            "{}: {}: {}",
            context(),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    // Python subprocess text mode normalizes universal newlines before rstrip.
    let output = String::from_utf8(output.stdout).map_err(|e| format!("{}: {e}", context()))?;
    Ok(output
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .trim_end_matches('\n')
        .into())
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
    // Recheck the leaf even when replacing an unresolved mutable expression.
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
    directories(&base, &base.join(&checkout.id), false)?;
    let has_jj = checkout.directory.join(".jj").is_dir();
    let mut resolved = rev.to_owned();
    let mut failures = Vec::new();
    if !immutable(rev) {
        if has_jj {
            match command(
                runner,
                timeout,
                &checkout.directory,
                "jj",
                &[
                    "--ignore-working-copy",
                    "--at-operation=@",
                    "log",
                    "--no-graph",
                    "-r",
                    rev,
                    "-T",
                    "commit_id ++ \"\\n\"",
                ],
            ) {
                Ok(output) => {
                    if let Some(first) = output
                        .lines()
                        .next()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        resolved = first.into();
                    }
                }
                Err(error) => failures.push(error),
            }
        }
        if resolved == rev {
            match command(
                runner,
                timeout,
                &checkout.directory,
                "git",
                &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
            ) {
                Ok(output) if !output.trim().is_empty() => resolved = output.trim().into(),
                Ok(_) => {}
                Err(error) => failures.push(error),
            }
        }
    }
    let reusable = immutable(rev) || resolved != rev;
    let key = if immutable(&resolved) {
        resolved[..12].to_owned()
    } else {
        digest(resolved.as_bytes())[..12].to_owned()
    };
    let root = base.join(&checkout.id).join(key);
    let file = root.join(path);
    let missing = file.with_file_name(format!(
        ".{}.missing",
        file.file_name().expect("relative file").to_string_lossy()
    ));
    directories(&base, file.parent().expect("cache parent"), false)?;
    let content = inspect_entry(&file)?;
    let absent = inspect_entry(&missing)?;
    if absent.is_some_and(|size| size != 0 || content.is_some() || !reusable) {
        return Err(invalid(format!(
            "corrupt revision missing hint: {}",
            missing.display()
        )));
    }
    let target = RevisionDocument {
        checkout_id: checkout.id.clone(),
        root,
        file,
        revision: revision.into(),
    };
    if reusable && content.is_some() {
        return Ok(target);
    }
    let unavailable = |failures: &[String]| {
        RuntimeError::NotFound(format!(
            "document revision is unavailable: {path} at {revision}{}",
            if failures.is_empty() {
                String::new()
            } else {
                format!("; {}", failures.join("; "))
            }
        ))
    };
    if reusable && absent.is_some() {
        return Err(unavailable(&[]));
    }
    let mut content = None;
    if has_jj {
        match command(
            runner,
            timeout,
            &checkout.directory,
            "jj",
            &[
                "--ignore-working-copy",
                "--at-operation=@",
                "file",
                "show",
                "-r",
                &resolved,
                path,
            ],
        ) {
            Ok(output) => content = Some(output),
            Err(error) => failures.push(error),
        }
    }
    if content.is_none() {
        match command(
            runner,
            timeout,
            &checkout.directory,
            "git",
            &["show", &format!("{resolved}:{path}")],
        ) {
            Ok(output) => content = Some(output),
            Err(error) => failures.push(error),
        }
    }
    if let Some(mut content) = content {
        content.push('\n');
        persist(&base, &target.file, content.as_bytes())?;
        return Ok(target);
    }
    if reusable {
        persist(&base, &missing, b"")?;
    }
    Err(unavailable(&failures))
}
