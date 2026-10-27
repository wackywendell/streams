use super::*;

#[test]
fn direct_human_edit_retry_keeps_its_original_decision_and_actual_current_record() {
    let f = Fixture::new();
    let created = f.create("Work", None, Status::Active);
    let id = created["id"].as_str().unwrap();
    let before = f.select(id);
    let changes = json!({"goal":"First"}).as_object().unwrap().clone();
    let first = application::save_user_edit(
        f.opened.records(),
        "human-edit",
        &Record::Stream(before.clone()),
        &changes,
        TitleDefaults::default(),
    )
    .unwrap();
    let later = ok(f.update(
        id,
        StreamFields {
            goal: Some("Later".into()),
            ..Default::default()
        },
    ));
    let bytes = f.snapshot();
    let retry = application::save_user_edit(
        f.opened.records(),
        "human-edit",
        &Record::Stream(before.clone()),
        &changes,
        TitleDefaults::default(),
    )
    .unwrap();
    assert!(retry.repeated);
    assert_eq!(retry.outcome, first.outcome);
    assert_eq!(record_to_data(&retry.current.record), later["stream"]);
    assert_eq!(f.snapshot(), bytes);
}

#[test]
fn ordinary_human_edits_save_locally_without_opening_transport_or_publishing() {
    let f = Fixture::new();
    let stream = f.create("Work", None, Status::Active);
    let id = stream["id"].as_str().unwrap();
    std::fs::write(
        f.opened.root().join(".catalog-local/transport.json"),
        "invalid transport config",
    )
    .unwrap();
    let saved = ok(f.update(
        id,
        StreamFields {
            goal: Some("Saved locally".into()),
            ..Default::default()
        },
    ));
    assert_eq!(saved["save"]["local"], "persisted");
    assert_eq!(saved["save"]["domain"]["outcome"], "changes");
    assert!(saved["save"].get("sharing").is_none());
    assert!(saved.get("publication").is_none());
    assert_eq!(ok(f.show(id))["stream"], saved["stream"]);
    assert!(
        sync(&f.opened).is_err(),
        "explicit sync alone admits transport settings"
    );
}
