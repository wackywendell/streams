use crate::support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use streams_temper::storage::{StorageError, Transaction};

use serde_json::json;
use streams_temper::{
    application,
    config::{Config, Strategy},
    model::{Record, Stream},
    operations::{Actor, DomainOutcome, Operation, update},
    sync::{
        adoption::adopt_local,
        git::GitCatalog,
        git_io::{Git, GitError},
        history::read_history,
        local::LocalCatalog,
        policy::Policy,
        state::{GitConfig, RepairRequired, State},
    },
    yaml_store::YamlStore,
};
use support::git::test_command;

// Ignore workstation init templates in these fixtures; production deliberately
// rejects template-created symlinks instead of weakening its graph validation.
fn install_hook(
    catalog: &mut GitCatalog,
    mut hook: impl FnMut(
        &Git,
        &[&str],
        Option<&[u8]>,
        Option<&Path>,
    ) -> Option<Result<Vec<u8>, GitError>>
    + Send
    + 'static,
) {
    support::git::install_hook(catalog, move |git, args, input, index| {
        hook(git, args, input, index).or_else(|| {
            if args[0] == "init" {
                let mut isolated = args.to_vec();
                isolated.push("--template=");
                Some(test_command(git, &isolated, input, index))
            } else {
                None
            }
        })
    });
}

struct Fixture {
    _temp: tempfile::TempDir,
    config: Config,
    root: PathBuf,
    git: GitCatalog,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let environment = BTreeMap::from([
            ("HOME".into(), temp.path().to_str().unwrap().into()),
            (
                "XDG_CONFIG_HOME".into(),
                temp.path().join("config").to_str().unwrap().into(),
            ),
            (
                "XDG_DATA_HOME".into(),
                temp.path().join("data").to_str().unwrap().into(),
            ),
        ]);
        let config = Config::new(&environment, temp.path()).unwrap();
        let opened = application::initialize(&config, Strategy::None, None, "main", false).unwrap();
        let root = opened.root().to_path_buf();
        let host = opened.host().as_str();
        let mut git = GitCatalog::new(&root, host, Policy::default()).unwrap();
        install_hook(&mut git, |_, _, _, _| None);
        Self {
            _temp: temp,
            config,
            root,
            git,
        }
    }
    fn original_history(&self) -> (Operation, Operation) {
        let local = LocalCatalog::new(YamlStore::new(&self.root), self.git.host_id()).unwrap();
        let create = Operation::new(
            "original-create",
            None,
            Record::Stream(Stream::new("work", "Original", None, self.git.host_id()).unwrap()),
            Actor::User,
            None,
            vec![],
        )
        .unwrap();
        assert!(local.save(&create, None).unwrap().accepted());
        let edit = update(
            "retired-edit",
            create.after(),
            json!({"title":"Edited"}).as_object().unwrap(),
            Actor::User,
            None,
            vec![],
        )
        .unwrap();
        assert!(local.save(&edit, None).unwrap().accepted());
        // Noncanonical formatting is a physical revision and must survive import.
        let path = self.root.join("streams/work.yaml");
        let mut bytes = fs::read(&path).unwrap();
        bytes.extend_from_slice(b"\n# exact local formatting\n");
        fs::write(path, bytes).unwrap();
        (create, edit)
    }
    fn adopt(&self) -> Result<String, RepairRequired> {
        let _lock = self.config.lock().unwrap();
        adopt_local(&self.git, "main")
    }
    fn candidates(&self) -> Vec<PathBuf> {
        fs::read_dir(self.root.join(".catalog-staging"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("adoption-")
            })
            .collect()
    }
    fn fenced(&self) {
        assert!(self.root.join(".catalog-sync/save.json").is_file());
        assert!(self.git.snapshot().is_err());
        assert!(application::open(&self.config).is_err());
        assert!(self.adopt().is_err());
        assert_eq!(self.candidates().len(), 1);
    }
}

#[test]
fn baseline_preserves_exact_bytes_authority_and_retired_retry_fingerprints() {
    let f = Fixture::new();
    let (create, edit) = f.original_history();
    let before = application::open(&f.config)
        .unwrap()
        .backend()
        .snapshot()
        .unwrap();
    let raw = fs::read(f.root.join("streams/work.yaml")).unwrap();
    let receipts = fs::read(f.root.join(".catalog-sync/receipts.json")).unwrap();
    let local = fs::read(f.root.join(".catalog-sync/local.json")).unwrap();
    let tip = f.adopt().unwrap();
    assert_eq!(f.git.snapshot().unwrap(), before);
    let history = read_history(f.git.git(), Some(&tip)).unwrap();
    assert_eq!(history.files()["streams/work.yaml"], raw);
    assert_eq!(history.operations().len(), 0);
    assert!(history.has_operation(create.id()));
    assert!(history.has_operation(edit.id()));
    assert_eq!(history.baseline().unwrap().host, f.git.host_id());
    let authority = f.git.authority("work").unwrap().unwrap();
    assert_eq!(authority.host_id, f.git.host_id());
    assert_eq!(authority.token, create.id());
    for operation in [&create, &edit] {
        let retry = f.git.save(operation, None).unwrap();
        assert!(retry.accepted());
        assert_eq!(retry.domain.unwrap().outcome(), DomainOutcome::NoChange);
    }
    assert_eq!(f.git.snapshot().unwrap(), before);
    assert_eq!(
        fs::read(f.root.join(".catalog-sync/receipts.json")).unwrap(),
        receipts
    );
    assert_eq!(
        fs::read(f.root.join(".catalog-sync/local.json")).unwrap(),
        local
    );
    assert_eq!(f.adopt().unwrap(), tip);
    let later = update(
        "post-baseline-edit",
        edit.after(),
        json!({"goal":"After the baseline"}).as_object().unwrap(),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(f.git.save(&later, None).unwrap().accepted());
    assert_ne!(f.git.git().head().unwrap(), tip);
    assert_eq!(f.adopt().unwrap(), tip);
    assert!(f.candidates().is_empty());
    let _lock = f.config.lock().unwrap();
    assert!(adopt_local(&f.git, "different").is_err());
}

#[test]
fn preparation_failure_cleans_only_its_owned_candidate_and_is_retryable() {
    let mut f = Fixture::new();
    let orphan = f.root.join(".catalog-staging/foreign-evidence");
    fs::create_dir_all(&orphan).unwrap();
    fs::write(orphan.join("retain"), "retain").unwrap();
    install_hook(&mut f.git, |_, args, _, _| {
        (args[0] == "commit-tree").then(|| Err(GitError::new("preparation failed", Some(1))))
    });
    assert!(f.adopt().is_err());
    assert!(f.candidates().is_empty());
    assert_eq!(fs::read_to_string(orphan.join("retain")).unwrap(), "retain");
    assert!(!f.root.join(".git").exists());
    assert!(!f.root.join(".catalog-sync/save.json").exists());
    assert!(application::open(&f.config).is_ok());
    install_hook(&mut f.git, |_, _, _, _| None);
    f.adopt().unwrap();
}

#[test]
fn changed_record_bytes_abort_preparation_before_fencing() {
    let mut f = Fixture::new();
    f.original_history();
    let path = f.root.join("streams/work.yaml");
    install_hook(&mut f.git, move |_, args, _, _| {
        if args[0] == "commit-tree" {
            let mut bytes = fs::read(&path).unwrap();
            bytes.push(b'\n');
            fs::write(&path, bytes).unwrap();
        }
        None
    });
    assert_eq!(
        f.adopt().unwrap_err().to_string(),
        "catalog records changed during adoption preparation"
    );
    assert!(f.candidates().is_empty());
    assert!(!f.root.join(".catalog-sync/save.json").exists());
    assert!(!f.root.join(".git").exists());
    assert!(application::open(&f.config).is_ok());
}

#[test]
fn first_possible_fence_effect_retains_prepared_evidence_even_if_write_fails() {
    let mut f = Fixture::new();
    f.git
        .set_state_hook(Arc::new(Mutex::new(|op: &str, name: &str| {
            if op == "write" && name == "save.json" {
                Err(RepairRequired::new("fence failure"))
            } else {
                Ok(())
            }
        })));
    assert!(f.adopt().is_err());
    let candidates = f.candidates();
    assert_eq!(candidates.len(), 1);
    assert!(candidates[0].join(".git").is_dir());
    assert!(!f.root.join(".git").exists());
    assert!(!f.root.join(".catalog-sync/save.json").exists());
    // The pre-effect injected failure is still readable as local. A real write
    // failure may have installed its fence, hence conservative candidate retention.
    assert!(application::open(&f.config).is_ok());
}

#[test]
fn install_failure_keeps_both_fence_and_uninstalled_graph() {
    let mut f = Fixture::new();
    let git_dir = f.root.join(".git");
    f.git
        .set_state_hook(Arc::new(Mutex::new(move |op: &str, name: &str| {
            if op == "write" && name == "save.json" {
                fs::create_dir(&git_dir).unwrap();
                fs::write(git_dir.join("block-install"), "retain").unwrap();
            }
            Ok(())
        })));
    assert!(f.adopt().is_err());
    f.fenced();
    assert!(f.candidates()[0].join(".git").is_dir());
}

#[test]
fn final_path_signing_profile_must_match_prepared_identity_and_policy() {
    let mut f = Fixture::new();
    let root = f.root.clone();
    install_hook(&mut f.git, move |git, args, input, index| {
        if git.root() == root && args == ["var", "GIT_AUTHOR_IDENT"] {
            let mut value = test_command(git, args, input, index).unwrap();
            value.splice(0..0, b"Changed ".iter().copied());
            Some(Ok(value))
        } else {
            None
        }
    });
    assert!(f.adopt().is_err());
    f.fenced();
    assert!(f.root.join(".git").is_dir());
    assert!(!f.root.join(".catalog-sync/config.json").exists());
}

#[test]
fn completed_retry_rejects_nonadoption_history_and_remote_configuration() {
    let f = Fixture::new();
    f.adopt().unwrap();
    let store = YamlStore::new(&f.root);
    store
        .with_transaction::<_, StorageError>(|tx| {
            State::from_journal(tx.journal())
                .write_config(
                    &GitConfig::new(Some("remote".into()), "main", f.git.host_id()).unwrap(),
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(f.adopt().is_err());
    let dir = tempfile::tempdir().unwrap();
    let mut git = GitCatalog::new(dir.path(), f.git.host_id(), Policy::default()).unwrap();
    install_hook(&mut git, |_, _, _, _| None);
    let git = git.initialize(None, "main").unwrap();
    let store = YamlStore::new(dir.path());
    store
        .with_transaction::<_, StorageError>(|tx| {
            State::from_journal(tx.journal())
                .write(
                    "cli.json",
                    json!({"host":git.host_id(),"strategy":"git"})
                        .as_object()
                        .unwrap()
                        .clone(),
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(adopt_local(&git, "main").is_err());
}

#[test]
fn opaque_import_fence_is_not_blindly_completed_by_reopen() {
    let f = Fixture::new();
    let store = YamlStore::new(&f.root);
    store
        .with_transaction::<_, StorageError>(|tx| {
            tx.journal().write(
                "save.json",
                json!({"version":1,"adoption":{"baseline":"opaque"}})
                    .as_object()
                    .unwrap()
                    .clone(),
            )?;
            Ok(())
        })
        .unwrap();
    let local = LocalCatalog::new(store, f.git.host_id()).unwrap();
    assert!(local.snapshot().is_err());
    assert!(f.adopt().is_err());
    assert!(!f.root.join(".git").exists());
}
