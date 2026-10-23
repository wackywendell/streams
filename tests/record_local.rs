//! Real YAML I/O observation is available to integration fixtures, not only unit tests.

use std::sync::{Arc, Mutex};

use serde_json::{Map, json};
use streams_temper::{
    model::{Record, RecordKind, Session, Stream},
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
    let session =
        Record::Session(Session::new("session", "host", "stream", "pi", "native", "Work").unwrap());
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
