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
    put_with_id(store, record, "setup");
}
fn put_with_id(store: &YamlRecordStore, record: Record, operation_id: &str) {
    let request = RecordRequest::new(
        RecordAddress::for_record(&record),
        operation_id,
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

fn reference_value(url: &str) -> Value {
    json!({"name":"Design","purpose":"Guide work","role":"brief","state":"active",
        "target":{"kind":"link","url":url}})
}
fn reference(url: &str, revision: &str) -> streams::model::Reference {
    let mut value = reference_value(url);
    value["revision"] = json!(revision);
    serde_json::from_value(value).unwrap()
}
fn reference_wire(before: &Stream, actor: &str, changes: Value) -> Value {
    json!({"version":2,"title_default":"automatic","catalog_source":"source","operation_id":"references","kind":"stream","actor":actor,"before":before,
        "changes":{"reference_changes":changes},
        "inputs":if actor=="automatic" {json!([{"kind":"session","record":session()}])} else {json!([])}})
}
fn set_row(url: &str, desired: Value) -> Value {
    json!({"key":{"kind":"url","url":url},"desired":desired,"next_revision":"references"})
}
fn doc_row() -> Value {
    json!({"key":{"kind":"document","repository_url":"git@example.test:repo","path":"docs/design.md"},
        "desired":{"name":"Design","purpose":"Guide work","role":"brief","state":"active",
            "target":{"kind":"document","repository_url":"git@example.test:repo","path":"docs/design.md"}},
        "next_revision":"references"})
}
fn saved_stream(save: &streams::storage::records::RecordSave) -> Stream {
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
            put_with_id(&store, Record::Session(captured.clone()), "checkout");
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
            put_with_id(&store, Record::Stream(before.clone()), "recovery");
            let mut row = doc_row();
            row["desired"]["purpose"] = json!("Changed purpose");
            row["next_revision"] = json!("update-document");
            let mut request = reference_wire(&before, "automatic", json!([row]));
            request["operation_id"] = json!("update-document");
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
        reference("https://example.test/a", "old"),
        reference("https://example.test/dropped", "drop"),
    ];
    before.references[1].state = streams::model::ReferenceState::Dropped;
    let mut current = before.clone();
    current.references[0].purpose = "Outside edit with unchanged token".into();
    current
        .references
        .push(reference("https://example.test/unrelated", "outside"));
    put_with_id(&store, Record::Stream(current.clone()), "current");
    update(
        &store,
        &wire("progress", &session(), "automatic", "Working"),
    );
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
    assert_eq!(first.outcome["outcome"], "changes");
    assert_eq!(first.outcome["conflicts"], json!([]));
    assert_eq!(first.outcome["reason"], "");
    let rows = &first.outcome["reference_results"];
    assert_eq!(rows[0]["outcome"], "superseded");
    assert_eq!(rows[0]["reason"], "reference changed since capture");
    assert_eq!(rows[1]["outcome"], "superseded");
    assert_eq!(rows[1]["action"], "restore");
    assert_eq!(rows[2]["outcome"], "changes");
    assert_eq!(rows[2]["action"], "add");
    let saved = saved_stream(&first);
    assert_eq!(saved.references[..3], current.references);
    assert_eq!(saved.references[3].revision, "references");
    let replay = update(&store, &request);
    assert!(replay.repeated);
    assert_eq!(replay.outcome, first.outcome);
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
            request["operation_id"] = json!("next");
            request["changes"]["reference_changes"][0]["next_revision"] = json!("next");
            let result = update(&store, &request);
            assert_eq!(result.outcome["outcome"], "no-change");
            assert_eq!(saved_stream(&result).references[0].revision, "references");
            desired["target"]["kind"] = json!("pr");
            request["operation_id"] = json!("classify");
            request["changes"]["reference_changes"][0]["next_revision"] = json!("classify");
            request["changes"]["reference_changes"][0]["desired"] = desired;
            assert_eq!(update(&store, &request).outcome["outcome"], "changes");
        }
    }
}

#[test]
fn user_correction_conflicts_at_occupied_destination_but_saves_independent_delete() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut before = stream("a");
    before.references = vec![
        reference("https://example.test/a", "a"),
        reference("https://example.test/b", "b"),
        reference("https://example.test/c", "c"),
    ];
    put_with_id(&store, Record::Stream(before.clone()), "current");
    let request = reference_wire(
        &before,
        "user",
        json!([
            set_row("https://example.test/a",reference_value("https://example.test/b")),
            {"key":{"kind":"url","url":"https://example.test/c"},"desired":null}
        ]),
    );
    let result = update(&store, &request);
    assert_eq!(result.outcome["outcome"], "changes");
    assert_eq!(
        result.outcome["reference_results"][0]["reason"],
        "replacement target already exists"
    );
    assert_eq!(
        result.outcome["reference_results"][0]["outcome"],
        "conflict"
    );
    assert_eq!(result.outcome["reference_results"][1]["action"], "delete");
    assert_eq!(saved_stream(&result).references, before.references[..2]);
    let correction = reference_wire(
        &before,
        "user",
        json!([set_row(
            "https://example.test/a",
            reference_value("https://example.test/new")
        )]),
    );
    let mut correction = correction;
    correction["operation_id"] = json!("correction");
    correction["changes"]["reference_changes"][0]["next_revision"] = json!("correction");
    let result = update(&store, &correction);
    assert_eq!(result.outcome["outcome"], "changes");
    assert_eq!(
        saved_stream(&result).references[0].target.key(),
        reference("https://example.test/new", "x").target.key()
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
                    "move",
                    &MoveIntent::new("host", "pi", "native", "b").unwrap(),
                )
                .unwrap();
                if kind == "revisit" {
                    move_session_observed(
                        &store,
                        "back",
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
                "references",
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
                        next_revision: "references".into(),
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
            assert_eq!(result.domain()["outcome"], "superseded");
            assert_eq!(result.domain()["conflicts"], json!(["inputs"]));
        } else {
            let result = update(&store, &request);
            assert_eq!(result.outcome["outcome"], "superseded", "{kind}");
            assert_eq!(result.outcome["conflicts"], json!(["inputs"]));
            assert_eq!(saved_stream(&result), before);
        }
    }
}

#[test]
fn captured_mutations_never_recreate_deleted_or_recreated_rows_but_absent_additions_rediscover() {
    for actor in ["automatic", "user"] {
        for current_kind in ["deleted", "recreated", "changed-token"] {
            let directory = tempfile::tempdir().unwrap();
            let store = setup(directory.path());
            let mut before = stream("a");
            before
                .references
                .push(reference("https://example.test/a", "original"));
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
                current.references[0].revision = "recreated-or-ABA".into();
            }
            put_with_id(&store, Record::Stream(current.clone()), "outside");
            let result = update(&store, &request);
            assert_eq!(
                result.outcome["outcome"],
                if actor == "automatic" {
                    "superseded"
                } else {
                    "conflict"
                }
            );
            assert_eq!(saved_stream(&result), current);
            assert_eq!(result.outcome["conflicts"], json!(["references"]));
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
        // Permanent deletion leaves no suppression ledger, even for an old capture.
        let mut temporary = before.clone();
        temporary
            .references
            .push(reference("https://example.test/a", "temporary"));
        put_with_id(&store, Record::Stream(temporary), "create-temporary");
        put_with_id(&store, Record::Stream(before), "permanent-delete");
        assert_eq!(update(&store, &request).outcome["outcome"], "changes");
    }
}

#[test]
fn correction_old_selector_cannot_reach_the_new_target_and_user_can_restore_dropped() {
    let directory = tempfile::tempdir().unwrap();
    let store = setup(directory.path());
    let mut before = stream("a");
    before
        .references
        .push(reference("https://example.test/a", "original"));
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
    corrected.references[0].revision = "correction".into();
    corrected.references[0].state = streams::model::ReferenceState::Dropped;
    put_with_id(&store, Record::Stream(corrected.clone()), "correction");
    let result = update(&store, &request);
    assert_eq!(
        result.outcome["reference_results"][0]["reason"],
        "reference no longer exists"
    );
    assert_eq!(saved_stream(&result), corrected);
    let mut desired = reference_value("https://example.test/b");
    desired["state"] = json!("fulfilled");
    let mut restore = reference_wire(
        &corrected,
        "user",
        json!([set_row("https://example.test/b", desired)]),
    );
    restore["operation_id"] = json!("restore");
    restore["changes"]["reference_changes"][0]["next_revision"] = json!("restore");
    let result = update(&store, &restore);
    assert_eq!(result.outcome["reference_results"][0]["action"], "restore");
    assert_eq!(
        saved_stream(&result).references[0].state,
        streams::model::ReferenceState::Fulfilled
    );
}

#[test]
fn reference_raw_parser_rejects_authority_presence_overlap_and_old_list_payloads() {
    let valid = reference_wire(&stream("a"), "automatic", json!([doc_row()]));
    assert!(parse_record_update(&valid.to_string(), "source", TitleDefaults::default()).is_ok());
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
    for field in ["key", "desired", "next_revision"] {
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
    invalid["changes"]["reference_changes"][0]["next_revision"] = json!("wrong");
    invalids.push(invalid);
    let mut invalid = valid.clone();
    invalid["changes"]["reference_changes"][0]["key"]["path"] = json!("different");
    invalids.push(invalid);
    let mut invalid = valid.clone();
    invalid["actor"] = json!("user");
    invalids.push(invalid);
    let mut deletion = valid.clone();
    deletion["changes"]["reference_changes"][0]["desired"] = Value::Null;
    deletion["changes"]["reference_changes"][0]
        .as_object_mut()
        .unwrap()
        .remove("next_revision");
    invalids.push(deletion);
    for invalid in invalids {
        assert!(
            parse_record_update(&invalid.to_string(), "source", TitleDefaults::default()).is_err(),
            "{invalid}"
        );
    }
    // User nullable recovery fields are allowed only after dispatch by actor.
    let mut user = valid.clone();
    user["actor"] = json!("user");
    user["inputs"] = json!([]);
    user["changes"]["reference_changes"][0]["desired"]["target"]["ref"] = Value::Null;
    assert!(parse_record_update(&user.to_string(), "source", TitleDefaults::default()).is_ok());
    let mut before = stream("a");
    before
        .references
        .push(reference("https://example.test/a", "captured"));
    let deletion = reference_wire(
        &before,
        "user",
        json!([{"key":{"kind":"url","url":"https://example.test/a"},"desired":null}]),
    );
    assert!(parse_record_update(&deletion.to_string(), "source", TitleDefaults::default()).is_ok());
    let mut invalid = deletion.clone();
    invalid["changes"]["reference_changes"][0]["next_revision"] = json!("references");
    assert!(parse_record_update(&invalid.to_string(), "source", TitleDefaults::default()).is_err());
}

#[test]
fn reference_constructor_rejects_overlap_reused_revisions_and_foreign_captures() {
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
    before.references = vec![reference("https://example.test/a", "captured")];
    let user = |old: &str, new: &str, revision: &str| ReferenceChange::UserSet {
        key: ResourceKey::Url { url: old.into() },
        desired: reference(new, "unused").value(),
        next_revision: revision.into(),
    };
    for changes in [
        vec![user(
            "https://example.test/a",
            "https://example.test/b",
            "captured",
        )],
        vec![
            user("https://example.test/a", "https://example.test/b", "next"),
            user("https://example.test/b", "https://example.test/b", "next"),
        ],
        vec![
            user("https://example.test/a", "https://example.test/c", "next"),
            user("https://example.test/b", "https://example.test/c", "next"),
        ],
        vec![user(
            "https://example.test/b",
            "https://example.test/c",
            "next",
        )],
        vec![ReferenceChange::UserDelete {
            key: ResourceKey::Url {
                url: "https://example.test/absent".into(),
            },
        }],
    ] {
        let id = if matches!(&changes[0], ReferenceChange::UserSet {next_revision,..} if next_revision=="captured")
        {
            "captured"
        } else {
            "next"
        };
        assert!(ReferenceChanges::new(id, &before, Actor::User, None, changes).is_err());
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
        next_revision: "next".into(),
    };
    assert!(ReferenceChanges::new("next", &before, Actor::User, None, vec![automatic()]).is_err());
    assert!(
        ReferenceChanges::new(
            "next",
            &before,
            Actor::Automatic,
            Some(&session()),
            vec![user(
                "https://example.test/a",
                "https://example.test/a",
                "next"
            )]
        )
        .is_err()
    );
    assert!(
        ReferenceChanges::new("next", &before, Actor::Native, None, vec![automatic()]).is_err()
    );
    for field in ["host", "stream"] {
        let mut captured = session();
        if field == "host" {
            captured.host_id = "foreign".into();
        } else {
            captured.stream_id = "other".into();
        }
        assert!(
            ReferenceChanges::new(
                "next",
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
                    root.join(format!("hosts/host/streams/unrelated-{index}.yaml")),
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
            let expected_bytes = std::fs::metadata(&marker).unwrap().len()
                + std::fs::metadata(&stream_path).unwrap().len()
                + if actor == "automatic" {
                    std::fs::metadata(&session_path).unwrap().len()
                } else {
                    0
                };
            let events = Arc::new(Mutex::new(Vec::new()));
            let observed = events.clone();
            let store = store.with_io_observer(move |event| observed.lock().unwrap().push(event));
            let request = reference_wire(&stream("a"), actor, json!([doc_row()]));
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
            assert!(update(&store, &request).repeated);
            let trace = events.lock().unwrap().clone();
            assert_eq!(record_io::accesses(&trace), expected[..3]);
            assert_eq!(
                record_io::bytes_read(&trace),
                std::fs::metadata(marker).unwrap().len() as usize + written
            );
            assert!(record_io::writes(&trace).is_empty());
        }
    }
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
