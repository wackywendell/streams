use crate::support::record_io;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use streams::{
    agent::parse_record_update,
    application::{move_session_observed, save_record_update},
    file_store::records::{FileRecordStore, RecordIoEvent, relative_record_path},
    model::{NamingMode, Record, RecordKind, Session, Stream, TitleDefaults, TitleMode},
    operations::{DomainOutcome, EditOutcome, membership::MoveIntent},
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordSave, RecordStore},
    },
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
/// The saved record, with the revision a fresh capture would carry.
fn stored(store: &FileRecordStore, record: Record) -> Record {
    store
        .with_reader::<_, StorageError>(|reader| reader.read(&RecordAddress::for_record(&record)))
        .unwrap()
        .unwrap()
        .record
}
fn stored_session(store: &FileRecordStore) -> Session {
    let Record::Session(session) = stored(store, Record::Session(session())) else {
        unreachable!()
    };
    session
}
fn stored_stream(store: &FileRecordStore, id: &str) -> Stream {
    let Record::Stream(stream) = stored(store, Record::Stream(stream(id))) else {
        unreachable!()
    };
    stream
}
fn setup(root: &std::path::Path) -> FileRecordStore {
    let store = FileRecordStore::new(root, "host").unwrap();
    store.initialize().unwrap();
    put(&store, Record::Stream(stream("a")));
    put(&store, Record::Stream(stream("b")));
    put(&store, Record::Session(session()));
    store
}
fn wire(before: &Session, actor: &str, state: &str) -> Value {
    json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"session","actor":actor,"before":before,"changes":{"state":state},"inputs":[]})
}
fn update(store: &FileRecordStore, value: &Value) -> RecordSave<EditOutcome> {
    update_with_defaults(store, value, TitleDefaults::default())
}
fn update_with_defaults(
    store: &FileRecordStore,
    value: &Value,
    defaults: TitleDefaults,
) -> RecordSave<EditOutcome> {
    save_record_update(
        store,
        &parse_record_update(value.clone(), "source", defaults).unwrap(),
    )
    .unwrap()
}

#[test]
fn session_report_reads_its_current_stream_after_a_valid_move() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = wire(&session(), "automatic", "progress");
    move_session_observed(
        &store,
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
    assert_eq!(current.membership_epoch, 1);
    assert_eq!(current.state, "progress");
    assert_eq!(result.outcome.outcome, DomainOutcome::Changes);
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
                    root.join(format!("hosts/host/streams/unrelated-{index}.json")),
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
                wire(&session(), "automatic", &prose)
            } else {
                json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal":prose},"inputs":[{"kind":"session","record":session()}]})
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

            // A retry that already landed reads the same records and writes nothing.
            events.lock().unwrap().clear();
            let retry = update(&store, &request);
            assert!(!retry.written);
            assert_eq!(retry.outcome.outcome, DomainOutcome::NoChange);
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
            let bytes_now: usize = [&marker, &session_path, &stream_path]
                .iter()
                .map(|path| std::fs::metadata(path).unwrap().len() as usize)
                .sum();
            assert_eq!(record_io::bytes_read(&trace), bytes_now);
            assert!(record_io::writes(&trace).is_empty());
        }
    }
}

#[test]
fn stream_report_checks_membership_period_without_retargeting_old_prose() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal":"old prose"},"inputs":[{"kind":"session","record":session()}]});
    move_session_observed(
        &store,
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    move_session_observed(
        &store,
        &MoveIntent::new("host", "pi", "native", "a").unwrap(),
    )
    .unwrap();
    let result = update(&store, &request);
    assert_eq!(result.outcome.outcome, DomainOutcome::Superseded);
    assert!(
        result
            .current
            .record
            .same_content(&Record::Stream(stream("a")))
    );
    let destination = RecordAddress::new("host", RecordKind::Stream, "b").unwrap();
    assert!(
        store
            .with_reader::<_, StorageError>(|reader| reader.read(&destination))
            .unwrap()
            .unwrap()
            .record
            .same_content(&Record::Stream(stream("b")))
    );
}

#[test]
fn saved_records_hold_only_the_current_work_text() {
    fn saved_bytes(payload: &str) -> Vec<u8> {
        let directory = tempfile::tempdir().unwrap();
        let store = setup(directory.path());
        let mut current = stored_session(&store);
        for index in 0..16 {
            let saved = update(
                &store,
                &wire(&current, "user", &format!("{index}: {payload}")),
            );
            assert_eq!(
                json!(saved.outcome),
                json!({"outcome":"changes","conflicts":[],"reason":""})
            );
            let Record::Session(session) = saved.current.record else {
                unreachable!()
            };
            current = session;
        }
        assert_eq!(current.revision, 17);
        let target = RecordAddress::for_record(&Record::Session(current.clone()));
        let bytes = std::fs::read(
            directory
                .path()
                .join(relative_record_path(&target).unwrap()),
        )
        .unwrap();
        let file: Session = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(file, current);
        bytes
    }
    let small = saved_bytes("");
    let payload = "x".repeat(64 * 1024);
    let large = saved_bytes(&payload);
    assert_eq!(large.len() - small.len(), payload.len());
}

#[test]
fn no_change_and_refused_report_retries_decide_again_without_writing() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let unchanged = wire(&session(), "user", "");
    let no_change = update(&store, &unchanged);
    assert_eq!(no_change.outcome.outcome, DomainOutcome::NoChange);
    assert!(!no_change.written);
    update(&store, &wire(&stored_session(&store), "user", "current"));
    for (actor, expected) in [
        ("user", DomainOutcome::Conflict),
        ("automatic", DomainOutcome::Superseded),
    ] {
        let request = wire(&session(), actor, "stale proposal");
        let first = update(&store, &request);
        assert_eq!(first.outcome.outcome, expected);
        assert!(!first.outcome.conflicts.is_empty());
        assert!(!first.outcome.reason.is_empty());
        assert!(!first.written);
        let repeated = update(&store, &request);
        assert!(!repeated.written);
        assert_eq!(repeated.outcome, first.outcome);
        assert_eq!(repeated.current, first.current);
    }
    let repeated = update(&store, &unchanged);
    assert!(!repeated.written);
    let Record::Session(current) = repeated.current.record else {
        unreachable!()
    };
    assert_eq!(current.state, "current");
}

#[test]
fn accepted_report_retry_after_later_edits_keeps_them() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut original = wire(&session(), "automatic", "accepted");
    original["changes"]["title"] = json!("Accepted generated name");
    let first = update(&store, &original);
    let Record::Session(before) = &first.current.record else {
        unreachable!()
    };
    update(&store, &wire(before, "user", "current work"));
    move_session_observed(
        &store,
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
    // The retry finds later work in place and leaves it there.
    assert!(!repeated.written);
    assert_eq!(repeated.outcome.outcome, DomainOutcome::Superseded);
    let Record::Session(current) = repeated.current.record else {
        unreachable!()
    };
    assert_eq!(current.state, "current work");
    assert_eq!(current.title, "Accepted generated name");
    assert_eq!(current.stream_id, "b");
    assert!(record_io::writes(&events.lock().unwrap()).is_empty());
}

#[test]
fn stale_never_applied_inherited_title_is_superseded_but_unrelated_progress_is_not() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut request = wire(&session(), "automatic", "captured progress");
    request["changes"]["title"] = json!("Stale generated name");
    let manual = TitleDefaults {
        session: NamingMode::Manual,
        ..TitleDefaults::default()
    };
    let result = update_with_defaults(&store, &request, manual);
    assert_eq!(result.outcome.outcome, DomainOutcome::Superseded);
    assert!(
        result
            .current
            .record
            .same_content(&Record::Session(session()))
    );
    let progress =
        update_with_defaults(&store, &wire(&session(), "automatic", "fresh work"), manual);
    assert_eq!(progress.outcome.outcome, DomainOutcome::Changes);
    let Record::Session(current) = progress.current.record else {
        unreachable!()
    };
    assert_eq!(current.state, "fresh work");
    assert_eq!(current.title, "Work");
}

#[test]
fn accepted_stream_title_retry_after_default_drift_and_move_is_no_change() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let request = json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"title":"Generated effort"},"inputs":[{"kind":"session","record":session()}]});
    let first = update(&store, &request);
    move_session_observed(
        &store,
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
    assert!(!repeated.written);
    assert_eq!(repeated.outcome.outcome, DomainOutcome::NoChange);
    assert_eq!(repeated.current.record, first.current.record);
    assert!(record_io::writes(&events.lock().unwrap()).is_empty());
}

#[test]
fn explicit_title_modes_do_not_depend_on_changed_global_defaults() {
    for mode in [TitleMode::Automatic, TitleMode::Manual] {
        let directory = tempfile::tempdir().unwrap();
        let store = setup(directory.path());
        let policy = update(
            &store,
            &json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"session","actor":"user","before":stored_session(&store),"changes":{"title_mode":mode},"inputs":[]}),
        );
        let Record::Session(before) = policy.current.record else {
            unreachable!()
        };
        let mut request = wire(&before, "automatic", "progress");
        request["changes"]["title"] = json!("Generated title");
        let result = update_with_defaults(
            &store,
            &request,
            TitleDefaults {
                session: NamingMode::Manual,
                ..TitleDefaults::default()
            },
        );
        assert_eq!(result.outcome.outcome, DomainOutcome::Changes);
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
    let valid = wire(&session(), "automatic", "work");
    assert!(
        parse_record_update(
            valid.clone(),
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
    let mut old_operation = valid.clone();
    old_operation["operation_id"] = json!("old");
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
        old_operation,
        old_version,
        dependency,
    ] {
        assert!(
            parse_record_update(
                invalid.clone(),
                "source",
                streams::model::TitleDefaults::default()
            )
            .is_err()
        );
    }
    assert!(
        parse_record_update(
            valid.clone(),
            "changed-source",
            streams::model::TitleDefaults::default()
        )
        .is_err()
    );
    let roster = json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal":"prose"},"inputs":[{"kind":"session","record":session()},{"kind":"session","record":session()}]});
    assert!(
        parse_record_update(
            roster.clone(),
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
    let request = json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":"user","before":stored_stream(&store, "a"),"changes":{"goal":"manual goal"},"inputs":[]});
    let mut invalid = request.clone();
    invalid["inputs"] = json!([{"kind":"session","record":session()}]);
    assert!(
        parse_record_update(
            invalid.clone(),
            "source",
            streams::model::TitleDefaults::default()
        )
        .is_err()
    );
    move_session_observed(
        &store,
        &MoveIntent::new("host", "pi", "native", "b").unwrap(),
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
    let result = update(&store, &request);
    assert_eq!(result.outcome.outcome, DomainOutcome::Changes);
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
fn agent_requests_cannot_set_cli_or_user_only_fields() {
    let user = json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":"user","before":stream("a"),"changes":{"repositories":{}},"inputs":[]});
    let automatic = json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":"automatic","before":stream("a"),"changes":{"goal_mode":"manual"},"inputs":[{"kind":"session","record":session()}]});
    for (request, field) in [(user, "repositories"), (automatic, "goal_mode")] {
        let error = parse_record_update(request, "source", TitleDefaults::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("field is not editable by this actor: {field}")
        );
    }
    // Stream patches reject `status` outright because Stream status is derived from Sessions.
    let stream_status = json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":"user","before":stream("a"),"changes":{"status":"done"},"inputs":[]});
    let error = parse_record_update(stream_status, "source", TitleDefaults::default()).unwrap_err();
    assert!(
        error.to_string().contains("unknown field `status`"),
        "{error}"
    );
}

fn reference_value(url: &str) -> Value {
    json!({"name":"Design","purpose":"Guide work","role":"brief","state":"active",
        "target":{"kind":"link","url":url}})
}
fn reference(url: &str) -> streams::model::Reference {
    serde_json::from_value(reference_value(url)).unwrap()
}
fn reference_wire(before: &Stream, actor: &str, changes: Value) -> Value {
    json!({"version":2,"title_default":"automatic","catalog_source":"source","kind":"stream","actor":actor,"before":before,
        "changes":{"reference_changes":changes},
        "inputs":if actor=="automatic" {json!([{"kind":"session","record":session()}])} else {json!([])}})
}
fn set_row(url: &str, desired: Value) -> Value {
    json!({"key":{"kind":"url","url":url},"desired":desired})
}
fn doc_row() -> Value {
    json!({"key":{"kind":"document","repository_url":"git@example.test:repo","path":"docs/design.md"},
        "desired":{"name":"Design","purpose":"Guide work","role":"brief","state":"active",
            "target":{"kind":"document","repository_url":"git@example.test:repo","path":"docs/design.md"}}})
}
fn saved_stream(save: &RecordSave<EditOutcome>) -> Stream {
    let Record::Stream(stream) = &save.current.record else {
        unreachable!()
    };
    stream.clone()
}

#[test]
fn reference_documents_assign_and_preserve_recovery_without_aliases() {
    for explicit in [false, true] {
        for matching_checkout in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let store = setup(directory.path());
            let before = stream("a");
            let mut captured = session();
            if matching_checkout {
                let mut checkout =
                    streams::model::Checkout::new("repo", "checkout-ref", ".").unwrap();
                checkout.repository_url = Some("git@example.test:repo".into());
                captured.checkout = Some(checkout);
            }
            put(&store, Record::Session(captured.clone()));
            let mut row = doc_row();
            if explicit {
                row["desired"]["target"]["ref"] = json!("explicit");
            }
            let mut request = reference_wire(&before, "automatic", json!([row]));
            request["inputs"][0]["record"] = json!(captured);
            let saved = update(&store, &request);
            let current = saved_stream(&saved);
            let streams::model::ReferenceTarget::Document(document) = &current.references[0].target
            else {
                unreachable!()
            };
            assert_eq!(
                document.r#ref.as_deref(),
                if explicit {
                    Some("explicit")
                } else if matching_checkout {
                    Some("checkout-ref")
                } else {
                    None
                }
            );
            assert_eq!(document.commit, None);
            assert_eq!(
                document.source_session_id.as_deref(),
                Some(session().id.as_str())
            );
            let mut before = current.clone();
            let streams::model::ReferenceTarget::Document(document) =
                &mut before.references[0].target
            else {
                unreachable!()
            };
            document.commit = Some("verified".into());
            document.source_session_id = None;
            let preserved_ref = document.r#ref.clone();
            put(&store, Record::Stream(before.clone()));
            let mut row = doc_row();
            row["desired"]["purpose"] = json!("Changed purpose");
            let mut request = reference_wire(&before, "automatic", json!([row]));
            request["inputs"][0]["record"] = json!(captured);
            let next = saved_stream(&update(&store, &request));
            let streams::model::ReferenceTarget::Document(next) = &next.references[0].target else {
                unreachable!()
            };
            assert_eq!(next.commit.as_deref(), Some("verified"));
            assert_eq!(next.source_session_id, None);
            assert_eq!(next.r#ref, preserved_ref);
        }
    }
}

#[test]
fn partial_reference_save_preserves_stale_and_dropped_rows_and_accepts_independent_changes() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut before = stream("a");
    before.references = vec![
        reference("https://example.test/a"),
        reference("https://example.test/dropped"),
    ];
    before.references[1].state = streams::model::ReferenceState::Dropped;
    let mut current = before.clone();
    current.references[0].purpose = "Outside edit".into();
    current
        .references
        .push(reference("https://example.test/unrelated"));
    put(&store, Record::Stream(current.clone()));
    update(&store, &wire(&session(), "automatic", "Working"));
    let request = reference_wire(
        &before,
        "automatic",
        json!([
            set_row(
                "https://example.test/a",
                reference_value("https://example.test/a")
            ),
            set_row(
                "https://example.test/dropped",
                reference_value("https://example.test/dropped")
            ),
            set_row(
                "https://example.test/new",
                reference_value("https://example.test/new")
            )
        ]),
    );
    let first = update(&store, &request);
    assert_eq!(first.outcome.outcome, DomainOutcome::Changes);
    assert_eq!(json!(first.outcome)["conflicts"], json!([]));
    assert_eq!(json!(first.outcome)["reason"], "");
    let rows = &json!(first.outcome)["reference_results"];
    assert_eq!(rows[0]["outcome"], "superseded");
    assert_eq!(rows[0]["reason"], "reference changed since capture");
    assert_eq!(rows[1]["outcome"], "superseded");
    assert_eq!(rows[1]["action"], "restore");
    assert_eq!(rows[2]["outcome"], "changes");
    assert_eq!(rows[2]["action"], "add");
    let saved = saved_stream(&first);
    assert_eq!(saved.references[..3], current.references);
    // The retry finds the added row in place and the stale rows still stale.
    let replay = update(&store, &request);
    assert!(!replay.written);
    assert_eq!(replay.outcome.outcome, DomainOutcome::Superseded);
    let rows = &json!(replay.outcome)["reference_results"];
    assert_eq!(rows[0]["outcome"], "superseded");
    assert_eq!(rows[1]["outcome"], "superseded");
    assert_eq!(rows[2]["outcome"], "no-change");
}

#[test]
fn reference_roles_states_kind_changes_and_noops_follow_saved_revisions() {
    for role in ["deliverable", "dependency", "brief", "background"] {
        for state in ["active", "fulfilled", "dropped"] {
            let directory = tempfile::tempdir().unwrap();
            let store = setup(directory.path());
            let mut desired = reference_value("https://example.test/item");
            desired["role"] = json!(role);
            desired["state"] = json!(state);
            desired["target"]["kind"] = json!("issue");
            let add = reference_wire(
                &stream("a"),
                "automatic",
                json!([set_row("https://example.test/item", desired.clone())]),
            );
            let before = saved_stream(&update(&store, &add));
            assert_eq!(
                serde_json::to_value(before.references[0].role).unwrap(),
                role
            );
            assert_eq!(
                serde_json::to_value(before.references[0].state).unwrap(),
                state
            );
            let mut request = reference_wire(
                &before,
                "automatic",
                json!([set_row("https://example.test/item", desired.clone())]),
            );
            let result = update(&store, &request);
            assert_eq!(result.outcome.outcome, DomainOutcome::NoChange);
            assert!(!result.written);
            desired["target"]["kind"] = json!("pr");
            request["changes"]["reference_changes"][0]["desired"] = desired;
            assert_eq!(
                update(&store, &request).outcome.outcome,
                DomainOutcome::Changes
            );
        }
    }
}

#[test]
fn user_correction_conflicts_at_occupied_destination_but_saves_independent_delete() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut before = stream("a");
    before.references = vec![
        reference("https://example.test/a"),
        reference("https://example.test/b"),
        reference("https://example.test/c"),
    ];
    put(&store, Record::Stream(before.clone()));
    let before = stored_stream(&store, "a");
    let request = reference_wire(
        &before,
        "user",
        json!([
            set_row("https://example.test/a",reference_value("https://example.test/b")),
            {"key":{"kind":"url","url":"https://example.test/c"},"desired":null}
        ]),
    );
    let result = update(&store, &request);
    assert_eq!(result.outcome.outcome, DomainOutcome::Changes);
    assert_eq!(
        json!(result.outcome)["reference_results"][0]["reason"],
        "replacement target already exists"
    );
    assert_eq!(
        json!(result.outcome)["reference_results"][0]["outcome"],
        "conflict"
    );
    assert_eq!(
        json!(result.outcome)["reference_results"][1]["action"],
        "delete"
    );
    assert_eq!(saved_stream(&result).references, before.references[..2]);
    let correction = reference_wire(
        &saved_stream(&result),
        "user",
        json!([set_row(
            "https://example.test/a",
            reference_value("https://example.test/new")
        )]),
    );
    let result = update(&store, &correction);
    assert_eq!(result.outcome.outcome, DomainOutcome::Changes);
    assert_eq!(
        saved_stream(&result).references[0].target.key(),
        reference("https://example.test/new").target.key()
    );
}

#[test]
fn reference_session_and_stream_guards_cover_move_revisit_native_identity_and_owner() {
    for kind in ["move", "revisit", "native", "missing", "owner", "identity"] {
        let directory = tempfile::tempdir().unwrap();
        let store = setup(directory.path());
        let before = stream("a");
        let request = reference_wire(&before, "automatic", json!([doc_row()]));
        match kind {
            "move" | "revisit" => {
                move_session_observed(
                    &store,
                    &MoveIntent::new("host", "pi", "native", "b").unwrap(),
                )
                .unwrap();
                if kind == "revisit" {
                    move_session_observed(
                        &store,
                        &MoveIntent::new("host", "pi", "native", "a").unwrap(),
                    )
                    .unwrap();
                }
            }
            "missing" => {
                let address = RecordAddress::for_record(&Record::Session(session()));
                std::fs::remove_file(
                    directory
                        .path()
                        .join(relative_record_path(&address).unwrap()),
                )
                .unwrap();
            }
            _ => {}
        }
        if matches!(kind, "owner" | "identity" | "native") {
            let edit = streams::operations::references::ReferenceChanges::new(
                &before,
                streams::operations::Actor::Automatic,
                Some(&session()),
                vec![
                    streams::operations::references::ReferenceChange::Automatic {
                        key: streams::model::ResourceKey::Url {
                            url: "https://example.test/a".into(),
                        },
                        desired: streams::operations::references::AutomaticReferenceValue {
                            name: "A".into(),
                            purpose: "Work".into(),
                            role: streams::model::ReferenceRole::Brief,
                            state: streams::model::ReferenceState::Active,
                            target:
                                streams::operations::references::AutomaticReferenceTarget::Link {
                                    url: "https://example.test/a".into(),
                                },
                        },
                    },
                ],
            )
            .unwrap();
            let mut current = before.clone();
            let mut current_session = session();
            match kind {
                "owner" => current.owner_host_id = "other".into(),
                "identity" => current.id = "other".into(),
                // The store rejects inconsistent Session keys before domain apply; inject the
                // changed native identity directly at the pure boundary instead.
                "native" => current_session.harness_session_id = "other".into(),
                _ => unreachable!(),
            }
            let result = edit.apply(&current, Some(&current_session)).unwrap();
            let outcome = result.outcome();
            assert_eq!(outcome.outcome, DomainOutcome::Superseded);
            assert_eq!(outcome.conflicts, ["inputs"]);
        } else {
            let result = update(&store, &request);
            assert_eq!(result.outcome.outcome, DomainOutcome::Superseded, "{kind}");
            assert_eq!(json!(result.outcome)["conflicts"], json!(["inputs"]));
            assert!(Record::Stream(saved_stream(&result)).same_content(&Record::Stream(before)));
        }
    }
}

#[test]
fn captured_mutations_never_recreate_deleted_or_recreated_rows_but_absent_additions_rediscover() {
    for actor in ["automatic", "user"] {
        for current_kind in ["deleted", "changed"] {
            let directory = tempfile::tempdir().unwrap();
            let store = setup(directory.path());
            let mut before = stream("a");
            before.references.push(reference("https://example.test/a"));
            let mut desired = reference_value("https://example.test/a");
            desired["purpose"] = json!("Captured change");
            let request = reference_wire(
                &before,
                actor,
                json!([set_row("https://example.test/a", desired)]),
            );
            let mut current = before.clone();
            if current_kind == "deleted" {
                current.references.clear();
            } else {
                current.references[0].purpose = "Outside edit".into();
            }
            put(&store, Record::Stream(current.clone()));
            let result = update(&store, &request);
            assert_eq!(
                result.outcome.outcome,
                if actor == "automatic" {
                    DomainOutcome::Superseded
                } else {
                    DomainOutcome::Conflict
                }
            );
            assert!(Record::Stream(saved_stream(&result)).same_content(&Record::Stream(current)));
            // A user capture is refused as a whole by the Stream revision.
            assert_eq!(
                json!(result.outcome)["conflicts"],
                if actor == "automatic" {
                    json!(["references"])
                } else {
                    json!(["inputs"])
                }
            );
        }
        let directory = tempfile::tempdir().unwrap();
        let store = setup(directory.path());
        let before = stream("a");
        let request = reference_wire(
            &before,
            actor,
            json!([set_row(
                "https://example.test/a",
                reference_value("https://example.test/a")
            )]),
        );
        // Permanent deletion leaves no suppression ledger, so an old automatic capture adds
        // the row again. A user edit compares and swaps on the Stream revision.
        let mut temporary = before.clone();
        temporary
            .references
            .push(reference("https://example.test/a"));
        put(&store, Record::Stream(temporary));
        put(&store, Record::Stream(before));
        assert_eq!(
            update(&store, &request).outcome.outcome,
            if actor == "automatic" {
                DomainOutcome::Changes
            } else {
                DomainOutcome::Conflict
            }
        );
    }
}

#[test]
fn correction_old_selector_cannot_reach_the_new_target_and_user_can_restore_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut before = stream("a");
    before.references.push(reference("https://example.test/a"));
    let request = reference_wire(
        &before,
        "automatic",
        json!([set_row(
            "https://example.test/a",
            reference_value("https://example.test/a")
        )]),
    );
    let mut corrected = before.clone();
    corrected.references[0].target = streams::model::ReferenceTarget::Link {
        url: "https://example.test/b".into(),
    };
    corrected.references[0].state = streams::model::ReferenceState::Dropped;
    put(&store, Record::Stream(corrected.clone()));
    let result = update(&store, &request);
    assert_eq!(
        json!(result.outcome)["reference_results"][0]["reason"],
        "reference no longer exists"
    );
    assert!(Record::Stream(saved_stream(&result)).same_content(&Record::Stream(corrected.clone())));
    let mut desired = reference_value("https://example.test/b");
    desired["state"] = json!("fulfilled");
    let restore = reference_wire(
        &stored_stream(&store, "a"),
        "user",
        json!([set_row("https://example.test/b", desired)]),
    );
    let result = update(&store, &restore);
    assert_eq!(
        json!(result.outcome)["reference_results"][0]["action"],
        "restore"
    );
    assert_eq!(
        saved_stream(&result).references[0].state,
        streams::model::ReferenceState::Fulfilled
    );
}

#[test]
fn reference_raw_parser_rejects_authority_presence_overlap_and_old_list_payloads() {
    let valid = reference_wire(&stream("a"), "automatic", json!([doc_row()]));
    assert!(parse_record_update(valid.clone(), "source", TitleDefaults::default()).is_ok());
    let mut invalids = Vec::new();
    for (field, value) in [
        ("commit", Value::Null),
        ("source_session_id", Value::Null),
        ("ref", Value::Null),
        ("ref", json!("")),
        ("extra", json!(true)),
        ("path", json!("../escape")),
        ("repository_url", json!("\u{85}")),
    ] {
        let mut value_wire = valid.clone();
        value_wire["changes"]["reference_changes"][0]["desired"]["target"][field] = value;
        invalids.push(value_wire);
    }
    for field in ["key", "desired"] {
        let mut invalid = valid.clone();
        invalid["changes"]["reference_changes"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        invalids.push(invalid);
    }
    for desired in [Value::Null, json!([])] {
        let mut invalid = valid.clone();
        invalid["changes"]["reference_changes"][0]["desired"] = desired;
        invalids.push(invalid);
    }
    for changes in [json!([]), Value::Null, json!([doc_row(), doc_row()])] {
        let mut invalid = valid.clone();
        invalid["changes"]["reference_changes"] = changes;
        invalids.push(invalid);
    }
    for (field, value) in [
        ("actor", json!("native")),
        ("inputs", json!([])),
        ("changes", json!({"references":[]})),
        ("changes", json!({"documents":[]})),
        ("changes", json!({"issues":[]})),
        ("changes", json!({"prs":[]})),
        ("changes", json!({"document_additions":[]})),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        invalids.push(invalid);
    }
    let mut invalid = valid.clone();
    invalid["changes"]["goal"] = json!("mixed");
    invalids.push(invalid);
    let mut invalid = valid.clone();
    invalid["changes"]["reference_changes"][0]["next_revision"] = json!("old");
    invalids.push(invalid);
    let mut invalid = valid.clone();
    invalid["changes"]["reference_changes"][0]["key"]["path"] = json!("different");
    invalids.push(invalid);
    let mut invalid = valid.clone();
    invalid["actor"] = json!("user");
    invalids.push(invalid);
    let mut deletion = valid.clone();
    deletion["changes"]["reference_changes"][0]["desired"] = Value::Null;
    invalids.push(deletion);
    for invalid in invalids {
        assert!(
            parse_record_update(invalid.clone(), "source", TitleDefaults::default()).is_err(),
            "{invalid}"
        );
    }
    // User nullable recovery fields are allowed only after dispatch by actor.
    let mut user = valid.clone();
    user["actor"] = json!("user");
    user["inputs"] = json!([]);
    user["changes"]["reference_changes"][0]["desired"]["target"]["ref"] = Value::Null;
    assert!(parse_record_update(user.clone(), "source", TitleDefaults::default()).is_ok());
    let mut before = stream("a");
    before.references.push(reference("https://example.test/a"));
    let deletion = reference_wire(
        &before,
        "user",
        json!([{"key":{"kind":"url","url":"https://example.test/a"},"desired":null}]),
    );
    assert!(parse_record_update(deletion.clone(), "source", TitleDefaults::default()).is_ok());
    let mut invalid = deletion.clone();
    invalid["operation_id"] = json!("old");
    assert!(parse_record_update(invalid.clone(), "source", TitleDefaults::default()).is_err());
}

#[test]
fn reference_constructor_rejects_overlap_and_foreign_captures() {
    use streams::{
        model::ResourceKey,
        operations::{
            Actor,
            references::{
                AutomaticReferenceTarget, AutomaticReferenceValue, ReferenceChange,
                ReferenceChanges,
            },
        },
    };
    let mut before = stream("a");
    before.references = vec![reference("https://example.test/a")];
    let user = |old: &str, new: &str| ReferenceChange::UserSet {
        key: ResourceKey::Url { url: old.into() },
        desired: reference(new).value(),
    };
    for changes in [
        vec![
            user("https://example.test/a", "https://example.test/b"),
            user("https://example.test/b", "https://example.test/b"),
        ],
        vec![
            user("https://example.test/a", "https://example.test/c"),
            user("https://example.test/b", "https://example.test/c"),
        ],
        vec![user("https://example.test/b", "https://example.test/c")],
        vec![ReferenceChange::UserDelete {
            key: ResourceKey::Url {
                url: "https://example.test/absent".into(),
            },
        }],
    ] {
        assert!(ReferenceChanges::new(&before, Actor::User, None, changes).is_err());
    }
    let automatic = || ReferenceChange::Automatic {
        key: ResourceKey::Url {
            url: "https://example.test/a".into(),
        },
        desired: AutomaticReferenceValue {
            name: "Name".into(),
            purpose: "Purpose".into(),
            role: streams::model::ReferenceRole::Brief,
            state: streams::model::ReferenceState::Active,
            target: AutomaticReferenceTarget::Link {
                url: "https://example.test/a".into(),
            },
        },
    };
    assert!(ReferenceChanges::new(&before, Actor::User, None, vec![automatic()]).is_err());
    assert!(
        ReferenceChanges::new(
            &before,
            Actor::Automatic,
            Some(&session()),
            vec![user("https://example.test/a", "https://example.test/a")]
        )
        .is_err()
    );
    assert!(ReferenceChanges::new(&before, Actor::Native, None, vec![automatic()]).is_err());
    for field in ["host", "stream"] {
        let mut captured = session();
        if field == "host" {
            captured.host_id = "foreign".into();
        } else {
            captured.stream_id = "other".into();
        }
        assert!(
            ReferenceChanges::new(
                &before,
                Actor::Automatic,
                Some(&captured),
                vec![automatic()]
            )
            .is_err()
        );
    }
}

#[test]
fn reference_io_budgets_are_record_local_with_malformed_unrelated_records() {
    for size in [0, 64] {
        for actor in ["automatic", "user"] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let store = setup(root);
            for index in 0..size {
                std::fs::write(
                    root.join(format!("hosts/host/streams/unrelated-{index}.json")),
                    "malformed unrelated record",
                )
                .unwrap();
            }
            let stream_path = root.join(
                relative_record_path(&RecordAddress::for_record(&Record::Stream(stream("a"))))
                    .unwrap(),
            );
            let session_path = root.join(
                relative_record_path(&RecordAddress::for_record(&Record::Session(session())))
                    .unwrap(),
            );
            let marker = root.join(".catalog-local/records.json");
            let stream_bytes = std::fs::metadata(&stream_path).unwrap().len();
            let expected_bytes = std::fs::metadata(&marker).unwrap().len()
                + stream_bytes
                + if actor == "automatic" {
                    std::fs::metadata(&session_path).unwrap().len()
                } else {
                    0
                };
            let request = reference_wire(&stored_stream(&store, "a"), actor, json!([doc_row()]));
            let events = Arc::new(Mutex::new(Vec::new()));
            let observed = events.clone();
            let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
            update(&store, &request);
            let trace = events.lock().unwrap().clone();
            let mut expected = vec![
                RecordIoEvent::Lock(root.join(".catalog.lock")),
                RecordIoEvent::Read(marker.clone()),
                RecordIoEvent::Read(stream_path.clone()),
            ];
            if actor == "automatic" {
                expected.push(RecordIoEvent::Read(session_path));
            }
            assert_eq!(record_io::accesses(&trace), expected);
            assert_eq!(record_io::bytes_read(&trace), expected_bytes as usize);
            let written = std::fs::metadata(&stream_path).unwrap().len() as usize;
            assert_eq!(record_io::writes(&trace), [(stream_path.clone(), written)]);
            events.lock().unwrap().clear();
            // A retry that already landed reads the same records and writes nothing.
            let retry = update(&store, &request);
            assert!(!retry.written);
            assert_eq!(retry.outcome.outcome, DomainOutcome::NoChange);
            let trace = events.lock().unwrap().clone();
            assert_eq!(record_io::accesses(&trace), expected);
            assert_eq!(
                record_io::bytes_read(&trace),
                expected_bytes as usize - stream_bytes as usize + written
            );
            assert!(record_io::writes(&trace).is_empty());
        }
    }
}
