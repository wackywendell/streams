//! Explicit snapshot import; preparation is disposable, installation is durably fenced.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::config::{Marker, Strategy};
use crate::model::check_text;
use crate::storage::{Snapshot, Store};
use crate::yaml_store::{YamlTransaction, ensure_directory, sync_directory};

use super::baseline::{Baseline, decode_baseline, encode_baseline};
use super::git::GitCatalog;
use super::git_io::Git;
use super::history::{read_history, records};
use super::local::LocalCatalog;
use super::protocol::ProtocolMachine;
use super::state::{GitConfig, RepairRequired};

fn repair(error: impl std::fmt::Display) -> RepairRequired {
    RepairRequired::new(error.to_string())
}

type Files = BTreeMap<String, Vec<u8>>;
type Bookkeeping = BTreeMap<String, Option<String>>;

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

fn bookkeeping(catalog: &GitCatalog) -> Result<Bookkeeping, RepairRequired> {
    let mut before = BTreeMap::new();
    for name in ["cli.json", "local.json", "receipts.json"] {
        // Validate the journal object before retaining its exact original text.
        let present = catalog.state.read(name)?.is_some();
        before.insert(
            name.into(),
            if present {
                Some(
                    fs::read_to_string(catalog.root.join(".catalog-sync").join(name))
                        .map_err(repair)?,
                )
            } else {
                None
            },
        );
    }
    Ok(before)
}

fn local_files(catalog: &GitCatalog, snapshot: &Snapshot) -> Result<Files, RepairRequired> {
    let mut files = BTreeMap::new();
    for record in snapshot.catalog.records() {
        let path = catalog.store.record_path(&record);
        files.insert(
            path.to_str()
                .ok_or_else(|| repair("record path must be UTF-8"))?
                .into(),
            fs::read(catalog.root.join(path)).map_err(repair)?,
        );
    }
    Ok(files)
}

/// Constructed only after preparation and the locked recheck. The permit consumes
/// this evidence, so installation cannot substitute a catalog, snapshot or policy.
pub(crate) struct PreparedImport<'a> {
    catalog: &'a GitCatalog,
    candidate: TempDir,
    branch: String,
    snapshot: Snapshot,
    files: Files,
    baseline: Baseline,
    before: Bookkeeping,
    profile: SigningProfile,
    tip: String,
}

fn prepare<'a>(
    catalog: &'a GitCatalog,
    tx: &YamlTransaction,
    branch: &str,
) -> Result<PreparedImport<'a>, RepairRequired> {
    let local = LocalCatalog::new(catalog.store.clone(), &catalog.host_id).map_err(repair)?;
    local.ready()?;
    let snapshot = tx.read().map_err(repair)?;
    let files = local_files(catalog, &snapshot)?;
    let before = bookkeeping(catalog)?;
    let creations = catalog
        .state
        .read("local.json")?
        .map(|data| serde_json::from_value(data.get("creations").cloned().unwrap_or(Value::Null)))
        .transpose()
        .map_err(repair)?
        .unwrap_or_default();
    let baseline = Baseline::new(&catalog.host_id, creations, catalog.state.fingerprints()?)
        .map_err(repair)?;
    let message = encode_baseline(&baseline).map_err(repair)?;
    decode_baseline(&message, &files).map_err(repair)?;
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
    for (path, bytes) in &files {
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
    if history.files != files
        || history.baseline.as_ref() != Some(&baseline)
        || signing_profile(&git)? != profile
    {
        return Err(repair("prepared Git history or signing policy changed"));
    }
    sync_tree(&candidate.path().join(".git"))?;
    sync_directory(candidate.path()).map_err(repair)?;
    sync_directory(&staging).map_err(repair)?;
    sync_directory(&catalog.root).map_err(repair)?;
    local.ready()?;
    if tx.read().map_err(repair)? != snapshot || local_files(catalog, &snapshot)? != files {
        return Err(repair(
            "catalog records changed during adoption preparation",
        ));
    }
    if bookkeeping(catalog)? != before {
        return Err(repair(
            "catalog bookkeeping changed during adoption preparation",
        ));
    }
    Ok(PreparedImport {
        catalog,
        candidate,
        branch: branch.into(),
        snapshot,
        files,
        baseline,
        before,
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
            snapshot,
            files,
            baseline,
            before,
            profile,
            tip,
        } = self;
        let digests: BTreeMap<_, _> = files
            .iter()
            .map(|(path, bytes)| (path, format!("{:x}", Sha256::digest(bytes))))
            .collect();
        let evidence: Map<String, Value> = json!({"adoption": {
            "baseline":tip,"branch":branch,"host":catalog.host_id,
            "before":before,"profile":profile,"files":digests,
        }})
        .as_object()
        .expect("object")
        .clone();
        // A failed write can already have replaced save.json. Disable Drop cleanup
        // BEFORE that first possible marker effect, not after write returns success.
        let retained = candidate.keep();
        catalog.state.write("save.json", evidence.clone())?;
        fs::rename(retained.join(".git"), catalog.root.join(".git")).map_err(repair)?;
        sync_directory(&catalog.root).map_err(repair)?;
        sync_directory(&retained).map_err(repair)?;
        if signing_profile(&catalog.git)? != profile {
            return Err(repair(
                "Git identity or signing policy differs at the final catalog path; inspect save.json",
            ));
        }
        catalog.clean().map_err(repair)?;
        let installed = read_history(&catalog.git, Some(&catalog.git.head().map_err(repair)?))?;
        if installed.tip.as_ref() != Some(&tip)
            || installed.files != files
            || installed.baseline.as_ref() != Some(&baseline)
            || records(&installed.catalog) != records(&snapshot.catalog)
        {
            return Err(repair(
                "installed Git baseline differs from the prepared catalog",
            ));
        }
        sync_tree(&catalog.root.join(".git"))?;
        let config = GitConfig::new(None, branch, &catalog.host_id).map_err(repair)?;
        catalog.state.write_config(&config)?;
        if catalog.config()? != config {
            return Err(repair("installed Git configuration changed"));
        }
        catalog.state.write(
            "cli.json",
            json!({"strategy":"git","host":catalog.host_id})
                .as_object()
                .expect("object")
                .clone(),
        )?;
        if let Err(error) = catalog.state.clear("save.json") {
            // Unlink may have happened before fsync failed. Reassert the ORIGINAL
            // evidence; neither this fence nor the retained candidate is a rollback.
            if let Err(reassert) = catalog.state.write("save.json", evidence) {
                return Err(repair(format!(
                    "{error}; cannot reassert import fence: {reassert}; inspect retained staging at {}",
                    retained.display()
                )));
            }
            return Err(error);
        }
        // Only our now-empty directory is eligible for cleanup after completion.
        let _ = fs::remove_dir(&retained);
        Ok(tip)
    }
}

/// Caller holds the config lock, binding this transition to its existing host identity.
/// A post-fence error leaves opaque save evidence and requires explicit repair.
pub fn adopt_local(catalog: &GitCatalog, branch: &str) -> Result<String, RepairRequired> {
    check_text(branch, "branch", false).map_err(repair)?;
    catalog.store.with_transaction(|tx| Ok((|| {
        catalog.state.check_save()?;
        let marker = Marker::read(&catalog.store.journal()).map_err(repair)?
            .ok_or_else(|| repair("adoption requires an initialized local-only catalog"))?;
        if marker.host.as_str() != catalog.host_id {
            return Err(repair("adoption requires this host's initialized catalog"));
        }
        if marker.strategy == Strategy::Git {
            let expected = GitConfig::new(None, branch, &catalog.host_id).map_err(repair)?;
            if catalog.ready().map_err(repair)? != expected {
                return Err(repair("completed adoption does not match this host, branch or local-only mode"));
            }
            let (history, _) = catalog.saved(tx).map_err(repair)?;
            if history.baseline.as_ref().is_none_or(|baseline| baseline.host != catalog.host_id) {
                return Err(repair("existing Git catalog is not a completed snapshot adoption"));
            }
            return history.root.ok_or_else(|| repair("adopted history has no baseline root"));
        }
        let prepared = prepare(catalog, tx, branch)?;
        let mut machine = ProtocolMachine::observe(&catalog.state, catalog.policy.attempts)?;
        machine.begin_import(prepared)?.run()
    })()))
    .map_err(repair)?
}
