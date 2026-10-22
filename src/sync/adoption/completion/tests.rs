use super::*;
use crate::storage::{Journal, StorageError, Store, Transaction};
use serde_json::json;
use std::cell::Cell;
use std::sync::{Arc, Mutex};

const HOST: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";

fn baseline_history(host: &str) -> History {
    use crate::sync::baseline::{Baseline, encode_baseline};
    let baseline = Baseline::new(host, Default::default(), Default::default()).unwrap();
    History::default()
        .append(
            &"a".repeat(40),
            &[],
            &encode_baseline(&baseline).unwrap(),
            Default::default(),
        )
        .unwrap()
}

#[test]
fn completed_retry_checks_config_before_reading_history() {
    let expected = GitConfig::new(None, "main", HOST).unwrap();
    for actual in [
        GitConfig::new(Some("remote".into()), "main", HOST).unwrap(),
        GitConfig::new(None, "different", HOST).unwrap(),
        GitConfig::new(None, "main", OTHER).unwrap(),
    ] {
        assert_eq!(
            retry(&actual, &expected, || panic!(
                "mismatched config must not read history"
            ))
            .unwrap_err()
            .to_string(),
            "completed adoption does not match this host, branch or local-only mode"
        );
    }
}

#[test]
fn completed_retry_requires_an_adoption_baseline_from_this_host() {
    let config = GitConfig::new(None, "main", HOST).unwrap();
    let bootstrap = History::default()
        .append(
            &"b".repeat(40),
            &[],
            crate::sync::envelope::BOOTSTRAP,
            Default::default(),
        )
        .unwrap();
    for history in [History::default(), bootstrap, baseline_history(OTHER)] {
        assert_eq!(
            retry(&config, &config, || Ok(history))
                .unwrap_err()
                .to_string(),
            "existing Git catalog is not a completed snapshot adoption"
        );
    }
}

#[test]
fn completed_retry_returns_baseline_root_after_later_operations() {
    use crate::sync::envelope::encode_operation;
    use crate::yaml_store::relative_record_path;
    use crate::{
        codec::record_to_data,
        model::{Record, Stream},
        operations::{Actor, Operation},
    };

    let operation = Operation::new(
        "later",
        None,
        Record::Stream(Stream::new("work", "Work", None, HOST).unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    let bytes = serde_json::to_vec(&json!({
        "version":1, "revision":"fixture", "kind":"stream", "record":record_to_data(operation.after())
    })).unwrap();
    let history = baseline_history(HOST)
        .append(
            &"b".repeat(40),
            &[&"a".repeat(40)],
            &encode_operation(&operation, None).unwrap(),
            [(
                relative_record_path(operation.after())
                    .to_str()
                    .unwrap()
                    .into(),
                bytes,
            )]
            .into(),
        )
        .unwrap();
    assert_ne!(history.tip(), history.root());
    let config = GitConfig::new(None, "main", HOST).unwrap();
    assert_eq!(
        retry(&config, &config, || Ok(history)).unwrap(),
        "a".repeat(40)
    );
}

use crate::memory;

fn with_pending_import(test: impl FnOnce(&dyn Journal, Map<String, Value>)) {
    memory::MemoryStore::default()
        .with_transaction(|tx| -> Result<(), StorageError> {
            let state = State::from_journal(tx.journal());
            Marker {
                strategy: Strategy::None,
                host: HostIdentity::parse(HOST).unwrap(),
            }
            .write(&state)
            .unwrap();
            let evidence =
                json!({"adoption":{"baseline":"prepared-tip","files":{"work.yaml":"digest"}}})
                    .as_object()
                    .unwrap()
                    .clone();
            state.write("save.json", evidence.clone()).unwrap();
            test(tx.journal(), evidence);
            Ok(())
        })
        .unwrap();
}

#[test]
fn config_marker_and_clear_failures_never_erase_repair_fence() {
    for (op, name) in [
        ("write", "config.json"),
        ("write", "cli.json"),
        ("clear", "save.json"),
    ] {
        with_pending_import(|journal, evidence| {
            let original = State::from_journal(journal).read("save.json").unwrap();
            let state = State::from_journal(journal).with_hook(Arc::new(Mutex::new(
                move |actual: &str, key: &str| {
                    if actual == op && key == name {
                        Err(repair("finalization failure"))
                    } else {
                        Ok(())
                    }
                },
            )));
            let config = GitConfig::new(None, "main", HOST).unwrap();
            let observations = Cell::new(0);
            let result = finish(&state, &config, evidence, "retained-staging", || {
                observations.set(observations.get() + 1);
                Ok(state.config()?.unwrap())
            });
            assert!(result.is_err(), "{op}/{name}");
            assert_eq!(state.read("save.json").unwrap(), original);
            assert!(state.check_save().is_err());
            assert_eq!(state.config().unwrap().is_some(), name != "config.json");
            assert_eq!(observations.get(), usize::from(name != "config.json"));
            assert_eq!(
                Marker::read(journal).unwrap().unwrap().strategy,
                if name == "save.json" {
                    Strategy::Git
                } else {
                    Strategy::None
                }
            );
        });
    }
}

#[test]
fn physical_unlink_failure_reasserts_original_evidence_for_a_fresh_store() {
    use crate::yaml_store::YamlStore;
    use std::fs;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(".catalog-sync/save.json");
    let config = GitConfig::new(None, "main", HOST).unwrap();
    let original = YamlStore::new(dir.path())
        .with_transaction(|tx| -> Result<_, StorageError> {
            let state = State::from_journal(tx.journal());
            let evidence =
                json!({"adoption":{"baseline":"prepared-tip","files":{"work.yaml":"digest"}}})
                    .as_object()
                    .unwrap()
                    .clone();
            state.write("save.json", evidence.clone()).unwrap();
            Marker {
                strategy: Strategy::None,
                host: HostIdentity::parse(HOST).unwrap(),
            }
            .write(&state)
            .unwrap();
            let original = fs::read(&path).unwrap();
            let clear_path = path.clone();
            let state = state.with_hook(Arc::new(Mutex::new(move |op: &str, name: &str| {
                if op == "clear" && name == "save.json" {
                    fs::remove_file(&clear_path).unwrap();
                    assert!(!clear_path.exists());
                    return Err(repair("directory fsync failed after unlink"));
                }
                Ok(())
            })));
            let error = finish(&state, &config, evidence, "retained-import", || {
                Ok(state.config()?.unwrap())
            })
            .unwrap_err();
            assert_eq!(error.to_string(), "directory fsync failed after unlink");
            assert_eq!(fs::read(&path).unwrap(), original);
            Ok(original)
        })
        .unwrap();
    YamlStore::new(dir.path())
        .with_transaction(|tx| -> Result<(), StorageError> {
            let state = State::from_journal(tx.journal());
            assert_eq!(fs::read(&path).unwrap(), original);
            assert!(state.check_save().is_err());
            assert_eq!(state.config().unwrap(), Some(config));
            assert_eq!(
                Marker::read(tx.journal()).unwrap().unwrap().strategy,
                Strategy::Git
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn changed_configuration_keeps_import_fenced_and_local_strategy() {
    with_pending_import(|journal, evidence| {
        let state = State::from_journal(journal);
        let original = state.read("save.json").unwrap();
        let config = GitConfig::new(None, "main", HOST).unwrap();
        assert!(
            finish(&state, &config, evidence, "retained-staging", || {
                Ok(GitConfig::new(None, "changed", HOST).unwrap())
            })
            .unwrap_err()
            .to_string()
            .contains("configuration changed")
        );
        assert_eq!(state.read("save.json").unwrap(), original);
        assert_eq!(
            Marker::read(journal).unwrap().unwrap().strategy,
            Strategy::None
        );
    });
}

#[test]
fn successful_completion_installs_configuration_and_strategy_before_unfencing() {
    with_pending_import(|journal, evidence| {
        let state = State::from_journal(journal);
        let config = GitConfig::new(None, "main", HOST).unwrap();
        finish(&state, &config, evidence, "retained-staging", || {
            assert_eq!(state.config().unwrap(), Some(config.clone()));
            assert_eq!(
                Marker::read(journal).unwrap().unwrap().strategy,
                Strategy::None
            );
            Ok(config.clone())
        })
        .unwrap();
        assert!(state.read("save.json").unwrap().is_none());
        assert_eq!(state.config().unwrap(), Some(config));
        assert_eq!(
            Marker::read(journal).unwrap().unwrap().strategy,
            Strategy::Git
        );
    });
}
