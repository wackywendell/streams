use crate::support::record_io;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use streams::{
    agent::parse_record_update,
    application::{move_session_observed, save_record_update},
    model::{NamingMode, Record, RecordKind, Session, Stream, TitleDefaults, TitleMode},
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
    json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":id,"kind":"session","actor":actor,"before":before,"changes":{"state":state},"inputs":[]})
}
fn update(store: &YamlRecordStore, value: &Value) -> streams::storage::records::RecordSave {
    update_with_defaults(store, value, TitleDefaults::default())
}
fn update_with_defaults(
    store: &YamlRecordStore,
    value: &Value,
    defaults: TitleDefaults,
) -> streams::storage::records::RecordSave {
    save_record_update(
        store,
        &parse_record_update(&value.to_string(), "source", defaults).unwrap(),
    )
    .unwrap()
}

#[test]
fn session_report_reads_its_current_stream_after_a_valid_move() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = wire("report", &session(), "automatic", "progress");
    move_session_observed(
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
        record_io::accesses(&events.lock().unwrap()),
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
fn report_byte_and_directory_budgets_do_not_grow_with_unrelated_records() {
    for size in [0, 64] {
        for kind in [RecordKind::Session, RecordKind::Stream] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let store = setup(root);
            for index in 0..size {
                std::fs::write(
                    root.join(format!("hosts/host/streams/unrelated-{index}.yaml")),
                    "malformed unrelated record",
                )
                .unwrap();
            }
            let session_key = RecordAddress::for_record(&Record::Session(session()));
            let stream_key = RecordAddress::for_record(&Record::Stream(stream("a")));
            let session_path = root.join(relative_record_path(&session_key).unwrap());
            let stream_path = root.join(relative_record_path(&stream_key).unwrap());
            let marker = root.join(".catalog-local/records.json");
            let bytes_before: usize = [&marker, &session_path, &stream_path]
                .iter()
                .map(|path| std::fs::metadata(path).unwrap().len() as usize)
                .sum();
            let (target, dependency) = if kind == RecordKind::Session {
                (&session_path, &stream_path)
            } else {
                (&stream_path, &session_path)
            };
            let prose = "x".repeat(64 * 1024);
            let request = if kind == RecordKind::Session {
                wire("report", &session(), "automatic", &prose)
            } else {
                json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":"report","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal":prose},"inputs":[{"kind":"session","record":session()}]})
            };
            let events = Arc::new(Mutex::new(Vec::new()));
            let observed = events.clone();
            let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
            update(&store, &request);
            let trace = events.lock().unwrap().clone();
            assert_eq!(
                record_io::accesses(&trace),
                [
                    RecordIoEvent::Lock(root.join(".catalog.lock")),
                    RecordIoEvent::Read(marker.clone()),
                    RecordIoEvent::Read(target.clone()),
                    RecordIoEvent::Read(dependency.clone())
                ]
            );
            assert_eq!(record_io::bytes_read(&trace), bytes_before);
            let written = std::fs::metadata(target).unwrap().len() as usize;
            assert_eq!(record_io::writes(&trace), [(target.clone(), written)]);

            events.lock().unwrap().clear();
            assert!(update(&store, &request).repeated);
            let trace = events.lock().unwrap().clone();
            assert_eq!(
                record_io::accesses(&trace),
                [
                    RecordIoEvent::Lock(root.join(".catalog.lock")),
                    RecordIoEvent::Read(marker.clone()),
                    RecordIoEvent::Read(target.clone())
                ]
            );
            assert_eq!(
                record_io::bytes_read(&trace),
                std::fs::metadata(&marker).unwrap().len() as usize + written
            );
            assert!(record_io::writes(&trace).is_empty());
        }
    }
}

#[test]
fn stream_report_checks_membership_period_without_retargeting_old_prose() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":"report","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal":"old prose"},"inputs":[{"kind":"session","record":session()}]});
    move_session_observed(
        &store,
        "out",
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    move_session_observed(
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
fn report_receipts_keep_work_text_only_in_the_current_record() {
    fn saved_bytes(payload: &str) -> Vec<u8> {
        let directory = tempfile::tempdir().unwrap();
        let store = setup(directory.path());
        let mut current = session();
        for index in 0..16 {
            let saved = update(
                &store,
                &wire(
                    &format!("report-{index}"),
                    &current,
                    "user",
                    &format!("{index}: {payload}"),
                ),
            );
            assert_eq!(
                saved.outcome,
                json!({"outcome":"changes","conflicts":[],"reason":""})
                    .as_object()
                    .unwrap()
                    .clone()
            );
            let Record::Session(session) = saved.current.record else {
                unreachable!()
            };
            current = session;
        }
        let target = RecordAddress::for_record(&Record::Session(current));
        let bytes = std::fs::read(
            directory
                .path()
                .join(relative_record_path(&target).unwrap()),
        )
        .unwrap();
        let envelope: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(envelope["receipts"].as_object().unwrap().len(), 17);
        bytes
    }
    let small = saved_bytes("");
    let payload = "x".repeat(64 * 1024);
    let large = saved_bytes(&payload);
    assert_eq!(large.len() - small.len(), payload.len());
}

#[test]
fn no_change_and_refused_report_retries_retain_only_the_original_decision() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let unchanged = wire("unchanged", &session(), "user", "");
    let no_change = update(&store, &unchanged);
    assert_eq!(no_change.outcome["outcome"], "no-change");
    update(&store, &wire("newer", &session(), "user", "current"));
    for (actor, expected) in [("user", "conflict"), ("automatic", "superseded")] {
        let request = wire(actor, &session(), actor, "stale proposal");
        let first = update(&store, &request);
        assert_eq!(first.outcome["outcome"], expected);
        assert!(!first.outcome["conflicts"].as_array().unwrap().is_empty());
        assert!(!first.outcome["reason"].as_str().unwrap().is_empty());
        assert_eq!(first.outcome.len(), 3);
        let repeated = update(&store, &request);
        assert!(repeated.repeated);
        assert_eq!(repeated.outcome, first.outcome);
        assert_eq!(repeated.current, first.current);
    }
    let repeated = update(&store, &unchanged);
    assert!(repeated.repeated);
    assert_eq!(repeated.outcome, no_change.outcome);
    let Record::Session(current) = repeated.current.record else {
        unreachable!()
    };
    assert_eq!(current.state, "current");
}

#[test]
fn accepted_report_retry_after_later_edits_reads_only_target_and_retains_original_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut original = wire("report", &session(), "automatic", "accepted");
    original["changes"]["title"] = json!("Accepted generated name");
    let first = update(&store, &original);
    let Record::Session(before) = &first.current.record else {
        unreachable!()
    };
    update(&store, &wire("newer", before, "user", "current work"));
    move_session_observed(
        &store,
        "move",
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
    let repeated = update_with_defaults(
        &store,
        &original,
        TitleDefaults {
            stream: NamingMode::Manual,
            session: NamingMode::Manual,
        },
    );
    assert!(repeated.repeated);
    assert_eq!(repeated.outcome, first.outcome);
    let Record::Session(current) = repeated.current.record else {
        unreachable!()
    };
    assert_eq!(current.state, "current work");
    assert_eq!(current.title, "Accepted generated name");
    assert_eq!(current.stream_id, "b");
    assert_eq!(record_io::accesses(&events.lock().unwrap()).len(), 3);
    let mut changed_default = original.clone();
    changed_default["title_default"] = json!("manual");
    assert!(
        save_record_update(
            &store,
            &parse_record_update(
                &changed_default.to_string(),
                "source",
                TitleDefaults::default()
            )
            .unwrap()
        )
        .is_err()
    );
    let mut changed = original;
    changed["changes"]["state"] = json!("different intent");
    assert!(
        save_record_update(
            &store,
            &parse_record_update(
                &changed.to_string(),
                "source",
                streams::model::TitleDefaults::default()
            )
            .unwrap()
        )
        .is_err()
    );
}

#[test]
fn stale_never_applied_inherited_title_is_superseded_but_unrelated_progress_is_not() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut request = wire("stale-title", &session(), "automatic", "captured progress");
    request["changes"]["title"] = json!("Stale generated name");
    let manual = TitleDefaults {
        session: NamingMode::Manual,
        ..TitleDefaults::default()
    };
    let result = update_with_defaults(&store, &request, manual);
    assert_eq!(result.outcome["outcome"], "superseded");
    assert_eq!(result.current.record, Record::Session(session()));
    let progress = update_with_defaults(
        &store,
        &wire("progress", &session(), "automatic", "fresh work"),
        manual,
    );
    assert_eq!(progress.outcome["outcome"], "changes");
    let Record::Session(current) = progress.current.record else {
        unreachable!()
    };
    assert_eq!(current.state, "fresh work");
    assert_eq!(current.title, "Work");
}

#[test]
fn accepted_stream_title_retry_after_default_drift_and_move_reads_only_target() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":"title","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"title":"Generated effort"},"inputs":[{"kind":"session","record":session()}]});
    let first = update(&store, &request);
    move_session_observed(
        &store,
        "move",
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
    let repeated = update_with_defaults(
        &store,
        &request,
        TitleDefaults {
            stream: NamingMode::Manual,
            ..TitleDefaults::default()
        },
    );
    assert!(repeated.repeated);
    assert_eq!(repeated.outcome, first.outcome);
    assert_eq!(repeated.current.record, first.current.record);
    assert_eq!(record_io::accesses(&events.lock().unwrap()).len(), 3);
}

#[test]
fn explicit_title_modes_do_not_depend_on_changed_global_defaults() {
    for mode in [TitleMode::Automatic, TitleMode::Manual] {
        let directory = tempfile::tempdir().unwrap();
        let store = setup(directory.path());
        let policy = update(
            &store,
            &json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":"mode","kind":"session","actor":"user","before":session(),"changes":{"title_mode":mode},"inputs":[]}),
        );
        let Record::Session(before) = policy.current.record else {
            unreachable!()
        };
        let mut request = wire("report", &before, "automatic", "progress");
        request["changes"]["title"] = json!("Generated title");
        let result = update_with_defaults(
            &store,
            &request,
            TitleDefaults {
                session: NamingMode::Manual,
                ..TitleDefaults::default()
            },
        );
        assert_eq!(result.outcome["outcome"], "changes");
        let Record::Session(current) = result.current.record else {
            unreachable!()
        };
        assert_eq!(
            current.title,
            if mode == TitleMode::Automatic {
                "Generated title"
            } else {
                "Work"
            }
        );
    }
}

#[test]
fn request_schema_rejects_old_tokens_unversioned_requests_and_rosters() {
    let valid = wire("report", &session(), "automatic", "work");
    assert!(
        parse_record_update(
            &valid.to_string(),
            "source",
            streams::model::TitleDefaults::default()
        )
        .is_ok()
    );
    let mut missing = valid.clone();
    missing.as_object_mut().unwrap().remove("version");
    let mut missing_default = valid.clone();
    missing_default
        .as_object_mut()
        .unwrap()
        .remove("title_default");
    let mut inherited_default = valid.clone();
    inherited_default["title_default"] = json!("inherit");
    let mut old_token = valid.clone();
    old_token["ownership_token"] = json!("old");
    let mut old_authority = valid.clone();
    old_authority["authority"] = Value::Null;
    let mut old_version = valid.clone();
    old_version["version"] = json!(1);
    let mut dependency = valid.clone();
    dependency["inputs"] = json!([{"kind":"stream","record":stream("a")}]);
    for invalid in [
        missing,
        missing_default,
        inherited_default,
        old_token,
        old_authority,
        old_version,
        dependency,
    ] {
        assert!(
            parse_record_update(
                &invalid.to_string(),
                "source",
                streams::model::TitleDefaults::default()
            )
            .is_err()
        );
    }
    assert!(
        parse_record_update(
            &valid.to_string(),
            "changed-source",
            streams::model::TitleDefaults::default()
        )
        .is_err()
    );
    let roster = json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":"report","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal":"prose"},"inputs":[{"kind":"session","record":session()},{"kind":"session","record":session()}]});
    assert!(
        parse_record_update(
            &roster.to_string(),
            "source",
            streams::model::TitleDefaults::default()
        )
        .is_err()
    );
}

#[test]
fn manual_stream_edit_has_no_session_dependency_even_after_membership_changes() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":"manual","kind":"stream","actor":"user","before":stream("a"),"changes":{"goal":"manual goal"},"inputs":[]});
    let mut invalid = request.clone();
    invalid["inputs"] = json!([{"kind":"session","record":session()}]);
    assert!(
        parse_record_update(
            &invalid.to_string(),
            "source",
            streams::model::TitleDefaults::default()
        )
        .is_err()
    );
    move_session_observed(
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
        record_io::accesses(&events.lock().unwrap()),
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
        &json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":"rename","kind":"session","actor":"user","before":initial,"changes":{"title":"Human name"},"inputs":[]}),
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
            &parse_record_update(
                &request.to_string(),
                "source",
                streams::model::TitleDefaults::default()
            )
            .unwrap()
        )
        .is_err()
    );
}
