use crate::{
    model::{Catalog, Host, Record, Session, Stream},
    operations::{Actor, Operation},
    sync::{
        envelope::Origin,
        history::{CommittedOperation, History},
        replay::{ConflictTarget, Replay, ReplayConflict, replay},
    },
};
use serde_json::json;

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
    let Replay::Ready { catalog, changes } = replay(&local, &History::default()).unwrap() else {
        panic!("expected a ready replay");
    };
    assert_eq!(catalog, local.catalog);
    assert_eq!(changes.len(), 1);
    let change = &changes[0];
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
    let Replay::Ready { catalog, changes } = replay(&local, &remote).unwrap() else {
        panic!("expected a ready replay");
    };
    assert_eq!(catalog, remote.catalog);
    assert!(changes.is_empty());
}

#[test]
fn conflicting_creations_keep_the_existing_conflict_wire_shape() {
    let Replay::Blocked(conflict) =
        replay(&creation("local-create"), &creation("remote-create")).unwrap()
    else {
        panic!("expected a blocked replay");
    };
    let expected = json!({
        "operation": "local-create",
        "target": {"kind": "stream", "id": "work", "host_id": null, "stream_id": null},
        "origin": {"host": "host"},
        "fields": ["identity"],
        "reason": "remote stream has a different creation",
    });
    assert_eq!(serde_json::to_value(&conflict).unwrap(), expected);
    assert_eq!(
        serde_json::from_value::<ReplayConflict>(expected).unwrap(),
        conflict
    );
}

#[test]
fn conflict_after_a_successful_change_exposes_only_the_conflict() {
    let host = Record::Host(
        Host::new(
            "host",
            "Host",
            chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z").unwrap(),
        )
        .unwrap(),
    );
    let host_operation =
        Operation::new("host-create", None, host.clone(), Actor::User, None, vec![]).unwrap();
    let prefix = History {
        catalog: Catalog::default().replacing(host.clone()).unwrap(),
        operations: vec![CommittedOperation {
            operation: host_operation,
            origin: Origin::new(),
        }],
        ..History::default()
    }
    .validated()
    .unwrap();
    let remote = creation("remote-create");
    let Replay::Ready { changes, .. } = replay(&prefix, &remote).unwrap() else {
        panic!("the earlier change must replay successfully on its own");
    };
    assert_eq!(changes.len(), 1);

    let mut local = creation("local-create");
    local.catalog = local.catalog.replacing(host).unwrap();
    local.operations.insert(0, prefix.operations[0].clone());
    let local = local.validated().unwrap();
    let original_local = local.clone();
    let original_remote = remote.clone();
    let result = replay(&local, &remote).unwrap();
    let Replay::Blocked(conflict) = result else {
        panic!("a blocked replay must not expose a catalog or changes");
    };
    assert_eq!(conflict.operation, "local-create");
    assert_eq!(conflict.fields, ["identity"]);
    assert_eq!(local, original_local);
    assert_eq!(remote, original_remote);
}

#[test]
fn conflict_target_roundtrips_all_identity_shapes() {
    for (record, wire) in [
        (
            Record::Host(Host::new("host", "Host", chrono::Utc::now().fixed_offset()).unwrap()),
            json!({"kind": "host", "id": "host", "host_id": null, "stream_id": null}),
        ),
        (
            Record::Stream(Stream::new("work", "Work", None, "host").unwrap()),
            json!({"kind": "stream", "id": "work", "host_id": null, "stream_id": null}),
        ),
        (
            Record::Session(
                Session::new("session", "host", "work", "pi", "native", "Session").unwrap(),
            ),
            json!({"kind": "session", "id": "session", "host_id": "host", "stream_id": "work"}),
        ),
    ] {
        let conflict = ReplayConflict {
            operation: "op".into(),
            target: ConflictTarget::from_record(&record),
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
        invalid["target"]["host_id"] = if conflict.target.key().host_id().is_some() {
            json!(null)
        } else {
            json!("host")
        };
        assert!(serde_json::from_value::<ReplayConflict>(invalid).is_err());
        let mut invalid = encoded;
        invalid["target"]["stream_id"] = if conflict.target.key().host_id().is_some() {
            json!(null)
        } else {
            json!("work")
        };
        assert!(serde_json::from_value::<ReplayConflict>(invalid).is_err());
    }
}
