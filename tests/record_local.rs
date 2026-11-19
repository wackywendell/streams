//! Real record I/O observation is available to integration fixtures, not only unit tests.

use crate::support::record_io;
use std::sync::{Arc, Mutex};

use streams::{
    application::move_session_observed,
    file_store::records::{FileRecordStore, RecordIoEvent, decode_record, relative_record_path},
    model::{Record, RecordKind, Session, Stream},
    operations::{DomainOutcome, membership::MoveIntent},
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordStore},
    },
};

#[test]
fn user_reference_edits_swap_on_the_stream_revision_and_a_landed_retry_writes_nothing() {
    use streams::{
        application::save_user_references,
        model::{Reference, ReferenceRole, ReferenceState, ReferenceTarget, TitleDefaults},
        operations::{DomainOutcome, references::ReferenceChange},
    };
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let store = FileRecordStore::new(root, "host").unwrap();
    store.initialize().unwrap();
    let value = |url: &str| Reference {
        name: "Reference".into(),
        purpose: "Guide work".into(),
        role: ReferenceRole::Brief,
        state: ReferenceState::Active,
        target: ReferenceTarget::Link { url: url.into() },
    };
    let a = value("https://example.test/a");
    let b = value("https://example.test/b");
    let mut stream = Stream::new("stream", "Stream", None, "host").unwrap();
    stream.references = vec![a.clone(), b.clone()];
    let record = Record::Stream(stream);
    let address = RecordAddress::for_record(&record);
    let saved = store
        .save::<_, StorageError>(&address, |_, _| {
            Ok(RecordChange {
                record,
                outcome: (),
            })
        })
        .unwrap();
    let Record::Stream(before) = saved.current.record else {
        unreachable!()
    };
    let mut desired = value("https://example.test/a");
    desired.purpose = "Newer purpose".into();
    let changes = vec![
        ReferenceChange::UserSet {
            key: a.target.key(),
            desired,
        },
        ReferenceChange::UserDelete {
            key: b.target.key(),
        },
    ];
    let first =
        save_user_references(&store, &before, changes.clone(), TitleDefaults::default()).unwrap();
    assert_eq!(first.outcome.outcome, DomainOutcome::Changes);
    let Record::Stream(after) = &first.current.record else {
        unreachable!()
    };
    assert_eq!(after.references.len(), 1);
    assert_eq!(after.references[0].purpose, "Newer purpose");
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
    let retry =
        save_user_references(&store, &before, changes.clone(), TitleDefaults::default()).unwrap();
    assert!(!retry.written);
    assert_eq!(retry.outcome.outcome, DomainOutcome::NoChange);
    assert_eq!(retry.current, first.current);
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
    let delete = vec![ReferenceChange::UserDelete {
        key: a.target.key(),
    }];
    let deleted = save_user_references(&store, after, delete, TitleDefaults::default()).unwrap();
    events.lock().unwrap().clear();
    // A stale capture whose rows no longer match the record conflicts and writes nothing.
    let stale = save_user_references(&store, &before, changes, TitleDefaults::default()).unwrap();
    assert!(!stale.written);
    assert_eq!(stale.outcome.outcome, DomainOutcome::Conflict);
    assert_eq!(stale.current, deleted.current);
    assert!(record_io::writes(&events.lock().unwrap()).is_empty());
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
        store
            .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
                Ok(RecordChange {
                    record,
                    outcome: (),
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
fn membership_save_reads_only_three_records_and_a_repeated_move_writes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let store = FileRecordStore::new(root, "host")
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
        store
            .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
                Ok(RecordChange {
                    record,
                    outcome: (),
                })
            })
            .unwrap();
    }
    let a_bytes = std::fs::read(&a_path).unwrap();
    let b_bytes = std::fs::read(&b_path).unwrap();
    events.lock().unwrap().clear();
    let to_b = MoveIntent::new("host", "pi", "native", "b").unwrap();
    let observed = move_session_observed(&store, &to_b).unwrap();
    assert_eq!(observed.stream.id, "b");
    let first = observed.save;
    assert!(first.written);
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
    events.lock().unwrap().clear();
    let observed = move_session_observed(&store, &to_b).unwrap();
    assert_eq!(observed.stream.id, "b");
    let retry = observed.save;
    assert!(!retry.written);
    assert_eq!(retry.current, first.current);
    assert_eq!(retry.outcome, DomainOutcome::NoChange);
    assert_eq!(
        record_io::accesses(&events.lock().unwrap()),
        vec![
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
            RecordIoEvent::Read(root.join(relative_record_path(&session_key).unwrap())),
            RecordIoEvent::Read(b_path.clone()),
        ]
    );
    let Record::Session(current) = retry.current.record else {
        unreachable!()
    };
    assert_eq!(current.stream_id, "b");
    assert_eq!(current.membership_epoch, 1);
    assert_eq!(std::fs::read(a_path).unwrap(), a_bytes);
    assert_eq!(std::fs::read(b_path).unwrap(), b_bytes);
    let missing = MoveIntent::new("host", "pi", "native", "missing").unwrap();
    assert!(
        move_session_observed(&store, &missing)
            .err()
            .unwrap()
            .to_string()
            .contains("create it before moving")
    );
    assert!(!root.join("hosts/host/streams/missing.json").exists());
}
