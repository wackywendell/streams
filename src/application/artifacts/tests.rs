use super::*;
use crate::{
    file_store::records::{
        FileRecordStore, RecordIoEvent, relative_artifact_path, relative_record_path,
    },
    model::{Stream, Title},
    storage::{
        StorageError,
        records::{RecordInventory, RecordStore},
    },
};
use std::{
    fs,
    sync::{Arc, Mutex},
};

fn put(store: &FileRecordStore, record: Record) {
    store
        .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
            Ok(RecordChange {
                record,
                outcome: (),
            })
        })
        .unwrap();
}

fn setup() -> (
    tempfile::TempDir,
    FileRecordStore,
    Session,
    Arc<Mutex<Vec<RecordIoEvent>>>,
) {
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let store = FileRecordStore::new(root.path(), "host")
        .unwrap()
        .with_io_observer(move |event| capture.lock().unwrap().push(event));
    store.initialize().unwrap();
    put(
        &store,
        Record::Stream(Stream::new("host", "stream", Title::new("Work", false), None).unwrap()),
    );
    let session = Session::new(
        "host",
        "pi",
        "native",
        "stream",
        Title::new("Assessment", false),
    )
    .unwrap();
    put(&store, Record::Session(session.clone()));
    events.lock().unwrap().clear();
    (root, store, session, events)
}

fn intent(session: &Session, text: Option<&str>) -> WriteArtifact {
    WriteArtifact {
        harness: session.harness.clone(),
        session_id: session.id.clone(),
        name: "assessment.md".into(),
        description: "Findings".into(),
        content: text.map(Into::into),
    }
}

fn key(session: &Session) -> ArtifactKey {
    ArtifactKey {
        host: session.host_id.clone(),
        harness: session.harness.clone(),
        session: session.id.clone(),
        name: "assessment.md".into(),
    }
}

#[test]
fn crud_keeps_metadata_only_in_session_and_emits_original_text() {
    let (root, store, session, _) = setup();
    let saved = write_artifact(
        &store,
        "host",
        &intent(&session, Some("# Findings\nUseful output\n")),
    )
    .unwrap();
    assert!(saved.warnings.is_empty());
    let key = key(&session);
    assert_eq!(
        fs::read_to_string(root.path().join(relative_artifact_path(&key).unwrap())).unwrap(),
        "# Findings\nUseful output\n"
    );
    assert_eq!(
        read_artifact(&store, &key).unwrap().artifact.description,
        "Findings"
    );
    let mut update = intent(&session, None);
    update.description = "Revised description".into();
    write_artifact(&store, "host", &update).unwrap();
    let output = read_artifact(&store, &key).unwrap();
    assert_eq!(output.content, "# Findings\nUseful output\n");
    assert_eq!(output.artifact.description, "Revised description");
    write_artifact(&store, "host", &intent(&session, Some("new"))).unwrap();
    let retry = write_artifact(&store, "host", &intent(&session, Some("new"))).unwrap();
    assert!(!retry.save.written);
    assert_eq!(read_artifact(&store, &key).unwrap().content, "new");
    delete_artifact(&store, &key).unwrap();
    assert!(
        !root
            .path()
            .join(relative_artifact_path(&key).unwrap())
            .exists()
    );
    assert!(read_artifact(&store, &key).is_err());
    assert!(
        list_session_artifacts(&store, "host", &session.harness, &session.id)
            .unwrap()
            .is_empty()
    );
    assert!(store.inventory().is_ok());
    assert!(!delete_artifact(&store, &key).unwrap().save.written);
    write_artifact(&store, "host", &intent(&session, Some("recreated"))).unwrap();
    assert_eq!(read_artifact(&store, &key).unwrap().content, "recreated");
}

#[test]
fn reads_and_session_lists_pin_io_without_reading_contents_for_descriptions() {
    let (_root, store, session, events) = setup();
    write_artifact(&store, "host", &intent(&session, Some("output"))).unwrap();
    let key = key(&session);
    let session_path = relative_record_path(
        &RecordAddress::session("host", &session.harness, &session.id).unwrap(),
    )
    .unwrap();
    events.lock().unwrap().clear();
    assert_eq!(
        list_session_artifacts(&store, "host", &session.harness, &session.id)
            .unwrap()
            .len(),
        1
    );
    let trace = events.lock().unwrap().clone();
    let reads: Vec<_> = trace
        .iter()
        .filter_map(|e| {
            if let RecordIoEvent::Read(path) = e {
                Some(path.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(reads.len(), 2); // marker + named Session
    assert!(reads[1].ends_with(&session_path));
    assert!(!trace.iter().any(|e| matches!(e, RecordIoEvent::List(_))));

    events.lock().unwrap().clear();
    assert_eq!(read_artifact(&store, &key).unwrap().content, "output");
    let trace = events.lock().unwrap().clone();
    assert_eq!(
        trace
            .iter()
            .filter(|e| matches!(e, RecordIoEvent::Read(_)))
            .count(),
        3
    ); // marker, Session, content
    assert!(!trace.iter().any(|e| matches!(e, RecordIoEvent::List(_))));
}

#[test]
fn moving_a_session_keeps_its_artifacts_and_key_unchanged() {
    let (_root, store, session, _) = setup();
    write_artifact(&store, "host", &intent(&session, Some("output"))).unwrap();
    let key = key(&session);
    put(
        &store,
        Record::Stream(
            Stream::new("host", "destination", Title::new("Other work", false), None).unwrap(),
        ),
    );
    let mut moved = store
        .with_artifacts(|reader| read_session(reader, "host", &session.harness, &session.id))
        .unwrap();
    moved.stream_id = "destination".into();
    put(&store, Record::Session(moved));

    let rows = list_session_artifacts(&store, "host", &session.harness, &session.id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].key, key.to_string());
    assert_eq!(rows[0].stream_id, "destination");
    assert_eq!(read_artifact(&store, &key).unwrap().content, "output");
}

#[test]
fn missing_artifact_file_fails_read_and_inventory() {
    let (root, store, session, _) = setup();
    write_artifact(&store, "host", &intent(&session, Some("output"))).unwrap();
    let key = key(&session);
    fs::remove_file(root.path().join(relative_artifact_path(&key).unwrap())).unwrap();
    assert!(read_artifact(&store, &key).is_err());
    assert!(store.inventory().is_err());
}
