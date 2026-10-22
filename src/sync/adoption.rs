//! Explicit snapshot import; preparation is disposable, installation is durably fenced.

mod completion;
mod source;

use source::Source;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::config::{Marker, Strategy};
use crate::file_io::{ensure_directory, sync_directory};
use crate::model::check_text;
use crate::storage::Transaction;

use super::baseline::{Baseline, decode_baseline, encode_baseline};
use super::git::{GitCatalog, LockedCatalog};
use super::git_io::Git;
use super::history::read_history;
use super::protocol::ProtocolMachine;
use super::state::{GitConfig, RepairRequired};

fn repair(error: impl std::fmt::Display) -> RepairRequired {
    RepairRequired::new(error.to_string())
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct SigningProfile {
    author: String,
    committer: String,
    policy: BTreeMap<String, String>,
}

fn signing_profile(git: &Git) -> Result<SigningProfile, RepairRequired> {
    let raw = git
        .run(
            &[
                "config",
                "--null",
                "--get-regexp",
                r"^(user\.|author\.|committer\.|commit\.gpgsign$|gpg\.)",
            ],
            None,
            None,
            &[0, 1],
        )
        .map_err(repair)?;
    let raw = std::str::from_utf8(&raw).map_err(repair)?;
    let mut policy = BTreeMap::new();
    for entry in raw.split('\0').filter(|s| !s.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        policy.insert(key.into(), value.into());
    }
    let identity = |name: &str| -> Result<String, RepairRequired> {
        let value = git.text(&["var", name], None, None, &[0]).map_err(repair)?;
        value
            .rsplitn(3, ' ')
            .nth(2)
            .map(str::to_owned)
            .ok_or_else(|| repair("invalid Git identity"))
    };
    Ok(SigningProfile {
        author: identity("GIT_AUTHOR_IDENT")?,
        committer: identity("GIT_COMMITTER_IDENT")?,
        policy,
    })
}

fn sync_tree(root: &Path) -> Result<(), RepairRequired> {
    let metadata = fs::symlink_metadata(root).map_err(repair)?;
    if metadata.file_type().is_symlink() {
        return Err(repair("prepared Git machinery contains a symlink"));
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(root).map_err(repair)? {
            sync_tree(&entry.map_err(repair)?.path())?;
        }
        sync_directory(root).map_err(repair)
    } else if metadata.is_file() {
        File::open(root)
            .and_then(|file| file.sync_all())
            .map_err(repair)
    } else {
        Err(repair("prepared Git machinery contains a nonregular file"))
    }
}

/// Constructed only after preparation and the locked recheck. The permit consumes
/// this evidence, so installation cannot substitute a catalog, snapshot or policy.
pub(crate) struct PreparedImport<'a> {
    catalog: &'a LockedCatalog<'a>,
    candidate: TempDir,
    branch: String,
    source: Source,
    baseline: Baseline,
    profile: SigningProfile,
    tip: String,
}

fn prepare<'a>(
    catalog: &'a LockedCatalog<'a>,
    branch: &str,
) -> Result<PreparedImport<'a>, RepairRequired> {
    let source = Source::capture(catalog.root, catalog.transaction())?;
    let creations = catalog
        .state
        .read("local.json")?
        .map(|data| serde_json::from_value(data.get("creations").cloned().unwrap_or(Value::Null)))
        .transpose()
        .map_err(repair)?
        .unwrap_or_default();
    let baseline =
        Baseline::new(catalog.host_id, creations, catalog.state.fingerprints()?).map_err(repair)?;
    let message = encode_baseline(&baseline).map_err(repair)?;
    decode_baseline(&message, &source.files).map_err(repair)?;
    let staging = catalog.root.join(".catalog-staging");
    ensure_directory(&staging, true).map_err(repair)?;
    let candidate = tempfile::Builder::new()
        .prefix("adoption-")
        .tempdir_in(&staging)
        .map_err(repair)?;
    let git = catalog.git.fresh_root(candidate.path());
    git.run(
        &["init", &format!("--initial-branch={branch}")],
        None,
        None,
        &[0],
    )
    .map_err(repair)?;
    git.run(
        &["check-ref-format", &format!("refs/heads/{branch}")],
        None,
        None,
        &[0],
    )
    .map_err(repair)?;
    if !git
        .text(&["remote"], None, None, &[0])
        .map_err(repair)?
        .is_empty()
    {
        return Err(repair("local Git preparation has an unexpected remote"));
    }
    git.run(&["read-tree", "--empty"], None, None, &[0])
        .map_err(repair)?;
    let mut entries = Vec::new();
    for (path, bytes) in &source.files {
        let blob = git
            .text(&["hash-object", "-w", "--stdin"], Some(bytes), None, &[0])
            .map_err(repair)?;
        entries.extend_from_slice(format!("100644 {blob}\t{path}\0").as_bytes());
    }
    git.run(
        &["update-index", "-z", "--index-info"],
        Some(&entries),
        None,
        &[0],
    )
    .map_err(repair)?;
    let profile = signing_profile(&git)?;
    let tree = git
        .text(&["write-tree"], None, None, &[0])
        .map_err(repair)?;
    let tip = git.commit(&tree, &message, None).map_err(repair)?;
    git.run(
        &["update-ref", &format!("refs/heads/{branch}"), &tip],
        None,
        None,
        &[0],
    )
    .map_err(repair)?;
    let history = read_history(&git, Some(&tip))?;
    if history.files() != &source.files
        || history.baseline() != Some(&baseline)
        || signing_profile(&git)? != profile
    {
        return Err(repair("prepared Git history or signing policy changed"));
    }
    sync_tree(&candidate.path().join(".git"))?;
    sync_directory(candidate.path()).map_err(repair)?;
    sync_directory(&staging).map_err(repair)?;
    sync_directory(catalog.root).map_err(repair)?;
    source.check_unchanged(catalog.root, catalog.transaction())?;
    Ok(PreparedImport {
        catalog,
        candidate,
        branch: branch.into(),
        source,
        baseline,
        profile,
        tip,
    })
}

impl PreparedImport<'_> {
    pub(crate) fn install(self) -> Result<String, RepairRequired> {
        let Self {
            catalog,
            candidate,
            branch,
            source,
            baseline,
            profile,
            tip,
        } = self;
        let digests: BTreeMap<_, _> = source
            .files
            .iter()
            .map(|(path, bytes)| (path, format!("{:x}", Sha256::digest(bytes))))
            .collect();
        let evidence: Map<String, Value> = json!({"adoption": {
            "baseline":tip,"branch":branch,"host":catalog.host_id,
            "before":source.bookkeeping,"profile":profile,"files":digests,
        }})
        .as_object()
        .expect("object")
        .clone();
        // A failed write can already have replaced save.json. Disable Drop cleanup
        // BEFORE that first possible marker effect, not after write returns success.
        let retained = candidate.keep();
        catalog.state.write("save.json", evidence.clone())?;
        fs::rename(retained.join(".git"), catalog.root.join(".git")).map_err(repair)?;
        sync_directory(catalog.root).map_err(repair)?;
        sync_directory(&retained).map_err(repair)?;
        if signing_profile(catalog.git)? != profile {
            return Err(repair(
                "Git identity or signing policy differs at the final catalog path; inspect save.json",
            ));
        }
        catalog.clean().map_err(repair)?;
        let installed = read_history(catalog.git, Some(&catalog.git.head().map_err(repair)?))?;
        if installed.tip() != Some(tip.as_str())
            || installed.files() != &source.files
            || installed.baseline() != Some(&baseline)
            || installed.catalog() != &source.snapshot.catalog
        {
            return Err(repair(
                "installed Git baseline differs from the prepared catalog",
            ));
        }
        sync_tree(&catalog.root.join(".git"))?;
        let config = GitConfig::new(None, branch, catalog.host_id).map_err(repair)?;
        completion::finish(
            &catalog.state,
            &config,
            evidence,
            retained.display(),
            || catalog.config(),
        )?;
        // Only our now-empty directory is eligible for cleanup after completion.
        let _ = fs::remove_dir(&retained);
        Ok(tip)
    }
}

/// Caller holds the config lock, binding this transition to its existing host identity.
/// A post-fence error leaves opaque save evidence and requires explicit repair.
pub fn adopt_local(catalog: &GitCatalog, branch: &str) -> Result<String, RepairRequired> {
    check_text(branch, "branch", false).map_err(repair)?;
    catalog
        .with_lock(|catalog| {
            let result = (|| -> Result<_, RepairRequired> {
                catalog.state.check_save()?;
                let marker = Marker::read(catalog.transaction().journal())
                    .map_err(repair)?
                    .ok_or_else(|| repair("adoption requires an initialized local-only catalog"))?;
                if marker.host.as_str() != catalog.host_id {
                    return Err(repair("adoption requires this host's initialized catalog"));
                }
                if marker.strategy == Strategy::Git {
                    let expected = GitConfig::new(None, branch, catalog.host_id).map_err(repair)?;
                    return completion::retry(&catalog.ready().map_err(repair)?, &expected, || {
                        catalog.saved().map(|(history, _)| history).map_err(repair)
                    });
                }
                let prepared = prepare(catalog, branch)?;
                let mut machine =
                    ProtocolMachine::observe(&catalog.state, catalog.policy.attempts())?;
                machine.begin_import(prepared)?.run()
            })();
            result.map_err(Into::into)
        })
        .map_err(repair)
}
