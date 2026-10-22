use std::collections::BTreeMap;

use streams_temper::storage::{StorageError, Store, Transaction};

use super::*;

fn journal(f: &Fixture) -> BTreeMap<String, serde_json::Map<String, Value>> {
    f.store
        .with_transaction::<_, StorageError>(|tx| {
            tx.journal()
                .names()?
                .into_iter()
                .map(|name| {
                    let data = tx.journal().read(&name)?.unwrap();
                    Ok((name, data))
                })
                .collect()
        })
        .unwrap()
}

#[test]
fn captured_apply_preserves_original_identity_base_and_stable_receipts() {
    let f = Fixture::new();
    let stream = f.create("Work", Some("Original"), Status::Waiting);
    let id = stream["id"].as_str().unwrap();
    let request = json!({
        "operation_id": "captured-update",
        "catalog_source": "physical-source",
        "actor": "user",
        "kind": "stream",
        "before": stream,
        "changes": {"goal": "Captured"},
        "inputs": [],
    });
    let before = f.backend.snapshot().unwrap();
    let before_journal = journal(&f);
    let first = ok(apply(&f.backend, &request.to_string(), "physical-source"));
    assert_eq!(first["operation_id"], "captured-update");
    assert_eq!(first["save"]["domain"]["outcome"], "changes");
    assert_eq!(first["stream"]["goal"], "Captured");
    let accepted = f.backend.snapshot().unwrap();
    let accepted_journal = journal(&f);
    assert_ne!(accepted, before);
    assert_ne!(accepted_journal, before_journal);
    assert_eq!(ok(f.show(id))["stream"], first["stream"]);

    let retry = ok(apply(&f.backend, &request.to_string(), "physical-source"));
    assert_eq!(retry["operation_id"], "captured-update");
    assert_eq!(retry["save"]["domain"]["outcome"], "no-change");
    assert_eq!(retry["stream"], first["stream"]);
    assert_eq!(f.backend.snapshot().unwrap(), accepted);
    assert_eq!(journal(&f), accepted_journal);

    let mut wrong_source = request.clone();
    wrong_source["catalog_source"] = json!("wrong");
    let error = apply(&f.backend, &wrong_source.to_string(), "physical-source")
        .err()
        .unwrap()
        .data();
    assert!(error.get("error").is_some());
    assert_eq!(f.backend.snapshot().unwrap(), accepted);
    assert_eq!(journal(&f), accepted_journal);

    let mut stale = request;
    stale["operation_id"] = json!("new-stale-intent");
    stale["changes"] = json!({"goal": "Different"});
    let rejected = apply(&f.backend, &stale.to_string(), "physical-source").unwrap();
    assert_eq!(rejected.code, 2, "{}", rejected.value);
    assert_eq!(rejected.value["operation_id"], "new-stale-intent");
    assert_eq!(rejected.value["save"]["domain"]["outcome"], "conflict");
    assert_eq!(f.backend.snapshot().unwrap(), accepted);
    assert_eq!(journal(&f), accepted_journal);
    assert_eq!(ok(f.show(id))["stream"]["goal"], "Captured");
}
