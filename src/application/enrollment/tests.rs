use super::*;
use crate::{
    file_store::records::FileRecordStore,
    model::{Session, Stream},
    storage::StorageError,
};
use serde_json::json;
fn request(stream: Option<&str>) -> EnrollmentRequest {
    let mut wire = json!({"version":2,"catalog_source":"source","harness":"pi","harness_session_id":"native","session":{"title":"Conversation"},"actor":"automatic","cwd":"/fixture"});
    if let Some(id) = stream {
        wire["stream_id"] = json!(id);
    } else {
        wire["stream"] = json!({"title":"Effort","goal":"Goal"});
    }
    EnrollmentRequest::parse(wire.clone(), "source").unwrap()
}
fn read_session(store: &FileRecordStore) -> Option<Session> {
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
                .map(|read| match read {
                    Record::Session(session) => session,
                    _ => unreachable!(),
                }))
        })
        .unwrap()
}
fn put_stream(store: &FileRecordStore, stream: Stream) {
    let record = Record::Stream(stream);
    store
        .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
            Ok(RecordChange {
                record,
                outcome: (),
            })
        })
        .unwrap();
}
#[test]
fn interrupted_enrollment_leaves_only_its_effort_and_retries_each_record_independently() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    let request = request(None);
    let stream = enroll_stream(&store, "host", &request).unwrap();
    assert!(stream.written);
    assert_eq!(stream.outcome, DomainOutcome::Changes);
    assert!(read_session(&store).is_none());
    let retry = enroll_stream(&store, "host", &request).unwrap();
    assert!(!retry.written);
    assert_eq!(retry.outcome, DomainOutcome::NoChange);
    let session = enroll_session_observed(&store, "host", &request).unwrap();
    assert!(session.save.written);
    assert_eq!(
        session.stream.id,
        effort_id_for("host", request.harness(), "native")
    );
    assert_eq!(
        read_session(&store).unwrap().stream_id,
        effort_id_for("host", request.harness(), "native")
    );
    let repeated = enroll_session_observed(&store, "host", &request).unwrap();
    assert!(!repeated.save.written);
    assert_eq!(repeated.save.outcome, DomainOutcome::NoChange);
}
#[test]
fn existing_enrollment_does_not_edit_text_or_replace_membership() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    let initial = request(None);
    enroll_stream(&store, "host", &initial).unwrap();
    enroll_session(&store, "host", &initial).unwrap();
    let id = read_session(&store).unwrap().id;
    put_stream(&store, Stream::new("b", "B", None, "host").unwrap());
    crate::application::move_session_observed(
        &store,
        &crate::operations::membership::MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    let saved = enroll_session(&store, "host", &initial).unwrap();
    assert_eq!(saved.outcome, DomainOutcome::NoChange);
    assert!(!saved.written);
    let current = read_session(&store).unwrap();
    assert_eq!(current.id, id);
    assert_eq!(current.stream_id, "b");
    assert_eq!(current.title, "Conversation");
    assert!(enroll_session(&store, "host", &request(Some("missing"))).is_err());
    let observed = enroll_session_observed(&store, "host", &initial).unwrap();
    assert_eq!(observed.stream.id, "b");
    let Record::Session(current) = observed.save.current else {
        unreachable!()
    };
    assert_eq!(current.stream_id, "b");
}
#[test]
fn enrolled_session_size_is_independent_of_stream_prose() {
    fn session_bytes(goal: &str) -> Vec<u8> {
        let directory = tempfile::tempdir().unwrap();
        let store = FileRecordStore::new(directory.path(), "host").unwrap();
        store.initialize().unwrap();
        let mut stream = Stream::new("a", "A", None, "host").unwrap();
        stream.goal = Some(goal.into());
        put_stream(&store, stream);
        let observed = enroll_session_observed(&store, "host", &request(Some("a"))).unwrap();
        assert_eq!(observed.stream.goal.as_deref(), Some(goal));
        let saved = observed.save;
        assert_eq!(saved.outcome, DomainOutcome::Changes);
        let path = crate::file_store::records::relative_record_path(&RecordAddress::for_record(
            &saved.current,
        ))
        .unwrap();
        std::fs::read(directory.path().join(path)).unwrap()
    }
    assert_eq!(
        session_bytes("Goal").len(),
        session_bytes(&"x".repeat(64 * 1024)).len()
    );
}

#[test]
fn missing_stream_changed_intent_and_unrelated_existing_effort_refuse_without_implicit_moves() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    assert!(enroll_session(&store, "host", &request(Some("missing"))).is_err());
    assert!(read_session(&store).is_none());
    let initial = request(None);
    enroll_stream(&store, "host", &initial).unwrap();
    let changed=EnrollmentRequest::parse(json!({"version":2,"catalog_source":"source","harness":"pi","harness_session_id":"native","stream":{"title":"Changed"},"session":{"title":"Conversation"},"actor":"automatic","cwd":"/fixture"}),"source").unwrap();
    assert!(enroll_stream(&store, "host", &changed).is_err());
    assert!(read_session(&store).is_none());
}
#[test]
fn enrollment_requests_require_version_two_and_reject_old_permission_fields() {
    let wire = json!({"version":2,"catalog_source":"source","harness":"pi","harness_session_id":"native","stream":{"title":"Effort"},"session":{"title":"Conversation"},"actor":"automatic","cwd":"/fixture"});
    let mut old = wire.clone();
    old["version"] = json!(1);
    assert!(EnrollmentRequest::parse(old.clone(), "source").is_err());
    old = wire.clone();
    old["authority"] = json!({"host_id":"host","token":"old"});
    assert!(EnrollmentRequest::parse(old.clone(), "source").is_err());
    old = wire.clone();
    old["operation_id"] = json!("old");
    assert!(EnrollmentRequest::parse(old.clone(), "source").is_err());
}
