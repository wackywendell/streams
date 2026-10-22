use super::*;
use crate::storage::{Journal, StorageError, Store, Transaction};
use serde_json::json;
use std::cell::Cell;
use std::sync::{Arc, Mutex};

const HOST: &str = "00000000-0000-4000-8000-000000000001";

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
