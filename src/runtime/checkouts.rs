//! Stable host-local checkout IDs and exact, explicit repository URL bindings.

use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

mod canonical;
mod state;
use state::State;

use super::{Result, RuntimeError, digest, invalid, lock, read_json, text, write_json};

/// A registered physical root. Repository URLs are exact locators, not inferred remotes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutLocation {
    /// Opaque persisted identity; new registrations generate UUIDs.
    pub id: String,
    pub directory: PathBuf,
    pub repository_urls: Vec<String>,
    pub device: u64,
    pub inode: u64,
}

/// A discovered checkout root and the inspected directory's portable path within it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryInspection {
    pub root: PathBuf,
    pub relative_cwd: String,
}

/// Read-only checkout inspection, replaceable for isolated filesystem tests.
pub type Inspector = fn(&Path) -> Result<Option<RepositoryInspection>>;

/// Filesystem-backed host registry. Construction and reads never create state.
#[derive(Debug, Clone)]
pub struct CheckoutRegistry {
    path: PathBuf,
    host_id: String,
    inspector: Inspector,
}

/// Resolve an existing directory, using actual directory-entry spelling on macOS too.
pub fn canonical_directory(path: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(path)?;
    if !root.is_dir() {
        return Err(invalid("expected an existing directory"));
    }
    let canonical = canonical::spelling(&root)?;
    text(
        canonical
            .to_str()
            .ok_or_else(|| invalid("directory must be UTF-8"))?,
    )?;
    Ok(canonical)
}

fn physical(metadata: &fs::Metadata) -> (u64, u64) {
    (metadata.dev(), metadata.ino())
}

/// Reject nonportable paths before joining them to a checkout root.
pub fn relative_path(value: &str, allow_root: bool) -> Result<()> {
    text(value)?;
    if value == "." && allow_root {
        return Ok(());
    }
    if value.contains('\\')
        || Path::new(value).is_absolute()
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
        || value
            .split('/')
            .next()
            .is_some_and(|part| part.contains(':'))
    {
        return Err(invalid("expected a nonescaping portable relative path"));
    }
    Ok(())
}

fn canonical_saved_path(path: &Path) -> Result<()> {
    let value = path
        .to_str()
        .ok_or_else(|| invalid("directory must be UTF-8"))?;
    text(value)?;
    if !path.is_absolute()
        || value.starts_with("//")
        || value != "/"
            && (value.ends_with('/')
                || value[1..]
                    .split('/')
                    .any(|part| matches!(part, "" | "." | "..")))
    {
        return Err(invalid("checkout directory must be canonical and absolute"));
    }
    Ok(())
}

impl CheckoutRegistry {
    /// Bind a registry to its configured host, without reading any local state.
    pub fn new(state_dir: impl AsRef<Path>, host_id: &str) -> Result<Self> {
        text(host_id)?;
        Ok(Self {
            path: state_dir
                .as_ref()
                .join(format!("checkouts-{}.json", digest(host_id.as_bytes()))),
            host_id: host_id.into(),
            inspector: inspect_directory,
        })
    }

    /// Return the configured host-local registry path without reading or creating it.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Override only read-only repository inspection; identity and containment checks remain active.
    pub fn with_inspector(mut self, inspector: Inspector) -> Self {
        self.inspector = inspector;
        self
    }

    pub(crate) fn inspect(&self, path: &Path) -> Result<Option<RepositoryInspection>> {
        (self.inspector)(path)
    }

    fn read(&self) -> Result<State> {
        let state: State = read_json(&self.path)?.unwrap_or_else(|| State::new(&self.host_id));
        state.check(&self.host_id)?;
        let mut live = BTreeSet::new();
        for checkout in &state.checkouts {
            match fs::metadata(&checkout.directory) {
                Ok(metadata) => {
                    if !live.insert(physical(&metadata)) {
                        return Err(invalid("duplicate physical checkout roots"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(state)
    }

    fn update<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let _lock = lock(&self.path.with_extension("lock"))?;
        let mut state = self.read()?;
        let result = f(&mut state)?;
        write_json(&self.path, &state)?;
        Ok(result)
    }

    fn exact_root(&self, directory: &Path) -> Result<PathBuf> {
        let root = canonical_directory(directory)?;
        if self.inspect(&root)?.is_none_or(|found| found.root != root) {
            return Err(invalid(
                "directory must be an exact Git or jj checkout root",
            ));
        }
        Ok(root)
    }

    fn validate(&self, checkout: &CheckoutLocation) -> Result<CheckoutLocation> {
        self.exact_root(&checkout.current_root()?)?;
        Ok(checkout.clone())
    }

    fn find_in(&self, state: &State, root: &Path) -> Result<Option<CheckoutLocation>> {
        let identity = physical(&fs::metadata(root)?);
        for checkout in &state.checkouts {
            let same = match fs::metadata(&checkout.directory) {
                Ok(metadata) => physical(&metadata) == identity,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(error.into()),
            };
            if checkout.directory == root || (checkout.device, checkout.inode) == identity || same {
                return self.validate(checkout).map(Some);
            }
        }
        Ok(None)
    }

    /// List saved intent without requiring old roots to remain available.
    pub fn list(&self) -> Result<Vec<CheckoutLocation>> {
        Ok(self.read()?.checkouts)
    }

    /// Revalidate the exact canonical root and its recorded device/inode.
    pub fn get(&self, id: &str) -> Result<CheckoutLocation> {
        self.validate(self.read()?.lookup(id)?)
    }

    /// Find an already registered exact root; aliases never create a second identity.
    pub fn find(&self, directory: &Path) -> Result<Option<CheckoutLocation>> {
        self.find_in(&self.read()?, &self.exact_root(directory)?)
    }

    /// Explicitly register a root or add an exact repository URL binding; never set a default.
    pub fn register(
        &self,
        directory: &Path,
        repository_url: Option<&str>,
    ) -> Result<CheckoutLocation> {
        if let Some(url) = repository_url {
            text(url)?;
        }
        self.update(|state| {
            let root = self.exact_root(directory)?;
            let mut checkout = match self.find_in(state, &root)? {
                Some(checkout) => checkout,
                None => {
                    let metadata = fs::metadata(&root)?;
                    CheckoutLocation {
                        id: Uuid::new_v4().to_string(),
                        directory: root,
                        repository_urls: Vec::new(),
                        device: metadata.dev(),
                        inode: metadata.ino(),
                    }
                }
            };
            if let Some(url) = repository_url
                && !checkout.repository_urls.iter().any(|bound| bound == url)
            {
                checkout.repository_urls.push(url.into());
            }
            if let Some(index) = state
                .checkouts
                .iter()
                .position(|item| item.id == checkout.id)
            {
                state.checkouts[index] = checkout.clone();
            } else {
                state.checkouts.push(checkout.clone());
            }
            Ok(checkout)
        })
    }

    /// Fill absent repository locators from a jj backing checkout or the origin remote.
    /// Existing explicit bindings win; discovery never selects a default or contacts a remote.
    pub fn bind_inferred_repositories(
        &self,
        id: &str,
        runner: &dyn crate::process::ProcessRunner,
    ) -> Result<Vec<String>> {
        let checkout = self.get(id)?;
        if !checkout.repository_urls.is_empty() {
            return Ok(checkout.repository_urls);
        }
        let root = &checkout.directory;
        let mut urls = Vec::new();
        let jj_repo = root.join(".jj/repo");
        if jj_repo.is_file()
            && let Ok(pointer) = fs::read_to_string(&jj_repo)
            && !pointer.trim().is_empty()
            && let Ok(backing) = jj_repo
                .parent()
                .expect("repo has a parent")
                .join(pointer.trim())
                .canonicalize()
            && backing.file_name().is_some_and(|name| name == "repo")
            && let Some(jj) = backing
                .parent()
                .filter(|path| path.file_name().is_some_and(|name| name == ".jj"))
            && let Some(directory) = jj.parent()
            && let Ok(Some(backing)) = self.find(directory)
        {
            urls.extend(backing.repository_urls);
        }
        let push = |urls: &mut Vec<String>, raw: &str| -> Result<()> {
            let url = raw.trim();
            if !url.is_empty() && !urls.iter().any(|bound| bound == url) {
                text(url)?;
                urls.push(url.into());
            }
            Ok(())
        };
        if let Ok(origin) = inspect_command(
            "git",
            &["config", "--get", "remote.origin.url"],
            root,
            runner,
        ) {
            push(&mut urls, &origin)?;
        }
        if urls.is_empty()
            && (jj_repo.is_dir() || jj_repo.is_file())
            && let Ok(remotes) = inspect_command(
                "jj",
                &[
                    "--ignore-working-copy",
                    "--at-operation=@",
                    "git",
                    "remote",
                    "list",
                ],
                root,
                runner,
            )
        {
            for line in remotes.lines() {
                if let Some((name, url)) = line.trim_start().split_once(char::is_whitespace)
                    && name == "origin"
                {
                    push(&mut urls, url)?;
                }
            }
        }
        for url in &urls {
            self.register(root, Some(url))?;
        }
        Ok(urls)
    }

    /// Reassociate an ID with an exact root without moving any files or changing defaults.
    pub fn move_checkout(&self, id: &str, directory: &Path) -> Result<CheckoutLocation> {
        self.update(|state| {
            let mut checkout = state.lookup(id)?.clone();
            let root = self.exact_root(directory)?;
            let metadata = fs::metadata(&root)?;
            for other in state.checkouts.iter().filter(|other| other.id != id) {
                let same = match fs::metadata(&other.directory) {
                    Ok(other_metadata) => physical(&other_metadata) == physical(&metadata),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => return Err(error.into()),
                };
                if other.directory == root
                    || (other.device, other.inode) == physical(&metadata)
                    || same
                {
                    return Err(invalid("destination is already registered"));
                }
            }
            checkout.directory = root;
            checkout.device = metadata.dev();
            checkout.inode = metadata.ino();
            let index = state
                .checkouts
                .iter()
                .position(|item| item.id == id)
                .expect("lookup succeeded");
            state.checkouts[index] = checkout.clone();
            Ok(checkout)
        })
    }

    /// Set a default only for an explicitly bound, currently valid checkout.
    pub fn set_default(&self, repository_url: &str, id: &str) -> Result<()> {
        text(repository_url)?;
        self.update(|state| {
            self.validate(state.lookup(id)?)?;
            state.set_default(repository_url, id)
        })
    }

    /// Prefer an explicitly bound selection; other repositories require explicit defaults.
    /// Even a sole candidate is not an implicit default, and a broken selection never falls back.
    pub fn resolve(
        &self,
        repository_url: &str,
        selected_id: Option<&str>,
        selected_repository_url: Option<&str>,
    ) -> Result<CheckoutLocation> {
        text(repository_url)?;
        if let Some(url) = selected_repository_url {
            text(url)?;
        }
        let state = self.read()?;
        self.validate(state.resolve(repository_url, selected_id, selected_repository_url)?)
    }

    /// Extract a historical document hint; this grants no native-effect authority.
    pub fn revision_document(
        &self,
        id: &str,
        path: &str,
        revision: &str,
        runner: &dyn crate::process::ProcessRunner,
        timeout: f64,
        cache_root: &Path,
    ) -> Result<super::revisions::RevisionDocument> {
        super::revisions::extract(&self.get(id)?, path, revision, runner, timeout, cache_root)
    }

    /// Resolve a relative working directory with strict existence and containment checks.
    pub fn directory(&self, id: &str, relative: &str) -> Result<PathBuf> {
        self.get(id)?.contained(relative, true)
    }

    /// Resolve the current worktree file, not a historical revision or another checkout.
    pub fn document(
        &self,
        repository_url: &str,
        path: &str,
        selected_id: Option<&str>,
        selected_repository_url: Option<&str>,
    ) -> Result<(CheckoutLocation, PathBuf)> {
        let checkout = self.resolve(repository_url, selected_id, selected_repository_url)?;
        let target = checkout.document(path)?;
        Ok((checkout, target))
    }
}

impl CheckoutLocation {
    fn current_root(&self) -> Result<PathBuf> {
        let root = canonical_directory(&self.directory)?;
        if root != self.directory || physical(&fs::metadata(&root)?) != (self.device, self.inode) {
            return Err(invalid(
                "checkout root was replaced; explicitly move the registration",
            ));
        }
        Ok(root)
    }

    fn contained(&self, relative: &str, directory: bool) -> Result<PathBuf> {
        relative_path(relative, directory)?;
        let root = self.current_root()?;
        let target = fs::canonicalize(root.join(relative))?;
        if !target.starts_with(&root) {
            return Err(invalid("path escapes checkout root"));
        }
        let metadata = fs::metadata(&target)?;
        if !(if directory {
            metadata.is_dir()
        } else {
            metadata.is_file()
        }) {
            return Err(invalid("checkout path has the wrong file type"));
        }
        Ok(target)
    }

    /// Resolve a current-file hint within this already-resolved checkout snapshot.
    /// Rechecks filesystem identity and containment, without rereading the registry or VCS.
    /// This is not native action authority; registry bindings and files can change after a read.
    pub fn document(&self, path: &str) -> Result<PathBuf> {
        self.contained(path, false)
    }
}

/// Find the nearest marked Git/jj worktree without snapshotting a jj workspace.
pub fn inspect_directory(cwd: &Path) -> Result<Option<RepositoryInspection>> {
    inspect_directory_with_runner(cwd, &crate::process::NativeProcessRunner)
}

/// Inspect markers with an injected bounded runner, retaining the production inspection policy.
pub fn inspect_directory_with_runner(
    cwd: &Path,
    runner: &dyn crate::process::ProcessRunner,
) -> Result<Option<RepositoryInspection>> {
    let current = canonical_directory(cwd)?;
    for candidate in current.ancestors() {
        let jj = candidate.join(".jj");
        let git = candidate.join(".git");
        if !jj.try_exists()? && !jj.is_symlink() && !git.try_exists()? && !git.is_symlink() {
            continue;
        }
        let (program, args): (&str, &[&str]) = if jj.try_exists()? || jj.is_symlink() {
            if jj.is_symlink() || !jj.is_dir() {
                return Err(invalid("malformed jj marker"));
            }
            (
                "jj",
                &[
                    "--ignore-working-copy",
                    "--at-operation=@",
                    "workspace",
                    "root",
                ],
            )
        } else {
            if git.is_symlink() || !(git.is_dir() || git.is_file()) {
                return Err(invalid("malformed Git marker"));
            }
            (
                "git",
                &["rev-parse", "--path-format=absolute", "--show-toplevel"],
            )
        };
        let output = inspect_command(program, args, candidate, runner)?;
        let output = output.trim_end_matches(['\r', '\n']);
        let output_path = Path::new(output);
        if !output_path.is_absolute() {
            return Err(invalid("VCS root must be absolute"));
        }
        let root = canonical_directory(output_path)?;
        if root != candidate {
            return Err(invalid(
                "VCS marker does not describe its exact checkout root",
            ));
        }
        let relative = current
            .strip_prefix(&root)
            .map_err(|e| invalid(e.to_string()))?;
        return Ok(Some(RepositoryInspection {
            root: root.clone(),
            relative_cwd: if relative.as_os_str().is_empty() {
                ".".into()
            } else {
                relative
                    .to_str()
                    .ok_or_else(|| invalid("cwd must be UTF-8"))?
                    .into()
            },
        }));
    }
    Ok(None)
}

fn inspect_command(
    program: &str,
    args: &[&str],
    cwd: &Path,
    runner: &dyn crate::process::ProcessRunner,
) -> Result<String> {
    use crate::process::ProcessRequest;
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
        timeout: 15.0,
    };
    let output = runner
        .run(&request)
        .map_err(|error| RuntimeError::Inspection(error.to_string()))?;
    String::from_utf8(output.stdout).map_err(|error| RuntimeError::Inspection(error.to_string()))
}
