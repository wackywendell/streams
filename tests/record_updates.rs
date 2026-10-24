use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use streams_temper::{
    agent::parse_record_update,
    application::{move_session, save_record_update},
    model::{Record, RecordKind, Session, Stream},
    operations::membership::MoveIntent,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
    },
    yaml_store::records::{RecordIoEvent, YamlRecordStore, relative_record_path},
};

fn session() -> Session {
    Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "a",
        "pi",
        "native",
        "Work",
    )
    .unwrap()
}
fn stream(id: &str) -> Stream {
    Stream::new(id, id, None, "host").unwrap()
}
fn put(store: &YamlRecordStore, record: Record) {
    let request = RecordRequest::new(
        RecordAddress::for_record(&record),
        "setup",
        &json!({"record":record}),
    )
    .unwrap();
    store
        .save::<StorageError>(&request, |_, _| {
            Ok(RecordChange {
                record,
                outcome: serde_json::Map::new(),
            })
        })
        .unwrap();
}
fn setup(root: &std::path::Path) -> YamlRecordStore {
    let store = YamlRecordStore::new(root, "host").unwrap();
    store.initialize().unwrap();
    put(&store, Record::Stream(stream("a")));
    put(&store, Record::Stream(stream("b")));
    put(&store, Record::Session(session()));
    store
}
fn wire(id: &str, before: &Session, actor: &str, state: &str) -> Value {
    json!({"version":2,"catalog_source":"source","operation_id":id,"kind":"session","actor":actor,"before":before,"changes":{"state":state},"inputs":[]})
}
fn update(store: &YamlRecordStore, value: &Value) -> streams_temper::storage::records::RecordSave {
    save_record_update(
        store,
        &parse_record_update(&value.to_string(), "source").unwrap(),
    )
    .unwrap()
}

#[test]
fn session_report_reads_its_current_stream_after_a_valid_move() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = wire("report", &session(), "automatic", "progress");
    move_session(
        &store,
        "move",
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
    let result = update(&store, &request);
    let Record::Session(current) = result.current.record else {
        unreachable!()
    };
    assert_eq!(current.stream_id, "b");
    assert_eq!(current.previous_stream_ids, ["a"]);
    assert_eq!(current.state, "progress");
    assert_eq!(result.outcome["outcome"], "changes");
    let target = RecordAddress::new("host", RecordKind::Session, current.id).unwrap();
    let dependency = RecordAddress::new("host", RecordKind::Stream, "b").unwrap();
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            RecordIoEvent::Lock(directory.path().join(".catalog.lock")),
            RecordIoEvent::Read(directory.path().join(".catalog-local/records.json")),
            RecordIoEvent::Read(
                directory
                    .path()
                    .join(relative_record_path(&target).unwrap())
            ),
            RecordIoEvent::Read(
                directory
                    .path()
                    .join(relative_record_path(&dependency).unwrap())
            ),
        ]
    );
}

#[test]
fn stream_report_checks_membership_period_without_retargeting_old_prose() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = json!({"version":2,"catalog_source":"source","operation_id":"report","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal":"old prose"},"inputs":[{"kind":"session","record":session()}]});
    move_session(
        &store,
        "out",
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    move_session(
        &store,
        "back",
        &MoveIntent::new("host", "pi", "native", "a").unwrap(),
    )
    .unwrap();
    let result = update(&store, &request);
    assert_eq!(result.outcome["outcome"], "superseded");
    assert_eq!(result.current.record, Record::Stream(stream("a")));
    let destination = RecordAddress::new("host", RecordKind::Stream, "b").unwrap();
    assert_eq!(
        store
            .with_reader::<_, StorageError>(|reader| reader.read(&destination))
            .unwrap()
            .unwrap()
            .record,
        Record::Stream(stream("b"))
    );
}

#[test]
fn accepted_report_retry_after_later_edits_reads_only_target_and_retains_original_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let original = wire("report", &session(), "automatic", "accepted");
    let first = update(&store, &original);
    let Record::Session(before) = &first.current.record else {
        unreachable!()
    };
    update(&store, &wire("newer", before, "user", "current work"));
    move_session(
        &store,
        "move",
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
    let repeated = update(&store, &original);
    assert!(repeated.repeated);
    assert_eq!(repeated.outcome, first.outcome);
    let Record::Session(current) = repeated.current.record else {
        unreachable!()
    };
    assert_eq!(current.state, "current work");
    assert_eq!(current.stream_id, "b");
    assert_eq!(events.lock().unwrap().len(), 3);
    let mut changed = original;
    changed["changes"]["state"] = json!("different intent");
    assert!(
        save_record_update(
            &store,
            &parse_record_update(&changed.to_string(), "source").unwrap()
        )
        .is_err()
    );
}

#[test]
fn request_schema_rejects_old_tokens_unversioned_requests_and_rosters() {
    let valid = wire("report", &session(), "automatic", "work");
    assert!(parse_record_update(&valid.to_string(), "source").is_ok());
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("version");
    let mut old_token = valid.clone();
    old_token["ownership_token"] = json!("old");
    let mut old_authority = valid.clone();
    old_authority["authority"] = Value::Null;
    let mut old_version = valid.clone();
    old_version["version"] = json!(1);
    let mut dependency = valid.clone();
    dependency["inputs"] = json!([{"kind":"stream","record":stream("a")}]);
    for invalid in [missing, old_token, old_authority, old_version, dependency] {
        assert!(parse_record_update(&invalid.to_string(), "source").is_err());
    }
    assert!(parse_record_update(&valid.to_string(), "changed-source").is_err());
    let roster = json!({"version":2,"catalog_source":"source","operation_id":"report","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal":"prose"},"inputs":[{"kind":"session","record":session()},{"kind":"session","record":session()}]});
    assert!(parse_record_update(&roster.to_string(), "source").is_err());
}

#[test]
fn manual_stream_edit_has_no_session_dependency_even_after_membership_changes() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = json!({"version":2,"catalog_source":"source","operation_id":"manual","kind":"stream","actor":"user","before":stream("a"),"changes":{"goal":"manual goal"},"inputs":[]});
    let mut invalid = request.clone();
    invalid["inputs"] = json!([{"kind":"session","record":session()}]);
    assert!(parse_record_update(&invalid.to_string(), "source").is_err());
    move_session(
        &store,
        "move",
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
    let result = update(&store, &request);
    assert_eq!(result.outcome["outcome"], "changes");
    let Record::Stream(current) = result.current.record else {
        unreachable!()
    };
    assert_eq!(current.goal.as_deref(), Some("manual goal"));
    assert_eq!(
        *events.lock().unwrap(),
        vec![
            RecordIoEvent::Lock(directory.path().join(".catalog.lock")),
            RecordIoEvent::Read(directory.path().join(".catalog-local/records.json")),
            RecordIoEvent::Read(
                directory.path().join(
                    relative_record_path(
                        &RecordAddress::new("host", RecordKind::Stream, "a").unwrap()
                    )
                    .unwrap()
                )
            ),
        ]
    );
}

#[test]
fn declined_manual_text_is_still_part_of_the_retry_fingerprint() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let initial = session();
    let renamed = update(
        &store,
        &json!({"version":2,"catalog_source":"source","operation_id":"rename","kind":"session","actor":"user","before":initial,"changes":{"title":"Human name"},"inputs":[]}),
    );
    let Record::Session(before) = renamed.current.record else {
        unreachable!()
    };
    let mut request = wire("report", &before, "automatic", "progress");
    request["changes"]["title"] = json!("ignored title");
    update(&store, &request);
    request["changes"]["title"] = json!("different ignored title");
    assert!(
        save_record_update(
            &store,
            &parse_record_update(&request.to_string(), "source").unwrap()
        )
        .is_err()
    );
}
