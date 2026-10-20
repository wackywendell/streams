use std::sync::{Arc, Mutex};
use streams_temper::operations::Actor;

use proptest::prelude::*;
use streams_temper::{
    model::{Record, Stream},
    operations::Operation,
    sync::{local::LocalCatalog, outcomes::LocalStatus, state::RepairRequired},
    yaml_store::YamlStore,
};

fn operation(id: &str) -> Operation {
    Operation::new(
        id,
        None,
        Record::Stream(Stream::new(id, "Work", None, "host").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}

#[test]
fn every_bookkeeping_failure_preserves_a_safe_restart() {
    for (op, name) in [
        ("write", "save.json"),
        ("write", "local.json"),
        ("remember", "create"),
        ("write", "receipts.json"),
        ("clear", "save.json"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let mut local = LocalCatalog::new(YamlStore::new(dir.path()), "host").unwrap();
        local.set_state_hook(Arc::new(Mutex::new(
            move |actual_op: &str, actual_name: &str| {
                if actual_op == op && actual_name == name {
                    Err(RepairRequired::new("injected state failure"))
                } else {
                    Ok(())
                }
            },
        )));
        let create = operation("create");
        let outcome = local.save(&create, None).unwrap();
        assert!(outcome.requires_repair(), "{op}/{name}: {outcome:?}");
        let restarted = LocalCatalog::new(YamlStore::new(dir.path()), "host").unwrap();
        if dir.path().join(".catalog-sync/save.json").exists() {
            assert!(restarted.snapshot().is_err(), "{op}/{name}");
            assert!(restarted.save(&create, None).unwrap().requires_repair());
        } else {
            // A pre-intent failure performed no external mutation and is retryable.
            assert!(restarted.snapshot().unwrap().catalog.records().is_empty());
            assert!(restarted.save(&create, None).unwrap().accepted());
        }
    }
}

#[test]
fn operations_cannot_be_forged_through_the_wire_boundary() {
    use streams_temper::sync::envelope::{PREFIX, decode_operation, encode_operation};
    let record = operation("create").after().clone();
    let update = Operation::new(
        "edit",
        Some(record.clone()),
        record,
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    let text = encode_operation(&update, None).unwrap();
    let mut wire: serde_json::Value =
        serde_json::from_str(text.strip_prefix(PREFIX).unwrap()).unwrap();
    wire["after"]["owner"]["host_id"] = "other".into();
    assert!(decode_operation(&format!("{PREFIX}{}", wire)).is_err());
    wire["after"]["owner"]["host_id"] = "host".into();
    wire["actor"] = "automatic".into();
    assert!(decode_operation(&format!("{PREFIX}{}", wire)).is_err()); // missing token
    wire["ownership_token"] = "create".into();
    wire["after"]["status"] = "done".into();
    assert!(decode_operation(&format!("{PREFIX}{}", wire)).is_err());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn receipts_survive_reopened_retry_sequences(ids in prop::collection::vec(0u8..8, 1..25)) {
        let dir = tempfile::tempdir().unwrap();
        let mut seen = std::collections::BTreeSet::new();
        for id in ids {
            let id = format!("stream-{id}");
            let backend = LocalCatalog::new(YamlStore::new(dir.path()), "host").unwrap();
            let result = backend.save(&operation(&id), None).unwrap();
            prop_assert!(result.accepted());
            prop_assert_eq!(result.local, if seen.insert(id) { LocalStatus::Persisted } else { LocalStatus::Unchanged });
            prop_assert_eq!(backend.snapshot().unwrap().catalog.streams().len(), seen.len());
        }
    }
}
