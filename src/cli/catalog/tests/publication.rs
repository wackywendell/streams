use super::*;

#[test]
fn direct_human_edit_retry_after_a_later_edit_conflicts_and_keeps_the_later_record() {
    let f = Fixture::new();
    let created = f.create("Work", None);
    let id = created["id"].as_str().unwrap();
    let before = f.select(id);
    let changes = streams::operations::patches::StreamPatch {
        goal: Some(Some("First".into())),
        ..Default::default()
    };
    let first = application::save_stream_edit(
        f.opened.records(),
        &before,
        changes.clone(),
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
    let retry = application::save_stream_edit(
        f.opened.records(),
        &before,
        changes.clone(),
        TitleDefaults::default(),
    )
    .unwrap();
    assert!(!retry.written);
    assert_eq!(
        first.outcome.outcome,
        streams::operations::DomainOutcome::Changes
    );
    assert_eq!(
        retry.outcome.outcome,
        streams::operations::DomainOutcome::Conflict
    );
    assert_eq!(record_to_data(&retry.current.record), later["stream"]);
    assert_eq!(f.snapshot(), bytes);
}

#[test]
fn ordinary_human_edits_save_locally_without_opening_transport_or_publishing() {
    let f = Fixture::new();
    let stream = f.create("Work", None);
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
