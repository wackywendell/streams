use super::*;
use crate::{
    model::{Session, Stream},
    storage::StorageError,
    yaml_store::records::YamlRecordStore,
};
fn request(id: &str, stream: Option<&str>) -> EnrollmentRequest {
    let mut wire = json!({"version":2,"catalog_source":"source","operation_id":id,"harness":"pi","harness_session_id":"native","session":{"title":"Conversation"},"actor":"automatic","cwd":"/fixture"});
    if let Some(id) = stream {
        wire["stream_id"] = json!(id);
    } else {
        wire["stream"] = json!({"title":"Effort","goal":"Goal"});
    }
    EnrollmentRequest::parse(&wire.to_string(), "source").unwrap()
}
fn read_session(store: &YamlRecordStore) -> Option<Session> {
    store
        .with_reader::<_, StorageError>(|reader| {
            Ok(reader
                .read(
                    &RecordAddress::new(
                        "host",
                        RecordKind::Session,
                        Session::id_for("host", "pi", "native"),
                    )
                    .unwrap(),
                )?
                .map(|read| match read.record {
                    Record::Session(session) => session,
                    _ => unreachable!(),
                }))
        })
        .unwrap()
}
fn put_stream(store: &YamlRecordStore, stream: Stream) {
    let record = Record::Stream(stream);
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
                outcome: Default::default(),
            })
        })
        .unwrap();
}
#[test]
fn interrupted_enrollment_leaves_only_its_effort_and_retries_each_record_independently() {
    let directory = tempfile::tempdir().unwrap();
    let store = YamlRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    let request = request("enroll", None);
    let stream = enroll_stream(&store, "host", &request).unwrap();
    assert!(!stream.repeated);
    assert!(read_session(&store).is_none());
    assert!(enroll_stream(&store, "host", &request).unwrap().repeated);
    let session = enroll_session(&store, "host", &request).unwrap();
    assert!(!session.repeated);
    assert_eq!(
        read_session(&store).unwrap().stream_id,
        effort_id_for("host", request.harness(), "native")
    );
    assert!(enroll_session(&store, "host", &request).unwrap().repeated);
    assert_eq!(
        enroll_stream(&store, "host", &request).unwrap().outcome,
        stream.outcome
    );
}
#[test]
fn existing_enrollment_does_not_edit_text_or_replace_membership() {
    let directory = tempfile::tempdir().unwrap();
    let store = YamlRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    let initial = request("initial", None);
    enroll_stream(&store, "host", &initial).unwrap();
    enroll_session(&store, "host", &initial).unwrap();
    let id = read_session(&store).unwrap().id;
    put_stream(&store, Stream::new("b", "B", None, "host").unwrap());
    crate::application::move_session(
        &store,
        "move",
        &crate::operations::membership::MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    let fresh = request("fresh", None);
    let saved = enroll_session(&store, "host", &fresh).unwrap();
    assert_eq!(saved.outcome["outcome"], "no-change");
    let current = read_session(&store).unwrap();
    assert_eq!(current.id, id);
    assert_eq!(current.stream_id, "b");
    assert_eq!(current.title, "Conversation");
    assert!(enroll_session(&store, "host", &request("other", Some("missing"))).is_err());
    let repeated = enroll_session(&store, "host", &initial).unwrap();
    assert!(repeated.repeated);
    let Record::Session(current) = repeated.current.record else {
        unreachable!()
    };
    assert_eq!(current.stream_id, "b");
}
#[test]
fn missing_stream_changed_intent_and_unrelated_existing_effort_refuse_without_implicit_moves() {
    let directory = tempfile::tempdir().unwrap();
    let store = YamlRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    assert!(enroll_session(&store, "host", &request("missing", Some("missing"))).is_err());
    assert!(read_session(&store).is_none());
    let initial = request("initial", None);
    enroll_stream(&store, "host", &initial).unwrap();
    assert!(enroll_stream(&store, "host", &request("different invocation", None)).is_err());
    let changed=EnrollmentRequest::parse(&json!({"version":2,"catalog_source":"source","operation_id":"initial","harness":"pi","harness_session_id":"native","stream":{"title":"Changed"},"session":{"title":"Conversation"},"actor":"automatic","cwd":"/fixture"}).to_string(),"source").unwrap();
    assert!(enroll_stream(&store, "host", &changed).is_err());
    assert!(read_session(&store).is_none());
}
#[test]
fn enrollment_requests_require_version_two_and_reject_old_permission_fields() {
    let wire = json!({"version":2,"catalog_source":"source","operation_id":"initial","harness":"pi","harness_session_id":"native","stream":{"title":"Effort"},"session":{"title":"Conversation"},"actor":"automatic","cwd":"/fixture"});
    let mut old = wire.clone();
    old["version"] = json!(1);
    assert!(EnrollmentRequest::parse(&old.to_string(), "source").is_err());
    old = wire;
    old["authority"] = json!({"host_id":"host","token":"old"});
    assert!(EnrollmentRequest::parse(&old.to_string(), "source").is_err());
}
