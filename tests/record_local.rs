//! Real record I/O observation for format-5 record-local reads, edits, and moves.

use crate::support::record_io;
use serde_json::json;
use std::sync::{Arc, Mutex};

use streams::{
    application::{self, edit_session},
    file_store::records::{FileRecordStore, RecordIoEvent, relative_record_path},
    model::{Record, Session, Stream, Title},
    operations::edits::EditRequest,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordStore},
    },
};

#[test]
fn session_reference_edits_and_moves_read_only_named_records() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let store = FileRecordStore::new(root, "host")
        .unwrap()
        .with_io_observer(move |event| observed.lock().unwrap().push(event));
    store.initialize().unwrap();

    let s1 = Record::Stream(Stream::new("host", "s1", Title::new("One", false), None).unwrap());
    let s2 = Record::Stream(Stream::new("host", "s2", Title::new("Two", false), None).unwrap());
    let session = Record::Session(
        Session::new("host", "pi", "native", "s1", Title::new("Work", false)).unwrap(),
    );
    let session_addr = RecordAddress::for_record(&session);
    let s2_addr = RecordAddress::for_record(&s2);
    for record in [s1, s2, session] {
        store
            .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
                Ok(RecordChange {
                    record,
                    outcome: (),
                })
            })
            .unwrap();
    }

    // Adding a reference reads only lock, marker, and Session (zero directory listings).
    events.lock().unwrap().clear();
    let add_req = EditRequest::parse(&json!({
        "changes": {
            "references": {
                "https://example.test/pr/1": {
                    "kind": "pr",
                    "name": "PR 1",
                    "purpose": "Implements it",
                    "role": "deliverable",
                    "status": "active",
                    "manual": false
                }
            }
        },
        "before": {
            "references": {
                "https://example.test/pr/1": null
            }
        }
    }))
    .unwrap();
    let res = edit_session(&store, &session_addr, &add_req).unwrap();
    assert!(res.is_complete());
    assert!(res.written);
    let trace = events.lock().unwrap().clone();
    assert_eq!(
        record_io::accesses(&trace),
        vec![
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
            RecordIoEvent::Read(root.join(relative_record_path(&session_addr).unwrap())),
        ]
    );

    // Replaying the same request sees the entry already present with the exact requested value:
    // not_written is empty and nothing is written.
    events.lock().unwrap().clear();
    let retry = edit_session(&store, &session_addr, &add_req).unwrap();
    assert!(retry.is_complete());
    assert!(!retry.written);
    assert!(record_io::writes(&events.lock().unwrap()).is_empty());

    // Moving the Session to s2 reads only Session + destination Stream s2.
    events.lock().unwrap().clear();
    let move_req = EditRequest::parse(&json!({
        "changes": {"stream_id": "s2"},
        "before": {"stream_id": "s1"}
    }))
    .unwrap();
    let moved = edit_session(&store, &session_addr, &move_req).unwrap();
    assert!(moved.is_complete());
    assert!(moved.written);
    assert_eq!(
        record_io::accesses(&events.lock().unwrap()),
        vec![
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
            RecordIoEvent::Read(root.join(relative_record_path(&session_addr).unwrap())),
            RecordIoEvent::Read(root.join(relative_record_path(&s2_addr).unwrap())),
        ]
    );
}

#[test]
fn public_observer_names_actual_session_stream_and_fixed_file_reads() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let store = FileRecordStore::new(root, "host")
        .unwrap()
        .with_io_observer(move |event| observed.lock().unwrap().push(event));
    store.initialize().unwrap();
    let stream =
        Record::Stream(Stream::new("host", "stream", Title::new("Stream", false), None).unwrap());
    let session = Record::Session(
        Session::new("host", "pi", "native", "stream", Title::new("Work", false)).unwrap(),
    );
    let session_key = RecordAddress::for_record(&session);
    let stream_key = RecordAddress::for_record(&stream);
    application::create_record(&store, &stream).unwrap();
    application::create_record(&store, &session).unwrap();

    events.lock().unwrap().clear();
    store
        .with_reader::<_, StorageError>(|reader| {
            let current = reader.read(&session_key)?.unwrap();
            let Record::Session(session) = current else {
                unreachable!()
            };
            let stream = RecordAddress::stream(session.host_id, session.stream_id)?;
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
}
