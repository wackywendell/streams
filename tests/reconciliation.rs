use chrono::DateTime;
use serde_json::{Value, json};
use streams_temper::{
    model::{Catalog, Host, Record, Repository, Stream},
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
        "user",
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
    let operation =
        Operation::new("edit", Some(before.clone()), after, "user", None, vec![]).unwrap();
    assert!(operation.changed_fields().is_empty());
    let catalog = Catalog::default().replacing(before).unwrap();
    assert_eq!(reconcile(&catalog, &operation, None).outcome, "no-change");
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
        assert_eq!(result.outcome, "changes", "{field}");
        let merged = streams_temper::codec::record_to_data(&result.record.unwrap());
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
                assert_eq!(result.outcome, "conflict", "{field}");
                assert_eq!(result.conflicts, vec![field], "{field}");
                assert!(result.record.is_none());
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
        assert_eq!(result.outcome, "conflict", "{field}");
        assert_eq!(result.conflicts, vec![field], "{field}");
    }
}

#[test]
fn unrelated_concurrent_field_edits_merge() {
    let before = stream();
    let operation = edit(&before, json!({"goal": "Ship the feature"}));
    let current = edit(&before, json!({"archived": true})).after().clone();
    let catalog = Catalog::default().replacing(current).unwrap();
    let result = reconcile(&catalog, &operation, None);
    assert_eq!(result.outcome, "changes");
    let Record::Stream(merged) = result.record.unwrap() else {
        panic!("expected a stream");
    };
    assert_eq!(merged.goal.as_deref(), Some("Ship the feature"));
    assert_eq!(merged.goal_mode, "manual");
    assert!(merged.archived);
}

#[test]
fn creation_changed_fields_use_all_logical_names() {
    let operation = Operation::new("create", None, stream(), "user", None, vec![]).unwrap();
    let fields = operation.changed_fields();
    assert_eq!(fields.len(), 13);
    assert!(fields.contains("id"));
    assert!(fields.contains("owner_host_id"));
    assert!(!fields.contains("owner"));
}
