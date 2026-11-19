use super::*;
use crate::{
    file_store::records::{FileRecordStore, RecordIoEvent, relative_record_path},
    runtime::conversations::Harness,
    storage::records::{RecordChange, RecordStore},
};
use std::{
    fs,
    sync::{Arc, Mutex},
};

fn put(store: &FileRecordStore, record: Record) -> Record {
    store
        .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
            Ok(RecordChange {
                record,
                outcome: (),
            })
        })
        .unwrap()
        .current
}

fn request(native_id: &str) -> ContextRequest<'_> {
    ContextRequest {
        harness: Harness::Pi,
        native_id,
        stream_id: None,
        environment_stream_id: None,
        workspace_stream_id: None,
    }
}

#[test]
fn addressed_context_reads_only_this_session_and_stream_despite_unrelated_growth() {
    for size in [0, 64] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let store = FileRecordStore::new(root, "host").unwrap();
        store.initialize().unwrap();
        let session = Session::new(
            Session::id_for("host", "pi", "native"),
            "host",
            "effort",
            "pi",
            "native",
            "Work",
        )
        .unwrap();
        put(
            &store,
            Record::Stream(Stream::new("effort", "Effort", None, "host").unwrap()),
        );
        let Record::Session(session) = put(&store, Record::Session(session)) else {
            unreachable!()
        };
        for index in 0..size {
            let address =
                RecordAddress::new("host", RecordKind::Stream, format!("unrelated-{index}"))
                    .unwrap();
            fs::write(
                root.join(relative_record_path(&address).unwrap()),
                "malformed unrelated record",
            )
            .unwrap();
        }
        fs::create_dir_all(root.join(".catalog-sync")).unwrap();
        fs::write(root.join(".catalog-sync/history"), "not replayable").unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
        let context = store
            .with_reader(|reader| {
                record_context(
                    reader,
                    "host",
                    "source",
                    request("native"),
                    crate::model::TitleDefaults {
                        stream: crate::model::NamingMode::Manual,
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        assert_eq!(
            context.title_defaults.stream,
            crate::model::NamingMode::Manual
        );
        assert_eq!(
            context.title_defaults.session,
            crate::model::NamingMode::Automatic
        );
        assert_eq!(context.session, Some(session.clone()));
        assert_eq!(context.sessions, [session]);
        assert_eq!(context.stream.unwrap().id, "effort");
        let session_address = RecordAddress::new(
            "host",
            RecordKind::Session,
            Session::id_for("host", "pi", "native"),
        )
        .unwrap();
        let stream_address = RecordAddress::new("host", RecordKind::Stream, "effort").unwrap();
        let trace = events.lock().unwrap().clone();
        let files = [
            root.join(".catalog-local/records.json"),
            root.join(relative_record_path(&session_address).unwrap()),
            root.join(relative_record_path(&stream_address).unwrap()),
        ];
        assert_eq!(
            trace.iter().map(RecordIoEvent::bytes_read).sum::<usize>(),
            files
                .iter()
                .map(|path| fs::metadata(path).unwrap().len() as usize)
                .sum::<usize>()
        );
        assert!(!trace.iter().any(|event| event.written().is_some()));
        assert_eq!(
            trace
                .iter()
                .filter(|event| event.is_access())
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                RecordIoEvent::Lock(root.join(".catalog.lock")),
                RecordIoEvent::Read(root.join(".catalog-local/records.json")),
                RecordIoEvent::Read(root.join(relative_record_path(&session_address).unwrap())),
                RecordIoEvent::Read(root.join(relative_record_path(&stream_address).unwrap())),
            ]
        );
    }
}

#[test]
fn valid_membership_wins_over_stale_ambient_associations_but_explicit_mismatch_refuses() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    put(
        &store,
        Record::Stream(Stream::new("destination", "Destination", None, "host").unwrap()),
    );
    let mut session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "destination",
        "pi",
        "native",
        "Work",
    )
    .unwrap();
    session.membership_epoch = 1;
    put(&store, Record::Session(session));
    let mut input = request("native");
    input.environment_stream_id = Some("source");
    input.workspace_stream_id = Some("source");
    let context = store
        .with_reader(|reader| {
            record_context(
                reader,
                "host",
                "source",
                input,
                crate::model::TitleDefaults::default(),
            )
        })
        .unwrap();
    assert_eq!(context.stream.unwrap().id, "destination");
    input.stream_id = Some("source");
    assert!(
        store
            .with_reader(|reader| record_context(
                reader,
                "host",
                "source",
                input,
                crate::model::TitleDefaults::default()
            ))
            .is_err()
    );
}

#[test]
fn completed_format_missing_session_is_unenrolled_but_a_missing_relationship_is_not() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    let context = store
        .with_reader(|reader| {
            record_context(
                reader,
                "host",
                "source",
                request("native"),
                crate::model::TitleDefaults::default(),
            )
        })
        .unwrap();
    assert!(context.session.is_none() && context.stream.is_none() && context.sessions.is_empty());
    assert!(
        serde_json::to_value(context.wire())
            .unwrap()
            .get("authority")
            .is_none()
    );
    let session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "missing",
        "pi",
        "native",
        "Work",
    )
    .unwrap();
    put(&store, Record::Session(session));
    let error = store
        .with_reader(|reader| {
            record_context(
                reader,
                "host",
                "source",
                request("native"),
                crate::model::TitleDefaults::default(),
            )
        })
        .unwrap_err();
    assert!(error.to_string().contains("Stream is missing"));
}

#[test]
fn unenrolled_context_uses_only_unambiguous_exact_hints() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    put(
        &store,
        Record::Stream(Stream::new("effort", "Effort", None, "host").unwrap()),
    );
    let mut input = request("native");
    input.environment_stream_id = Some("effort");
    let context = store
        .with_reader(|reader| {
            record_context(
                reader,
                "host",
                "source",
                input,
                crate::model::TitleDefaults::default(),
            )
        })
        .unwrap();
    assert_eq!(context.stream.unwrap().id, "effort");
    assert!(context.session.is_none());
    input.workspace_stream_id = Some("different");
    assert!(
        store
            .with_reader(|reader| record_context(
                reader,
                "host",
                "source",
                input,
                crate::model::TitleDefaults::default()
            ))
            .is_err()
    );
}
