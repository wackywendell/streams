//! Real YAML I/O observation is available to integration fixtures, not only unit tests.

use crate::support::record_io;
use std::sync::{Arc, Mutex};

use serde_json::{Map, json};
use streams::{
    application::move_session_observed,
    model::{Record, RecordKind, Session, Stream},
    operations::membership::MoveIntent,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
    },
    yaml_store::records::{RecordIoEvent, YamlRecordStore, decode_record, relative_record_path},
};

#[test]
fn reference_partial_receipt_replays_after_deletion_and_refuses_changed_intent() {
    use streams::{
        application::save_user_references,
        model::{
            Reference, ReferenceRole, ReferenceState, ReferenceTarget, ReferenceValue,
            TitleDefaults,
        },
        operations::references::ReferenceChange,
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let store = YamlRecordStore::new(root, "host").unwrap();
    store.initialize().unwrap();
    let value = |url: &str| ReferenceValue {
        name: "Reference".into(),
        purpose: "Guide work".into(),
        role: ReferenceRole::Brief,
        state: ReferenceState::Active,
        target: ReferenceTarget::Link { url: url.into() },
    };
    let a = Reference::new(value("https://example.test/a"), "a").unwrap();
    let b = Reference::new(value("https://example.test/b"), "b").unwrap();
    let mut before = Stream::new("stream", "Stream", None, "host").unwrap();
    before.references = vec![a.clone(), b.clone()];
    let mut current = before.clone();
    current.references[0].purpose = "Newer purpose".into();
    let record = Record::Stream(current.clone());
    let address = RecordAddress::for_record(&record);
    let request = RecordRequest::new(address.clone(), "setup", &json!({"record":record})).unwrap();
    store
        .save::<StorageError>(&request, |_, _| {
            Ok(RecordChange {
                record,
                outcome: Map::new(),
            })
        })
        .unwrap();
    let changes = vec![
        ReferenceChange::UserSet {
            key: a.target.key(),
            desired: value("https://example.test/a"),
            next_revision: "partial".into(),
        },
        ReferenceChange::UserDelete {
            key: b.target.key(),
        },
    ];
    let first = save_user_references(
        &store,
        "partial",
        &before,
        changes.clone(),
        TitleDefaults::default(),
    )
    .unwrap();
    assert_eq!(first.outcome["outcome"], "changes");
    assert_eq!(first.outcome["reference_results"][0]["outcome"], "conflict");
    assert_eq!(first.outcome["reference_results"][1]["outcome"], "changes");
    let Record::Stream(after) = &first.current.record else {
        unreachable!()
    };
    assert_eq!(after.references, [current.references[0].clone()]);
    let delete = vec![ReferenceChange::UserDelete {
        key: a.target.key(),
    }];
    let deleted =
        save_user_references(&store, "delete", after, delete, TitleDefaults::default()).unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
    let retry = save_user_references(
        &store,
        "partial",
        &before,
        changes.clone(),
        TitleDefaults::default(),
    )
    .unwrap();
    assert!(retry.repeated);
    assert_eq!(retry.current, deleted.current);
    assert_eq!(retry.outcome, first.outcome);
    let trace = events.lock().unwrap().clone();
    assert_eq!(
        record_io::accesses(&trace),
        [
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
            RecordIoEvent::Read(root.join(relative_record_path(&address).unwrap())),
        ]
    );
    assert!(record_io::writes(&trace).is_empty());
    let mut changed = changes;
    let ReferenceChange::UserSet { desired, .. } = &mut changed[0] else {
        unreachable!()
    };
    desired.purpose = "Different intent".into();
    assert!(
        save_user_references(
            &store,
            "partial",
            &before,
            changed,
            TitleDefaults::default()
        )
        .is_err()
    );
}

#[test]
fn public_observer_names_actual_session_stream_and_fixed_file_reads() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let store = YamlRecordStore::new(root, "host")
        .unwrap()
        .with_io_observer(move |event| observed.lock().unwrap().push(event));
    store.initialize().unwrap();
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| matches!(event, RecordIoEvent::List(_)))
            .cloned()
            .collect::<Vec<_>>(),
        [RecordIoEvent::List(root.into())]
    );
    let stream = Record::Stream(Stream::new("stream", "Stream", None, "host").unwrap());
    let session = Record::Session(
        Session::new(
            Session::id_for("host", "pi", "native"),
            "host",
            "stream",
            "pi",
            "native",
            "Work",
        )
        .unwrap(),
    );
    let session_key = RecordAddress::for_record(&session);
    let stream_key = RecordAddress::for_record(&stream);
    for record in [stream, session] {
        let request =
            RecordRequest::new(RecordAddress::for_record(&record), "create", &json!({})).unwrap();
        store
            .save::<StorageError>(&request, |_, _| {
                Ok(RecordChange {
                    record,
                    outcome: Map::new(),
                })
            })
            .unwrap();
    }
    events.lock().unwrap().clear();
    store
        .with_reader::<_, StorageError>(|reader| {
            let current = reader.read(&session_key)?.unwrap();
            let Record::Session(session) = current.record else {
                unreachable!()
            };
            let stream =
                RecordAddress::new(session.host_id, RecordKind::Stream, session.stream_id)?;
            assert!(reader.read(&stream)?.is_some());
            Ok(())
        })
        .unwrap();
    assert_eq!(
        record_io::accesses(&events.lock().unwrap()),
        vec![
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
            RecordIoEvent::Read(root.join(relative_record_path(&session_key).unwrap())),
            RecordIoEvent::Read(root.join(relative_record_path(&stream_key).unwrap())),
        ]
    );
    let bytes = std::fs::read(root.join(relative_record_path(&session_key).unwrap())).unwrap();
    drop(store);
    drop(directory);
    let decoded = decode_record(&session_key, &bytes).unwrap();
    let Record::Session(session) = decoded.record else {
        unreachable!()
    };
    assert_eq!(session.stream_id, stream_key.id());
}

#[test]
fn membership_save_reads_only_three_records_and_retries_do_not_reapply_after_revisit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let store = YamlRecordStore::new(root, "host")
        .unwrap()
        .with_io_observer(move |event| observed.lock().unwrap().push(event));
    store.initialize().unwrap();
    let a = Record::Stream(Stream::new("a", "A", None, "host").unwrap());
    let b = Record::Stream(Stream::new("b", "B", None, "host").unwrap());
    let session = Record::Session(
        Session::new(
            Session::id_for("host", "pi", "native"),
            "host",
            "a",
            "pi",
            "native",
            "Work",
        )
        .unwrap(),
    );
    let session_key = RecordAddress::for_record(&session);
    let a_path = root.join(relative_record_path(&RecordAddress::for_record(&a)).unwrap());
    let b_path = root.join(relative_record_path(&RecordAddress::for_record(&b)).unwrap());
    for record in [a, b, session] {
        let request =
            RecordRequest::new(RecordAddress::for_record(&record), "create", &json!({})).unwrap();
        store
            .save::<StorageError>(&request, |_, _| {
                Ok(RecordChange {
                    record,
                    outcome: Map::new(),
                })
            })
            .unwrap();
    }
    let a_bytes = std::fs::read(&a_path).unwrap();
    let b_bytes = std::fs::read(&b_path).unwrap();
    events.lock().unwrap().clear();
    let to_b = MoveIntent::new("host", "pi", "native", "b").unwrap();
    let observed = move_session_observed(&store, "move-b", &to_b).unwrap();
    assert_eq!(observed.stream.unwrap().id, "b");
    let first = observed.save;
    assert!(!first.repeated);
    assert_eq!(
        record_io::accesses(&events.lock().unwrap()),
        vec![
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
            RecordIoEvent::Read(root.join(relative_record_path(&session_key).unwrap())),
            RecordIoEvent::Read(a_path.clone()),
            RecordIoEvent::Read(b_path.clone()),
        ]
    );
    let to_a = MoveIntent::new("host", "pi", "native", "a").unwrap();
    let returned = move_session_observed(&store, "move-a", &to_a).unwrap().save;
    events.lock().unwrap().clear();
    let observed = move_session_observed(&store, "move-b", &to_b).unwrap();
    assert!(observed.stream.is_none());
    let retry = observed.save;
    assert!(retry.repeated);
    assert_eq!(retry.current, returned.current);
    assert_eq!(retry.outcome, first.outcome);
    assert_eq!(retry.outcome["membership_period"], 1);
    assert_eq!(
        record_io::accesses(&events.lock().unwrap()),
        vec![
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
            RecordIoEvent::Read(root.join(relative_record_path(&session_key).unwrap())),
        ]
    );
    let Record::Session(current) = retry.current.record else {
        unreachable!()
    };
    assert_eq!(current.stream_id, "a");
    assert_eq!(current.previous_stream_ids, ["a", "b"]);
    assert_eq!(std::fs::read(a_path).unwrap(), a_bytes);
    assert_eq!(std::fs::read(b_path).unwrap(), b_bytes);
    let missing = MoveIntent::new("host", "pi", "native", "missing").unwrap();
    assert!(
        move_session_observed(&store, "missing", &missing)
            .err()
            .unwrap()
            .to_string()
            .contains("create it before moving")
    );
    assert!(!root.join("hosts/host/streams/missing.yaml").exists());
}
