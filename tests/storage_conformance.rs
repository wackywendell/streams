use crate::support::memory;

use serde_json::json;
use streams_temper::{
    model::{Record, Session, Stream},
    storage::{StorageError, Store, Transaction},
    sync::state::State,
    yaml_store::YamlStore,
};

fn stream(id: &str, title: &str) -> Record {
    Record::Stream(Stream::new(id, title, None, "host").unwrap())
}

fn legacy_session(store: &YamlStore) -> (Record, std::path::PathBuf) {
    let session = Record::Session(
        Session::new("conversation", "host", "source", "pi", "native", "Work").unwrap(),
    );
    let legacy = store.root.join("sessions/source/host/conversation.yaml");
    store
        .with_transaction::<_, StorageError>(|tx| {
            for record in [
                stream("source", "Source"),
                stream("destination", "Destination"),
                session.clone(),
            ] {
                tx.save(&record, &tx.read()?)?;
            }
            std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
            let canonical = store.root.join(store.record_path(&session));
            std::fs::rename(&canonical, &legacy).unwrap();
            std::fs::remove_dir(canonical.parent().unwrap()).unwrap();
            std::fs::remove_dir(canonical.parent().unwrap().parent().unwrap()).unwrap();
            Ok(())
        })
        .unwrap();
    (session, legacy)
}

#[test]
fn legacy_membership_move_relocates_one_session_and_preserves_stale_save_checks() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    let (original, legacy) = legacy_session(&store);
    let canonical = store
        .root
        .join(streams_temper::yaml_store::relative_record_path(&original));
    assert!(legacy.is_file());
    assert!(!canonical.exists());
    store
        .with_transaction::<_, StorageError>(|tx| {
            let before = tx.read()?;
            assert_eq!(before.catalog.get(&original.key()), Some(original.clone()));
            let Record::Session(mut moved) = original.clone() else {
                unreachable!()
            };
            moved.stream_id = "destination".into();
            let moved = Record::Session(moved);
            let after = tx.save(&moved, &before)?;
            assert_eq!(after.catalog.sessions().len(), 1);
            assert_eq!(after.catalog.get(&original.key()), Some(moved.clone()));
            assert!(canonical.is_file());
            assert!(!legacy.exists());
            assert_ne!(
                after.revisions[&original.key()],
                before.revisions[&original.key()]
            );
            assert!(
                tx.save(&original, &before)
                    .unwrap_err()
                    .is_revision_conflict()
            );
            assert_eq!(tx.read()?, after);
            assert!(!legacy.exists());
            Ok(())
        })
        .unwrap();
}

#[test]
fn legacy_reads_and_same_membership_updates_do_not_promote_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    let (original, legacy) = legacy_session(&store);
    let canonical = store
        .root
        .join(streams_temper::yaml_store::relative_record_path(&original));
    let bytes = std::fs::read(&legacy).unwrap();
    let before = store.with_inspection(|tx| tx.read()).unwrap();
    assert_eq!(std::fs::read(&legacy).unwrap(), bytes);
    assert!(!canonical.exists());
    assert_eq!(store.root.join(store.record_path(&original)), legacy);
    let Record::Session(mut edited) = original else {
        unreachable!()
    };
    edited.title = "Updated title".into();
    let edited = Record::Session(edited);
    let after = store
        .with_transaction(|tx| tx.save(&edited, &before))
        .unwrap();
    assert_eq!(after.catalog.get(&edited.key()), Some(edited));
    assert_eq!(after.catalog.sessions().len(), 1);
    assert!(legacy.is_file());
    assert!(!canonical.exists());
}

#[test]
fn failed_replacement_after_relocation_is_uncertain_and_keeps_original_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    let (original, legacy) = legacy_session(&store);
    let canonical = store
        .root
        .join(streams_temper::yaml_store::relative_record_path(&original));
    store
        .with_transaction::<_, StorageError>(|tx| {
            let before = tx.read()?;
            let bytes = std::fs::read(&legacy).unwrap();
            // The rename succeeds, but atomic replacement cannot create a staging file.
            std::fs::remove_dir(store.root.join(".catalog-staging")).unwrap();
            std::fs::write(store.root.join(".catalog-staging"), "blocked").unwrap();
            let Record::Session(mut moved) = original.clone() else {
                unreachable!()
            };
            moved.stream_id = "destination".into();
            let error = tx.save(&Record::Session(moved), &before).unwrap_err();
            assert!(error.is_uncertain());
            assert!(!error.is_revision_conflict());
            assert!(!legacy.exists());
            assert_eq!(std::fs::read(&canonical).unwrap(), bytes);
            assert_eq!(tx.read()?, before);
            Ok(())
        })
        .unwrap();
}

#[test]
fn session_paths_validate_host_identity_and_legacy_membership() {
    let record = Record::Session(
        Session::new("conversation", "host", "source", "pi", "native", "Work").unwrap(),
    );
    let bytes = serde_json::to_vec(&json!({
        "version": 1, "revision": "fixture", "kind": "session",
        "record": streams_temper::codec::record_to_data(&record)
    }))
    .unwrap();
    for path in [
        "conversations/host/conversation.yaml",
        "sessions/source/host/conversation.yaml",
    ] {
        assert_eq!(YamlStore::decode_record(path, &bytes).unwrap(), record);
    }
    for path in [
        "conversations/other/conversation.yaml",
        "conversations/host/other.yaml",
        "sessions/destination/host/conversation.yaml",
        "sessions/source/other/conversation.yaml",
    ] {
        assert!(YamlStore::decode_record(path, &bytes).is_err(), "{path}");
    }
}

#[test]
fn relocation_boundary_retains_original_record_and_revision_after_interruption() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    let (original, legacy) = legacy_session(&store);
    let canonical = store
        .root
        .join(streams_temper::yaml_store::relative_record_path(&original));
    store
        .with_transaction::<_, StorageError>(|tx| {
            let before = tx.read()?;
            // Simulate termination after the atomic rename, before record replacement.
            std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
            std::fs::rename(&legacy, &canonical).unwrap();
            assert_eq!(tx.read()?, before);
            let Record::Session(mut changed) = original.clone() else {
                unreachable!()
            };
            changed.title = "Continued work".into();
            let after = tx.save(&Record::Session(changed), &before)?;
            assert_eq!(after.catalog.sessions().len(), 1);
            assert!(!legacy.exists());
            Ok(())
        })
        .unwrap();
}

#[test]
fn duplicate_session_copies_across_storage_layouts_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    let (original, legacy) = legacy_session(&store);
    let canonical = store
        .root
        .join(streams_temper::yaml_store::relative_record_path(&original));
    std::fs::create_dir_all(canonical.parent().unwrap()).unwrap();
    std::fs::copy(&legacy, canonical).unwrap();
    assert!(store.with_transaction(|tx| tx.read()).is_err());
}

fn storage_laws(store: &impl Store) {
    let empty = store
        .with_transaction::<_, StorageError>(|tx| {
            let journal = tx.journal();
            let snapshot = tx.read()?;
            assert!(journal.names()?.is_empty());
            Ok(snapshot)
        })
        .unwrap();
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
    let saved = stream("committed", "Committed");
    let error = store.with_transaction::<(), StorageError>(|tx| {
        let journal = tx.journal();
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
        .with_transaction::<_, StorageError>(|tx| {
            let journal = tx.journal();
            assert_eq!(tx.read()?.catalog.get(&saved.key()), Some(saved));
            assert_eq!(journal.read("probe.json")?.unwrap()["value"], "kept");
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
        .with_transaction::<_, StorageError>(|tx| {
            let journal = tx.journal();
            let state = State::from_journal(journal);
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
fn yaml_typed_envelope_rejects_malformed_fields_and_array_structs() {
    let record = stream("work", "Work");
    let envelope = json!({
        "version": 1,
        "revision": "revision",
        "kind": "stream",
        "record": streams_temper::codec::record_to_data(&record),
    });
    let path = "streams/work.yaml";
    let decode = |value: &serde_json::Value| {
        YamlStore::decode_record(path, &serde_json::to_vec(value).unwrap())
    };
    assert_eq!(decode(&envelope).unwrap(), record);
    assert!(decode(&json!([1, "revision", "stream", envelope["record"]])).is_err());
    for field in ["version", "revision", "kind", "record"] {
        let mut missing = envelope.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode(&missing).is_err(), "missing {field}");
        let mut null = envelope.clone();
        null[field] = serde_json::Value::Null;
        assert!(decode(&null).is_err(), "null {field}");
        let mut array = envelope.clone();
        array[field] = json!([]);
        assert!(decode(&array).is_err(), "array {field}");
    }
    for (field, value) in [
        ("version", json!(true)),
        ("version", json!(1.0)),
        ("version", json!(2)),
        ("revision", json!("")),
        ("revision", json!("  ")),
        ("revision", json!("bad\0revision")),
        ("kind", json!("unknown")),
        ("record", json!("not an object")),
        ("extra", json!(null)),
    ] {
        let mut invalid = envelope.clone();
        invalid[field] = value;
        assert!(decode(&invalid).is_err(), "{field}: {invalid}");
    }
    let array_record = json!({
        "version": 1,
        "revision": "revision",
        "kind": "host",
        "record": ["host", "Host", "2026-01-01T00:00:00+00:00"],
    });
    assert!(
        YamlStore::decode_record(
            "hosts/host.yaml",
            &serde_json::to_vec(&array_record).unwrap()
        )
        .is_err()
    );
    let mut unknown_record_field = envelope.clone();
    unknown_record_field["record"]["extra"] = json!(true);
    assert!(decode(&unknown_record_field).is_err());
    assert!(
        YamlStore::decode_record(
            "streams/elsewhere.yaml",
            &serde_json::to_vec(&envelope).unwrap()
        )
        .is_err()
    );
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
fn transactions_preserve_caller_errors_and_converted_storage_error_kinds() {
    #[derive(Debug, PartialEq, Eq)]
    enum CallerError {
        Closure,
        Storage(StorageError),
    }
    impl From<StorageError> for CallerError {
        fn from(error: StorageError) -> Self {
            Self::Storage(error)
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    let error = store
        .with_transaction::<(), CallerError>(|_| Err(CallerError::Closure))
        .unwrap_err();
    assert_eq!(error, CallerError::Closure);
    let error = memory::MemoryStore::default()
        .with_transaction::<(), CallerError>(|_| Err(CallerError::Closure))
        .unwrap_err();
    assert_eq!(error, CallerError::Closure);

    let empty = store.with_transaction(|tx| tx.read()).unwrap();
    let record = stream("work", "Work");
    store
        .with_transaction(|tx| tx.save(&record, &empty))
        .unwrap();
    let expected = store
        .with_transaction(|tx| tx.save(&record, &empty))
        .unwrap_err();
    assert!(expected.is_revision_conflict());
    let error = store
        .with_transaction(|tx| -> Result<_, CallerError> { Ok(tx.save(&record, &empty)?) })
        .unwrap_err();
    assert_eq!(error, CallerError::Storage(expected));

    let blocked = tempfile::tempdir().unwrap();
    std::fs::create_dir(blocked.path().join(".catalog.lock")).unwrap();
    let error = YamlStore::new(blocked.path())
        .with_transaction::<(), CallerError>(|_| panic!("invalid lock admitted transaction"))
        .unwrap_err();
    let CallerError::Storage(error) = error else {
        panic!("expected converted adapter error");
    };
    assert_eq!(
        error.kind(),
        streams_temper::storage::StorageErrorKind::Definite
    );
}

#[test]
fn memory_transactions_serialize_and_release_scope_on_unwind() {
    let store = memory::MemoryStore::default();

    let unwind = std::panic::catch_unwind(|| {
        let _ = store.with_transaction::<(), StorageError>(|tx| {
            tx.journal().write("probe.json", Default::default())?;
            panic!("unwind transaction");
        });
    });
    assert!(unwind.is_err());
    store
        .with_transaction::<_, StorageError>(|tx| {
            tx.check_local()?;
            assert!(tx.journal().read("probe.json")?.is_some());
            Ok(())
        })
        .unwrap();

    let other = store.clone();
    let (starting, started) = std::sync::mpsc::channel();
    let (finished, completed) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let worker = store
            .with_transaction::<_, StorageError>(|tx| {
                let worker = scope.spawn(move || {
                    starting.send(()).unwrap();
                    other
                        .with_transaction::<_, StorageError>(|tx| {
                            tx.check_local()?;
                            tx.journal().clear("probe.json")?;
                            finished.send(()).unwrap();
                            Ok(())
                        })
                        .unwrap();
                });
                started.recv().unwrap();
                assert!(completed.try_recv().is_err());
                assert!(tx.journal().read("probe.json")?.is_some());
                Ok(worker)
            })
            .unwrap();
        worker.join().unwrap();
        completed.recv().unwrap();
    });
    store
        .with_transaction::<_, StorageError>(|tx| {
            assert!(tx.journal().read("probe.json")?.is_none());
            Ok(())
        })
        .unwrap();
}
