use serde_json::json;
use streams_temper::{
    model::{Catalog, Record, RecordKey, Stream},
    operations::{Actor, Operation},
    sync::{
        envelope::Origin,
        history::{CommittedOperation, History},
        replay::{ReplayConflict, replay},
    },
};

fn creation(id: &str) -> History {
    let record = Record::Stream(Stream::new("work", "Work", None, "host").unwrap());
    let operation = Operation::new(id, None, record.clone(), Actor::User, None, vec![]).unwrap();
    History {
        catalog: Catalog::default().replacing(record).unwrap(),
        operations: vec![CommittedOperation {
            operation,
            origin: Origin::from([("host".into(), "host".into())]),
        }],
        ..History::default()
    }
    .validated()
    .unwrap()
}

#[test]
fn named_changes_keep_original_intent_and_provenance() {
    let local = creation("create");
    let projected = replay(&local, &History::default()).unwrap();
    assert_eq!(projected.catalog, local.catalog);
    assert!(projected.conflicts.is_empty());
    assert_eq!(projected.changes.len(), 1);
    let change = &projected.changes[0];
    assert_eq!(&change.operation, &local.operations[0].operation);
    assert_eq!(&change.origin, &local.operations[0].origin);
    assert_eq!(&change.record, change.operation.after());
}

#[test]
fn fingerprints_remember_intent_without_a_replayable_entry() {
    let local = creation("create");
    let remote = History {
        catalog: local.catalog.clone(),
        fingerprints: local.fingerprints.clone(),
        ..History::default()
    }
    .validated()
    .unwrap();
    assert!(remote.has_operation("create"));
    assert!(remote.operation("create").is_none());
    assert!(!remote.has_operation("missing"));
    let projected = replay(&local, &remote).unwrap();
    assert!(projected.changes.is_empty());
    assert!(projected.conflicts.is_empty());
}

#[test]
fn conflicting_creations_keep_the_existing_conflict_wire_shape() {
    let result = replay(&creation("local-create"), &creation("remote-create")).unwrap();
    assert_eq!(result.conflicts.len(), 1);
    let conflict = &result.conflicts[0];
    let expected = json!({
        "operation": "local-create",
        "target": {"kind": "stream", "id": "work", "host_id": null},
        "origin": {"host": "host"},
        "fields": ["identity"],
        "reason": "remote stream has a different creation",
    });
    assert_eq!(serde_json::to_value(conflict).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<ReplayConflict>(expected).unwrap(),
        *conflict
    );
}

#[test]
fn conflict_target_roundtrips_all_identity_shapes() {
    for (target, wire) in [
        (
            RecordKey::Host { id: "host".into() },
            json!({"kind": "host", "id": "host", "host_id": null}),
        ),
        (
            RecordKey::Stream { id: "work".into() },
            json!({"kind": "stream", "id": "work", "host_id": null}),
        ),
        (
            RecordKey::Session {
                id: "session".into(),
                host_id: "host".into(),
            },
            json!({"kind": "session", "id": "session", "host_id": "host"}),
        ),
    ] {
        let conflict = ReplayConflict {
            operation: "op".into(),
            target,
            origin: Origin::new(),
            fields: vec![],
            reason: String::new(),
        };
        let encoded = serde_json::to_value(&conflict).unwrap();
        assert_eq!(encoded["target"], wire);
        assert_eq!(
            serde_json::from_value::<ReplayConflict>(encoded.clone()).unwrap(),
            conflict
        );
        let mut invalid = encoded.clone();
        invalid["target"]["host_id"] = if conflict.target.host_id().is_some() {
            json!(null)
        } else {
            json!("host")
        };
        assert!(serde_json::from_value::<ReplayConflict>(invalid).is_err());
    }
}
