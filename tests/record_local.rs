//! Real YAML I/O observation is available to integration fixtures, not only unit tests.

use std::sync::{Arc, Mutex};

use serde_json::{Map, json};
use streams_temper::{
    application::move_session,
    model::{Record, RecordKind, Session, Stream},
    operations::membership::MoveIntent,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
    },
    yaml_store::records::{RecordIoEvent, YamlRecordStore, decode_record, relative_record_path},
};

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
        *events.lock().unwrap(),
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
    let first = move_session(&store, "move-b", &to_b).unwrap();
    assert!(!first.repeated);
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
            RecordIoEvent::Read(root.join(relative_record_path(&session_key).unwrap())),
            RecordIoEvent::Read(a_path.clone()),
            RecordIoEvent::Read(b_path.clone()),
        ]
    );
    let to_a = MoveIntent::new("host", "pi", "native", "a").unwrap();
    let returned = move_session(&store, "move-a", &to_a).unwrap();
    events.lock().unwrap().clear();
    let retry = move_session(&store, "move-b", &to_b).unwrap();
    assert!(retry.repeated);
    assert_eq!(retry.current, returned.current);
    assert_eq!(retry.outcome, first.outcome);
    assert_eq!(retry.outcome["membership_period"], 1);
    assert_eq!(
        *events.lock().unwrap(),
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
        move_session(&store, "missing", &missing)
            .unwrap_err()
            .to_string()
            .contains("create it before moving")
    );
    assert!(!root.join("hosts/host/streams/missing.yaml").exists());
}
