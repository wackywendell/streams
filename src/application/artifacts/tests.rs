use super::*;
use crate::{
    artifacts::ArtifactEntry,
    model::Stream,
    storage::{
        StorageError,
        records::{RecordInventory, RecordStore},
    },
    yaml_store::records::{
        RecordIoEvent, YamlRecordStore, relative_artifact_path, relative_record_path,
    },
};
use std::{
    fs,
    sync::{Arc, Mutex},
};

fn put(store: &YamlRecordStore, id: &str, record: Record) {
    let request =
        RecordRequest::new(RecordAddress::for_record(&record), id, &json!({"id":id})).unwrap();
    store
        .save::<StorageError>(&request, |_, _| {
            Ok(RecordChange {
                record,
                outcome: Default::default(),
            })
        })
        .unwrap();
}

fn setup() -> (
    tempfile::TempDir,
    YamlRecordStore,
    Session,
    Arc<Mutex<Vec<RecordIoEvent>>>,
) {
    let root = tempfile::tempdir().unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let capture = events.clone();
    let store = YamlRecordStore::new(root.path(), "host")
        .unwrap()
        .with_io_observer(move |event| capture.lock().unwrap().push(event));
    store.initialize().unwrap();
    put(
        &store,
        "stream",
        Record::Stream(Stream::new("stream", "Work", None, "host").unwrap()),
    );
    let session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "stream",
        "pi",
        "native",
        "Assessment",
    )
    .unwrap();
    put(&store, "session", Record::Session(session.clone()));
    events.lock().unwrap().clear();
    (root, store, session, events)
}

fn intent(session: &Session, text: Option<&str>) -> WriteArtifact {
    WriteArtifact {
        session_id: session.id.clone(),
        stream_id: None,
        name: "assessment.md".into(),
        description: "Findings".into(),
        content: text.map(Into::into),
    }
}

fn key(session: &Session) -> ArtifactKey {
    ArtifactKey {
        host: session.host_id.clone(),
        stream: session.stream_id.clone(),
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
        "write",
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
    write_artifact(&store, "host", "description", &update).unwrap();
    let output = read_artifact(&store, &key).unwrap();
    assert_eq!(output.content, "# Findings\nUseful output\n");
    assert_eq!(output.artifact.description, "Revised description");
    write_artifact(&store, "host", "update", &intent(&session, Some("new"))).unwrap();
    let retry = write_artifact(
        &store,
        "host",
        "write",
        &intent(&session, Some("# Findings\nUseful output\n")),
    )
    .unwrap();
    assert!(retry.save.repeated);
    assert_eq!(read_artifact(&store, &key).unwrap().content, "new");
    delete_artifact(&store, "delete", &key).unwrap();
    assert!(
        !root
            .path()
            .join(relative_artifact_path(&key).unwrap())
            .exists()
    );
    assert!(read_artifact(&store, &key).is_err());
    assert!(
        list_session_artifacts(&store, "host", &session.id, None)
            .unwrap()
            .is_empty()
    );
    assert!(store.inventory().is_ok());
    write_artifact(
        &store,
        "host",
        "recreate",
        &intent(&session, Some("recreated")),
    )
    .unwrap();
    assert!(
        delete_artifact(&store, "delete", &key)
            .unwrap()
            .save
            .repeated
    );
    assert_eq!(read_artifact(&store, &key).unwrap().content, "recreated");
}

#[test]
fn reads_and_scoped_lists_pin_io_without_reading_contents_for_descriptions() {
    let (_root, store, session, events) = setup();
    write_artifact(&store, "host", "write", &intent(&session, Some("output"))).unwrap();
    let key = key(&session);
    let session_path = relative_record_path(
        &RecordAddress::new("host", RecordKind::Session, &session.id).unwrap(),
    )
    .unwrap();
    events.lock().unwrap().clear();
    assert_eq!(
        list_session_artifacts(&store, "host", &session.id, None)
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
    assert_eq!(
        list_stream_artifacts(&store, "host", "stream")
            .unwrap()
            .len(),
        1
    );
    let trace = events.lock().unwrap().clone();
    assert_eq!(
        trace
            .iter()
            .filter(|e| matches!(e, RecordIoEvent::List(_)))
            .count(),
        1
    );
    assert_eq!(
        trace
            .iter()
            .filter(|e| matches!(e, RecordIoEvent::Read(_)))
            .count(),
        3
    ); // marker, Stream, producer
    assert!(
        !trace
            .iter()
            .any(|e| matches!(e, RecordIoEvent::Read(path) if path.ends_with("assessment.md")))
    );
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
fn membership_moves_keep_old_outputs_and_cross_session_reads_use_exact_keys() {
    let (_root, store, session, _) = setup();
    write_artifact(&store, "host", "write", &intent(&session, Some("old work"))).unwrap();
    let old_key = key(&session);
    put(
        &store,
        "second-stream",
        Record::Stream(Stream::new("destination", "Other work", None, "host").unwrap()),
    );
    let mut moved = store
        .with_artifacts(|reader| super::session(reader, "host", &session.id))
        .unwrap();
    moved.previous_stream_ids.push(moved.stream_id.clone());
    moved.stream_id = "destination".into();
    put(&store, "move", Record::Session(moved.clone()));
    assert!(
        list_session_artifacts(&store, "host", &session.id, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        list_session_artifacts(&store, "host", &session.id, Some("stream"))
            .unwrap()
            .len(),
        1
    );
    assert_eq!(read_artifact(&store, &old_key).unwrap().content, "old work");
    write_artifact(
        &store,
        "host",
        "new-work",
        &intent(&moved, Some("new work")),
    )
    .unwrap();
    assert_eq!(
        read_artifact(&store, &key(&moved)).unwrap().content,
        "new work"
    );
    assert_eq!(
        list_stream_artifacts(&store, "host", "stream")
            .unwrap()
            .len(),
        1
    );
    let other = Session::new(
        Session::id_for("host", "pi", "reader"),
        "host",
        "destination",
        "pi",
        "reader",
        "Reader",
    )
    .unwrap();
    put(&store, "reader", Record::Session(other.clone()));
    assert!(
        list_session_artifacts(&store, "host", &other.id, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(read_artifact(&store, &old_key).unwrap().content, "old work");
    assert!(store.inventory().is_ok());
}

#[test]
fn refuses_unsafe_names_descriptions_content_and_foreign_writes() {
    let (_root, store, session, _) = setup();
    for name in ["", "../bad", ".", "x/y", "UPPER", "bad name"] {
        let mut write = intent(&session, Some("output"));
        write.name = name.into();
        assert!(write_artifact(&store, "host", name, &write).is_err());
    }
    for description in ["".into(), "two\nlines".into(), "x".repeat(241)] {
        let mut write = intent(&session, Some("output"));
        write.description = description;
        assert!(write_artifact(&store, "host", "description", &write).is_err());
    }
    let text = "x".repeat(artifacts::MAX_CONTENT_BYTES + 1);
    assert!(write_artifact(&store, "host", "large", &intent(&session, Some(&text))).is_err());
    assert!(write_artifact(&store, "host", "no-content", &intent(&session, None)).is_err());
    assert!(
        write_artifact(
            &store,
            "foreign",
            "foreign",
            &intent(&session, Some("output"))
        )
        .is_err()
    );
    let entry = ArtifactEntry {
        name: "ok.md".into(),
        description: "Valid".into(),
    };
    assert!(entry.validate().is_ok());
}

#[test]
fn metadata_save_failure_leaves_unattached_content_and_reports_partial_save() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let (root, store, session, _) = setup();
    let armed = Arc::new(AtomicBool::new(true));
    let trigger = armed.clone();
    let staging = root.path().join(".catalog-staging");
    let blocked = staging.clone();
    let store = store.with_io_observer(move |event| {
        if matches!(event, RecordIoEvent::Written(ref path, _) if path.ends_with("assessment.md"))
            && trigger.swap(false, Ordering::SeqCst)
        {
            fs::remove_dir(&blocked).unwrap();
            fs::write(&blocked, "block next metadata replacement").unwrap();
        }
    });
    let error = write_artifact(
        &store,
        "host",
        "partial",
        &intent(&session, Some("durable output")),
    )
    .err()
    .unwrap();
    assert!(error.to_string().contains("content was saved"));
    assert!(
        list_session_artifacts(&store, "host", &session.id, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(
            root.path()
                .join(relative_artifact_path(&key(&session)).unwrap())
        )
        .unwrap(),
        "durable output"
    );
    assert!(read_artifact(&store, &key(&session)).is_err());
    assert!(
        store.inventory().is_ok(),
        "unattached output is valid transport data"
    );
    fs::remove_file(staging).unwrap();
    write_artifact(
        &store,
        "host",
        "partial",
        &intent(&session, Some("durable output")),
    )
    .unwrap();
    assert_eq!(
        read_artifact(&store, &key(&session)).unwrap().content,
        "durable output"
    );
}

#[test]
fn failed_unlink_keeps_the_reference_deleted_without_following_a_symlink() {
    use std::{
        os::unix::fs::symlink,
        sync::atomic::{AtomicBool, Ordering},
    };
    let (root, store, session, _) = setup();
    write_artifact(&store, "host", "write", &intent(&session, Some("output"))).unwrap();
    let content = root
        .path()
        .join(relative_artifact_path(&key(&session)).unwrap());
    let outside = root.path().join("outside");
    fs::write(&outside, "leave alone").unwrap();
    let session_path = root.path().join(
        relative_record_path(
            &RecordAddress::new("host", RecordKind::Session, &session.id).unwrap(),
        )
        .unwrap(),
    );
    let armed = AtomicBool::new(true);
    let store = store.with_io_observer(move |event| {
        if matches!(event, RecordIoEvent::Written(ref path, _) if path == &session_path)
            && armed.swap(false, Ordering::SeqCst)
        {
            fs::remove_file(&content).unwrap();
            symlink(&outside, &content).unwrap();
        }
    });
    let saved = delete_artifact(&store, "delete", &key(&session)).unwrap();
    assert_eq!(saved.warnings.len(), 1);
    assert!(saved.warnings[0].contains("reference removed"));
    assert!(
        list_session_artifacts(&store, "host", &session.id, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(root.path().join("outside")).unwrap(),
        "leave alone"
    );
}

#[test]
fn source_files_limits_and_reference_encoding_are_explicit() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("source.md");
    fs::write(&path, "assessment\n").unwrap();
    assert_eq!(artifact_file(&path).unwrap(), "assessment\n");
    fs::write(&path, [0xff, 0xfe]).unwrap();
    assert!(artifact_file(&path).is_err());
    fs::write(&path, vec![b'x'; artifacts::MAX_CONTENT_BYTES + 1]).unwrap();
    assert!(artifact_file(&path).is_err());
    fs::write(&path, vec![b'x'; artifacts::MAX_CONTENT_BYTES]).unwrap();
    assert_eq!(
        artifact_file(&path).unwrap().len(),
        artifacts::MAX_CONTENT_BYTES
    );
    assert!(artifact_file(root.path()).is_err());
    let key = ArtifactKey {
        host: "host/name".into(),
        stream: "work/name".into(),
        session: "conversation".into(),
        name: "assessment.md".into(),
    };
    assert_eq!(ArtifactKey::parse(&key.reference()).unwrap(), key);
    assert!(ArtifactKey::parse("host/work/session/../outside").is_err());
    assert!(ArtifactKey::parse("host/work/session/%61ssessment.md").is_err());
}

#[test]
fn session_artifact_groups_reject_positional_entries_duplicates_and_unknown_membership() {
    let (_root, _store, session, _) = setup();
    for artifacts in [
        json!({"stream":[["assessment.md", "description"]]}),
        json!({"stream":[{"name":"assessment.md","description":"one"},{"name":"assessment.md","description":"two"}]}),
        json!({"unknown":[{"name":"assessment.md","description":"one"}]}),
    ] {
        let mut value = serde_json::to_value(&session).unwrap();
        value["artifacts"] = artifacts;
        assert!(crate::codec::record_from_data(RecordKind::Session, &value).is_err());
    }
}
