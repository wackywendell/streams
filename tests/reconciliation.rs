use chrono::DateTime;
use serde_json::{Value, json};
use streams_temper::model::NamingMode;
use streams_temper::operations::{Actor, DomainOutcome, Reconciliation};
use streams_temper::{
    model::{Catalog, Host, Record, Repository, Session, Stream},
    operations::{Operation, reconcile, update},
};

fn stream() -> Record {
    let mut stream = Stream::new("work", "Work", None, "host").unwrap();
    stream.repositories.insert(
        "repo".to_string(),
        Repository::new("https://example.com/repo").unwrap(),
    );
    Record::Stream(stream)
}

fn edit(before: &Record, changes: Value) -> Operation {
    update(
        "edit",
        before,
        changes.as_object().unwrap(),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}

fn entry(field: &str, name: &str) -> Value {
    if field == "documents" {
        json!({"repository": "repo", "ref": "main", "path": name, "description": name})
    } else {
        json!({"url": format!("https://example.com/{name}"), "description": name})
    }
}

#[test]
fn only_stream_creations_establish_authority_for_either_actor() {
    for actor in [Actor::User, Actor::Automatic] {
        let creation = Operation::new("create", None, stream(), actor, None, vec![]).unwrap();
        let (id, authority) = creation.created_authority().unwrap();
        assert_eq!(id, "work");
        assert_eq!(authority.host_id, "host");
        assert_eq!(authority.token, "create");
    }
    let edited = edit(&stream(), json!({"title": "Edited"}));
    assert!(edited.created_authority().is_none());
    let session = Record::Session(
        Session::new("session", "host", "work", "pi", "native", "Session").unwrap(),
    );
    let host = Record::Host(
        Host::new(
            "host",
            "Host",
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z").unwrap(),
        )
        .unwrap(),
    );
    for actor in [Actor::User, Actor::Automatic] {
        for record in [&session, &host] {
            let token = matches!(record, Record::Session(_)).then(|| "create".into());
            let operation =
                Operation::new("create", None, record.clone(), actor, token, vec![]).unwrap();
            assert!(operation.created_authority().is_none());
        }
    }
}

#[test]
fn same_instant_at_different_offsets_is_not_a_changed_field() {
    let host = |timestamp| {
        Record::Host(
            Host::new(
                "host",
                "Host",
                DateTime::parse_from_rfc3339(timestamp).unwrap(),
            )
            .unwrap(),
        )
    };
    let before = host("2025-01-01T00:00:00Z");
    let after = host("2024-12-31T19:00:00-05:00");
    let operation = Operation::new(
        "edit",
        Some(before.clone()),
        after,
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(operation.changed_fields().is_empty());
    let catalog = Catalog::default().replacing(before).unwrap();
    assert_eq!(
        reconcile(&catalog, &operation, None).outcome(),
        DomainOutcome::NoChange
    );
}

#[test]
fn concurrent_additive_collections_keep_current_order_then_append_desired() {
    for field in ["documents", "issues", "prs"] {
        let old = entry(field, "old");
        let local = entry(field, "local");
        let remote = entry(field, "remote");
        let shared = entry(field, "shared");
        let before = edit(&stream(), json!({field: [old]})).after().clone();
        let operation = edit(&before, json!({field: [old, local, shared]}));
        let current = edit(&before, json!({field: [remote, shared, old]}))
            .after()
            .clone();
        let catalog = Catalog::default().replacing(current).unwrap();
        let result = reconcile(&catalog, &operation, None);
        assert_eq!(result.outcome(), DomainOutcome::Changes, "{field}");
        let Reconciliation::Changes { record, .. } = result else {
            panic!("expected changes");
        };
        let merged = streams_temper::codec::record_to_data(&record);
        assert_eq!(
            merged[field],
            json!([remote, shared, old, local]),
            "{field}"
        );
    }
}

#[test]
fn concurrent_removal_or_change_of_old_collection_entries_conflicts() {
    for field in ["documents", "issues", "prs"] {
        let old = entry(field, "old");
        let before = edit(&stream(), json!({field: [old]})).after().clone();
        let mut changed = old.clone();
        changed["description"] = json!("changed");
        for replacement in [json!([]), json!([changed])] {
            for mutate_desired in [false, true] {
                let additive = json!([old, entry(field, "added")]);
                let (desired, current) = if mutate_desired {
                    (replacement.clone(), additive)
                } else {
                    (additive, replacement.clone())
                };
                let operation = edit(&before, json!({field: desired}));
                let current = edit(&before, json!({field: current})).after().clone();
                let catalog = Catalog::default().replacing(current).unwrap();
                let result = reconcile(&catalog, &operation, None);
                assert_eq!(result.outcome(), DomainOutcome::Conflict, "{field}");
                assert_eq!(result.conflicts(), &[field], "{field}");
                assert!(matches!(result, Reconciliation::Conflict { .. }));
            }
        }
    }
}

#[test]
fn concurrent_additions_with_same_identity_but_different_contents_conflict() {
    for field in ["documents", "issues", "prs"] {
        let before = stream();
        let desired = entry(field, "new");
        let mut current = desired.clone();
        if field == "documents" {
            current["ref"] = json!("other-branch");
        } else {
            current["description"] = json!("other description");
        }
        let operation = edit(&before, json!({field: [desired]}));
        let current = edit(&before, json!({field: [current]})).after().clone();
        let catalog = Catalog::default().replacing(current).unwrap();
        let result = reconcile(&catalog, &operation, None);
        assert_eq!(result.outcome(), DomainOutcome::Conflict, "{field}");
        assert_eq!(result.conflicts(), &[field], "{field}");
    }
}

#[test]
fn unrelated_concurrent_field_edits_merge() {
    let before = stream();
    let operation = edit(&before, json!({"goal": "Ship the feature"}));
    let current = edit(&before, json!({"archived": true})).after().clone();
    let catalog = Catalog::default().replacing(current).unwrap();
    let result = reconcile(&catalog, &operation, None);
    assert_eq!(result.outcome(), DomainOutcome::Changes);
    let Reconciliation::Changes { record, .. } = result else {
        panic!("expected changes");
    };
    let Record::Stream(merged) = *record else {
        panic!("expected a stream");
    };
    assert_eq!(merged.goal.as_deref(), Some("Ship the feature"));
    assert_eq!(merged.goal_mode, NamingMode::Manual);
    assert!(merged.archived);
}

#[test]
fn applying_fields_normalizes_records_without_capturing_or_mutating_intent() {
    let Record::Stream(mut before) = stream() else {
        unreachable!()
    };
    before.goal = Some("Original".into());
    before.goal_short = Some("Summary".into());
    let before = Record::Stream(before);
    let unchanged = streams_temper::operations::apply_changes(
        &before,
        json!({"goal":"Original"}).as_object().unwrap(),
        Actor::Automatic,
    )
    .unwrap();
    assert_eq!(unchanged, before);
    let changes = json!({"title":"Renamed", "goal":"Revised"});
    let after = streams_temper::operations::apply_changes(
        &before,
        changes.as_object().unwrap(),
        Actor::User,
    )
    .unwrap();
    assert_eq!(edit(&before, changes).after(), &after);
    let Record::Stream(after) = after else {
        unreachable!()
    };
    assert_eq!(after.title_mode, NamingMode::Manual);
    assert_eq!(after.goal_mode, NamingMode::Manual);
    assert_eq!(after.goal_short, None);
    assert!(
        streams_temper::operations::apply_changes(
            &before,
            json!({"goal":1}).as_object().unwrap(),
            Actor::User,
        )
        .is_err()
    );
    let Record::Stream(before) = before else {
        unreachable!()
    };
    assert_eq!(before.title, "Work");
    assert_eq!(before.goal_short.as_deref(), Some("Summary"));
}

#[test]
fn reconciliation_receipts_keep_the_four_canonical_wire_shapes() {
    let record = stream().validated().unwrap();
    let record_wire = serde_json::to_string(&record).unwrap();
    let cases = [
        (
            Reconciliation::Changes { operation_id: "original".into(), record: Box::new(record.clone()) },
            format!(r#"{{"operation_id":"original","outcome":"changes","record":{record_wire},"conflicts":[],"reason":""}}"#),
        ),
        (
            Reconciliation::no_change("original", "already saved"),
            r#"{"operation_id":"original","outcome":"no-change","record":null,"conflicts":[],"reason":"already saved"}"#.into(),
        ),
        (
            Reconciliation::Superseded {
                operation_id: "original".into(),
                conflicts: vec!["inputs".into()],
                reason: "captured catalog input changed".into(),
            },
            r#"{"operation_id":"original","outcome":"superseded","record":null,"conflicts":["inputs"],"reason":"captured catalog input changed"}"#.into(),
        ),
        (
            Reconciliation::Conflict {
                operation_id: "original".into(),
                conflicts: vec!["identity".into()],
                reason: "record already exists".into(),
            },
            r#"{"operation_id":"original","outcome":"conflict","record":null,"conflicts":["identity"],"reason":"record already exists"}"#.into(),
        ),
    ];
    for (receipt, expected) in cases {
        assert_eq!(receipt.operation_id(), "original");
        assert_eq!(serde_json::to_string(&receipt).unwrap(), expected);
        let wire: Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(wire["outcome"], json!(receipt.outcome()));
        assert_eq!(wire["conflicts"], json!(receipt.conflicts()));
        assert_eq!(wire["reason"], receipt.reason());
        if let Reconciliation::Changes {
            record: changed, ..
        } = receipt
        {
            assert_eq!(*changed, record);
            assert_eq!(wire["record"], serde_json::to_value(&record).unwrap());
        } else {
            assert!(wire["record"].is_null());
        }
    }
}

#[test]
fn creation_changed_fields_use_all_logical_names() {
    let operation = Operation::new("create", None, stream(), Actor::User, None, vec![]).unwrap();
    let fields = operation.changed_fields();
    assert_eq!(fields.len(), 13);
    assert!(fields.contains("id"));
    assert!(fields.contains("owner_host_id"));
    assert!(!fields.contains("owner"));
}
