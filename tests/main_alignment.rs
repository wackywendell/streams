use serde_json::json;
use streams_temper::storage::StorageError;
use streams_temper::{
    agent::session_id_for,
    codec::record_to_data,
    model::{Catalog, Record, Session, Stream},
    operations::{Actor, Authority, DomainOutcome, Reconciliation, reconcile, update},
    runtime::conversations::Harness,
    yaml_store::{YamlStore, relative_record_path},
};

fn edit(
    before: &Record,
    changes: serde_json::Value,
    actor: Actor,
) -> streams_temper::operations::Operation {
    let token = (actor == Actor::Automatic || matches!(before, Record::Session(_)))
        .then(|| "creation".into());
    update(
        "edit",
        before,
        changes.as_object().unwrap(),
        actor,
        token,
        vec![],
    )
    .unwrap()
}

#[test]
fn generated_stream_text_merges_unrelated_changes_but_fences_description_inputs() {
    let before = Record::Stream(Stream::new("work", "Work", None, "host").unwrap());
    let operation = edit(&before, json!({"goal": "Generated goal"}), Actor::Automatic);
    let authority = Authority::new("host", "creation").unwrap();
    for unrelated in [
        json!({"archived": true}),
        json!({"status": "waiting"}),
        json!({"issues": [{"url": "https://example.test/issue", "description": "Issue"}]}),
    ] {
        let current = edit(&before, unrelated, Actor::User).after().clone();
        let catalog = Catalog::default().replacing(current.clone()).unwrap();
        let result = reconcile(&catalog, &operation, Some(&authority));
        assert_eq!(result.outcome(), DomainOutcome::Changes);
        let Reconciliation::Changes { record, .. } = result else {
            panic!("expected changes");
        };
        let merged = record_to_data(&record);
        assert_eq!(merged["goal"], "Generated goal");
        for field in ["status", "archived", "issues"] {
            assert_eq!(merged[field], record_to_data(&current)[field]);
        }
    }
    for description in [
        json!({"title": "Renamed"}),
        json!({"goal": "Manual goal"}),
        json!({"title_mode": "manual"}),
        json!({"goal_mode": "manual"}),
    ] {
        let current = edit(&before, description, Actor::User).after().clone();
        let catalog = Catalog::default().replacing(current).unwrap();
        assert_eq!(
            reconcile(&catalog, &operation, Some(&authority)).outcome(),
            DomainOutcome::Superseded
        );
    }
}

#[test]
fn generated_session_text_still_fences_the_whole_captured_target() {
    let stream = Stream::new("work", "Work", None, "host").unwrap();
    let before = Record::Session(
        Session::new("session", "host", "work", "pi", "native", "Session").unwrap(),
    );
    let operation = edit(
        &before,
        json!({"state": "Generated state"}),
        Actor::Automatic,
    );
    let current = edit(&before, json!({"status": "waiting"}), Actor::User)
        .after()
        .clone();
    let catalog = Catalog::new(vec![stream], vec![], vec![])
        .unwrap()
        .replacing(current)
        .unwrap();
    let authority = Authority::new("host", "creation").unwrap();
    assert_eq!(
        reconcile(&catalog, &operation, Some(&authority)).outcome(),
        DomainOutcome::Superseded
    );
}

#[test]
fn session_ids_use_the_exact_python_unicode_identity_preimage() {
    assert_eq!(
        session_id_for("høst", Harness::Pi, "nâtive/😀\u{7f}"),
        "6d0d9e7d-b373-507a-896f-31140293e66a"
    );
    let id = session_id_for("host", Harness::Pi, "native");
    assert_eq!(id, session_id_for("host", Harness::Pi, "native"));
    assert_ne!(id, session_id_for("other-host", Harness::Pi, "native"));
    assert_ne!(id, session_id_for("host", Harness::Codex, "native"));
}

#[test]
fn sessions_are_physically_stream_partitioned_without_changing_logical_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let store = YamlStore::new(temporary.path());
    let stream = Record::Stream(Stream::new("work/Ω", "Work", None, "host/A").unwrap());
    let session = Record::Session(
        Session::new("native/😀", "host/A", "work/Ω", "pi", "native", "Session").unwrap(),
    );
    store
        .with_transaction::<_, StorageError>(|tx| {
            tx.save(&stream, &tx.read()?)?;
            tx.save(&session, &tx.read()?)?;
            assert_eq!(
                tx.read()?.catalog.get(&session.key()),
                Some(session.clone())
            );
            Ok(())
        })
        .unwrap();
    let path = relative_record_path(&session);
    assert_eq!(
        path.to_str().unwrap(),
        "sessions/work%2F%CE%A9/host%2F%41/native%2F%F0%9F%98%80.yaml"
    );
    assert!(temporary.path().join(&path).is_file());
    let bytes = std::fs::read(temporary.path().join(&path)).unwrap();
    assert_eq!(
        YamlStore::decode_record(path.to_str().unwrap(), &bytes).unwrap(),
        session
    );
    assert!(
        YamlStore::decode_record("sessions/host%2F%41/native%2F%F0%9F%98%80.yaml", &bytes).is_err()
    );
    let Record::Session(mut moved) = session.clone() else {
        unreachable!()
    };
    moved.stream_id = "another-stream".into();
    let moved = Record::Session(moved);
    assert_eq!(session.key(), moved.key());
    assert_ne!(relative_record_path(&session), relative_record_path(&moved));
}

#[test]
fn partitioned_session_scans_reject_symlinked_stream_directories() {
    let temporary = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir(temporary.path().join("sessions")).unwrap();
    std::os::unix::fs::symlink(outside.path(), temporary.path().join("sessions/work")).unwrap();
    let error = YamlStore::new(temporary.path())
        .with_transaction(|tx| tx.read())
        .unwrap_err();
    assert!(error.to_string().contains("symlinked catalog directory"));
}
