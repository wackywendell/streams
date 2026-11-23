use serde_json::{Value, json};
use streams::{
    codec::{load_json, record_from_data, record_to_data},
    model::{RecordKind, Session, Stream},
};

fn stream_json() -> Value {
    json!({
        "host_id": "host",
        "id": "work",
        "title": {"text": "Café", "manual": false},
        "goal": null,
        "revision": 1,
        "updated_at_ms": 0
    })
}

fn session_json() -> Value {
    json!({
        "host_id": "host",
        "harness": "pi",
        "id": "native-1",
        "stream_id": "work",
        "title": {"text": "Session", "manual": false},
        "status": "active",
        "recap": null,
        "state": null,
        "remaining": null,
        "directory": null,
        "references": {},
        "artifacts": {},
        "revision": 1,
        "updated_at_ms": 0
    })
}

fn host_json() -> Value {
    json!({"id": "host", "label": "Laptop"})
}

fn roundtrip(kind: RecordKind, data: &Value) -> Value {
    let record = record_from_data(kind, data).unwrap();
    let encoded = record_to_data(&record);
    assert_eq!(record_from_data(kind, &encoded).unwrap(), record);
    encoded
}

#[test]
fn records_roundtrip_and_refuse_missing_required_or_nullable_fields() {
    roundtrip(RecordKind::Stream, &stream_json());
    roundtrip(RecordKind::Session, &session_json());
    roundtrip(RecordKind::Host, &host_json());

    let mut legacy_host = host_json();
    legacy_host["checked_at"] = json!("2025-01-02T03:04:05+00:00");
    assert!(record_from_data(RecordKind::Host, &legacy_host).is_err());

    for field in [
        "host_id",
        "id",
        "title",
        "goal",
        "revision",
        "updated_at_ms",
    ] {
        let mut missing = stream_json();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            record_from_data(RecordKind::Stream, &missing).is_err(),
            "{field}"
        );
    }
    for field in [
        "host_id",
        "harness",
        "id",
        "stream_id",
        "title",
        "status",
        "recap",
        "state",
        "remaining",
        "directory",
        "references",
        "artifacts",
        "revision",
        "updated_at_ms",
    ] {
        let mut missing = session_json();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            record_from_data(RecordKind::Session, &missing).is_err(),
            "{field}"
        );
    }
}

#[test]
fn duplicate_keys_and_unknown_fields_are_rejected() {
    let valid = serde_json::to_string(&stream_json()).unwrap();
    let duplicate = valid.replace(
        r#""host_id":"host""#,
        r#""host_id":"host","host_id":"other""#,
    );
    assert!(load_json(&duplicate).is_err());

    let mut unknown = stream_json();
    unknown["status"] = json!("active");
    assert!(record_from_data(RecordKind::Stream, &unknown).is_err());

    let mut old_session = session_json();
    old_session["membership_epoch"] = json!(0);
    assert!(record_from_data(RecordKind::Session, &old_session).is_err());
}

#[test]
fn positional_arrays_are_rejected_for_records_and_nested_objects() {
    for (kind, positional) in [
        (RecordKind::Host, json!(["host", "Laptop"])),
        (
            RecordKind::Stream,
            json!(["host", "work", ["Café", false], null, 1, 0]),
        ),
        (
            RecordKind::Session,
            json!(["host", "pi", "native-1", "work"]),
        ),
    ] {
        let err = record_from_data(kind, &positional).unwrap_err();
        assert!(err.to_string().contains("must be an object"), "{err}");
    }

    let mut bad_title = stream_json();
    bad_title["title"] = json!(["Café", false]);
    assert!(serde_json::from_value::<Stream>(bad_title).is_err());

    let mut bad_dir = session_json();
    bad_dir["directory"] = json!(["/tmp", null]);
    assert!(serde_json::from_value::<Session>(bad_dir).is_err());
}

#[test]
fn reference_keys_must_agree_with_entry_kind() {
    let mut valid_session = session_json();
    valid_session["references"] = json!({
        "https://github.com/o/r/pull/43": {
            "kind": "pr",
            "name": "PR",
            "purpose": "Implements it",
            "role": "deliverable",
            "status": "active",
            "manual": false
        },
        "docs/plan.md": {
            "kind": "document",
            "name": "Plan",
            "purpose": "Guides it",
            "role": "brief",
            "status": "active",
            "manual": true
        }
    });
    let record = record_from_data(RecordKind::Session, &valid_session).unwrap();
    record.validate().unwrap();

    // URL key with document kind is refused.
    let mut mismatch = session_json();
    mismatch["references"] = json!({
        "https://github.com/o/r/pull/43": {
            "kind": "document",
            "name": "Wrong",
            "purpose": "Mismatch",
            "role": "brief",
            "status": "active",
            "manual": false
        }
    });
    assert!(record_from_data(RecordKind::Session, &mismatch).is_err());

    // Path key with link kind is refused.
    let mut mismatch = session_json();
    mismatch["references"] = json!({
        "docs/plan.md": {
            "kind": "link",
            "name": "Wrong",
            "purpose": "Mismatch",
            "role": "brief",
            "status": "active",
            "manual": false
        }
    });
    assert!(record_from_data(RecordKind::Session, &mismatch).is_err());

    // Credential URL and `..` path are refused.
    for (key, kind) in [
        ("https://user:pass@example.com/a", "link"),
        ("../escape.md", "document"),
        ("/abs/path.md", "document"),
        ("a\\b.md", "document"),
    ] {
        let mut bad = session_json();
        bad["references"] = json!({
            (key): {
                "kind": kind,
                "name": "Bad",
                "purpose": "Bad",
                "role": "background",
                "status": "active",
                "manual": false
            }
        });
        assert!(
            record_from_data(RecordKind::Session, &bad).is_err(),
            "{key}"
        );
    }
}
