use serde_json::{Map, json};
use streams_temper::{
    codec::{record_from_data, record_to_data},
    model::{Catalog, Record, Stream},
    operations::{Operation, reconcile, update},
    sync::{
        envelope::{decode_operation, encode_operation, load_json},
        local::LocalCatalog,
        outcomes::LocalStatus,
    },
    yaml_store::{YamlStore, encode_component},
};

fn creation(id: &str) -> Operation {
    Operation::new(
        id,
        None,
        Record::Stream(Stream::new("work", "Work", None, "host").unwrap()),
        "user",
        None,
        vec![],
    )
    .unwrap()
}

#[test]
fn record_and_operation_roundtrip() {
    let op = creation("create");
    let data = record_to_data(op.after());
    assert_eq!(&record_from_data("stream", &data).unwrap(), op.after());
    let encoded = encode_operation(&op, None).unwrap();
    assert_eq!(decode_operation(&encoded).unwrap().0, op);
    assert!(load_json(r#"{"a":1,"a":2}"#).is_err());
    assert!(record_from_data("stream", &json!({"extra": true})).is_err());
}

#[test]
fn local_save_retry_and_changed_identity_are_fenced() {
    let dir = tempfile::tempdir().unwrap();
    let local = LocalCatalog::new(YamlStore::new(dir.path()), "host").unwrap();
    let op = creation("create");
    let saved = local.save(&op, None).unwrap();
    assert_eq!(saved.local, LocalStatus::Persisted);
    assert!(saved.accepted());
    let reopened = LocalCatalog::new(YamlStore::new(dir.path()), "host").unwrap();
    assert_eq!(
        reopened.save(&op, None).unwrap().domain.unwrap().outcome,
        "no-change"
    );
    assert_eq!(reopened.authority("work").unwrap().unwrap().token, "create");
    let different = Operation::new(
        "create",
        None,
        Record::Stream(Stream::new("work", "Other", None, "host").unwrap()),
        "user",
        None,
        vec![],
    )
    .unwrap();
    assert!(reopened.save(&different, None).unwrap().requires_repair());
    assert!(!dir.path().join(".git").exists());
    assert!(!dir.path().join(".catalog-sync/save.json").exists());
}

#[test]
fn user_manual_mode_protects_against_automatic_changes() {
    let create = creation("create");
    let catalog = Catalog::default()
        .replacing(create.after().clone())
        .unwrap();
    let changes: Map<_, _> = json!({"title":"Chosen"}).as_object().unwrap().clone();
    let op = update("edit", create.after(), &changes, "user", None, vec![]).unwrap();
    let result = reconcile(&catalog, &op, None);
    let Record::Stream(s) = result.record.unwrap() else {
        panic!()
    };
    assert_eq!(s.title_mode, "manual");
    let data = record_to_data(&Record::Stream(s));
    assert_eq!(data["title"], "Chosen");
}

#[test]
fn snapshot_compare_and_swap_and_path_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    store
        .with_transaction(|tx| {
            let empty = tx.read()?;
            let rec = creation("create").after().clone();
            tx.save(&rec, &empty)?;
            assert!(tx.save(&rec, &empty).unwrap_err().is_revision_conflict);
            Ok(())
        })
        .unwrap();
    assert_eq!(encode_component("../A/é"), "..%2F%41%2F%C3%A9");
    assert_eq!(encode_component(".."), "%2E%2E");
}
