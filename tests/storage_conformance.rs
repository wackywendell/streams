#[path = "support/memory.rs"]
mod memory;

use serde_json::json;
use streams_temper::{
    model::{Record, Session, Stream},
    storage::{Journal, StorageError, Store, Transaction},
    sync::state::State,
    yaml_store::YamlStore,
};

fn stream(id: &str, title: &str) -> Record {
    Record::Stream(Stream::new(id, title, None, "host").unwrap())
}

fn storage_laws(store: &impl Store) {
    let empty = store.with_transaction(|tx| tx.read()).unwrap();
    assert!(empty.catalog.records().is_empty());
    assert!(empty.revisions.is_empty());
    let original = stream("work", "Original");
    let first = store
        .with_transaction(|tx| tx.save(&original, &empty))
        .unwrap();
    assert_eq!(first.catalog.get(&original.key()), Some(original.clone()));
    assert_eq!(first.revisions.len(), 1);
    assert!(!first.revisions[&original.key()].is_empty());

    let unchanged = store
        .with_transaction(|tx| tx.save(&original, &first))
        .unwrap();
    assert_eq!(unchanged, first);

    let unrelated = stream("other", "Other");
    let second = store
        .with_transaction(|tx| tx.save(&unrelated, &first))
        .unwrap();
    assert_eq!(
        second.revisions[&original.key()],
        first.revisions[&original.key()]
    );
    let stale = store
        .with_transaction(|tx| tx.save(&original, &first))
        .unwrap_err();
    assert!(stale.is_revision_conflict());
    assert!(!stale.is_uncertain());
    assert_eq!(store.with_transaction(|tx| tx.read()).unwrap(), second);

    let changed = stream("work", "Changed");
    let third = store
        .with_transaction(|tx| tx.save(&changed, &second))
        .unwrap();
    assert_ne!(
        third.revisions[&original.key()],
        second.revisions[&original.key()]
    );
    let restored = store
        .with_transaction(|tx| tx.save(&original, &third))
        .unwrap();
    assert_eq!(restored.catalog, second.catalog);
    assert_ne!(restored.revisions, second.revisions);
    assert!(
        store
            .with_transaction(|tx| tx.save(&changed, &second))
            .unwrap_err()
            .is_revision_conflict()
    );
    assert_eq!(store.with_transaction(|tx| tx.read()).unwrap(), restored);

    let missing_stream = Record::Session(
        Session::new("session", "host", "missing", "pi", "native", "Session").unwrap(),
    );
    let invalid = store
        .with_transaction(|tx| tx.save(&missing_stream, &restored))
        .unwrap_err();
    assert!(!invalid.is_uncertain());
    assert!(!invalid.is_revision_conflict());
    assert_eq!(store.with_transaction(|tx| tx.read()).unwrap(), restored);

    let mut invalid_record = Stream::new("invalid", "Valid", None, "host").unwrap();
    invalid_record.title.clear();
    assert!(
        store
            .with_transaction(|tx| tx.save(&Record::Stream(invalid_record), &restored))
            .is_err()
    );
    assert_eq!(store.with_transaction(|tx| tx.read()).unwrap(), restored);

    // An error from the closure is not a rollback of completed durable effects.
    let journal = store.journal();
    let saved = stream("committed", "Committed");
    let error = store.with_transaction::<()>(|tx| {
        tx.save(&saved, &restored)?;
        journal.write(
            "probe.json",
            json!({"version": 1, "value": "kept"})
                .as_object()
                .unwrap()
                .clone(),
        )?;
        Err(StorageError::new("after effects"))
    });
    assert!(error.is_err());
    store
        .with_transaction(|tx| {
            assert_eq!(tx.read()?.catalog.get(&saved.key()), Some(saved));
            assert_eq!(
                store.journal().read("probe.json")?.unwrap()["value"],
                "kept"
            );
            assert!(journal.names()?.contains(&"probe.json".to_owned()));
            journal.clear("probe.json")?;
            journal.clear("probe.json")?;
            assert!(journal.read("probe.json")?.is_none());
            assert!(!journal.names()?.contains(&"probe.json".to_owned()));
            Ok(())
        })
        .unwrap();

    // The adapter persists objects; version policy belongs to State, not Journal.
    store
        .with_transaction(|_| {
            let state = State::from_journal(store.journal());
            journal.write(
                "probe.json",
                json!({"version": 2}).as_object().unwrap().clone(),
            )?;
            assert!(state.read("probe.json").is_err());
            state
                .write(
                    "probe.json",
                    json!({"value": "current"}).as_object().unwrap().clone(),
                )
                .unwrap();
            assert_eq!(journal.read("probe.json")?.unwrap()["version"], 1);
            assert_eq!(
                state.read("probe.json").unwrap().unwrap()["value"],
                "current"
            );
            state.clear("probe.json").unwrap();
            Ok(())
        })
        .unwrap();
}

#[test]
fn yaml_storage_conforms() {
    let dir = tempfile::tempdir().unwrap();
    storage_laws(&YamlStore::new(dir.path()));
}

#[test]
fn memory_storage_conforms() {
    storage_laws(&memory::MemoryStore::default());
}

#[test]
fn memory_adapter_detects_access_outside_the_owning_transaction() {
    let store = memory::MemoryStore::default();
    let journal = store.journal();
    assert!(journal.read("probe.json").is_err());
    assert!(journal.write("probe.json", Default::default()).is_err());
    assert!(journal.clear("probe.json").is_err());
    assert!(journal.names().is_err());
    assert!(store.check_local().is_err());
    store
        .with_transaction(|_| {
            store.check_local()?;
            let foreign = journal.clone();
            assert!(
                std::thread::spawn(move || foreign.names())
                    .join()
                    .unwrap()
                    .is_err()
            );
            Ok(())
        })
        .unwrap();
    let unwind = std::panic::catch_unwind(|| {
        let _ = store.with_transaction::<()>(|_| panic!("unwind transaction"));
    });
    assert!(unwind.is_err());
    assert!(journal.names().is_err());
    store.with_transaction(|_| store.check_local()).unwrap();
}
