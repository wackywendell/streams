use serde_json::{Value, json};
use streams_temper::{
    codec::{record_from_data, record_to_data},
    model::RecordKind,
};

fn stream() -> Value {
    json!({"id": "work", "title": "Café", "goal": null, "owner": {"host_id": "host"}})
}

fn session() -> Value {
    json!({"id": "session", "host_id": "host", "stream_id": "work", "harness": "pi",
        "harness_session_id": "native", "title": "Session"})
}

fn host(checked_at: &str) -> Value {
    json!({"id": "host", "label": "Laptop", "checked_at": checked_at})
}

fn roundtrip(kind: RecordKind, data: &Value) -> Value {
    let record = record_from_data(kind, data).unwrap();
    let encoded = record_to_data(&record);
    assert_eq!(record_from_data(kind, &encoded).unwrap(), record);
    encoded
}

#[test]
fn unknown_fields_are_rejected_at_every_record_level() {
    for (kind, mut data) in [
        (RecordKind::Stream, stream()),
        (RecordKind::Session, session()),
        (RecordKind::Host, host("2025-01-02T03:04:05+00:00")),
    ] {
        data["extra"] = json!(true);
        assert!(record_from_data(kind, &data).is_err(), "{kind}");
    }
    for (field, value) in [
        ("owner", json!({"host_id": "host", "extra": true})),
        (
            "repositories",
            json!({"repo": {"url": "url", "extra": true}}),
        ),
        (
            "documents",
            json!([{"repository": "repo", "ref": "main", "path": "doc", "extra": true}]),
        ),
        ("issues", json!([{"url": "url", "extra": true}])),
        ("prs", json!([{"url": "url", "extra": true}])),
    ] {
        let mut data = stream();
        data["repositories"] = json!({"repo": {"url": "url"}});
        data[field] = value;
        assert!(
            record_from_data(RecordKind::Stream, &data).is_err(),
            "{field}"
        );
    }
    let mut data = session();
    data["checkout"] = json!({"repository": "repo", "ref": "main", "extra": true});
    assert!(record_from_data(RecordKind::Session, &data).is_err());
}

#[test]
fn goal_is_required_but_nullable_and_owner_is_nested() {
    let encoded = roundtrip(RecordKind::Stream, &stream());
    assert_eq!(encoded["goal"], Value::Null);
    assert_eq!(encoded["owner"], json!({"host_id": "host"}));
    assert!(encoded.get("owner_host_id").is_none());
    let mut data = stream();
    data.as_object_mut().unwrap().remove("goal");
    assert!(record_from_data(RecordKind::Stream, &data).is_err());
    for owner in [json!("host"), json!({}), Value::Null] {
        data = stream();
        data["owner"] = owner;
        assert!(record_from_data(RecordKind::Stream, &data).is_err());
    }
    data = stream();
    data.as_object_mut().unwrap().remove("owner");
    data["owner_host_id"] = json!("host");
    assert!(record_from_data(RecordKind::Stream, &data).is_err());
}

#[test]
fn defaults_apply_only_to_missing_nonnullable_fields() {
    let encoded = roundtrip(RecordKind::Stream, &stream());
    for field in ["title_mode", "goal_mode"] {
        assert_eq!(encoded[field], "automatic");
    }
    assert_eq!(encoded["status"], "active");
    assert_eq!(encoded["archived"], false);
    assert_eq!(encoded["repositories"], json!({}));
    assert_eq!(encoded["goal_short"], Value::Null);
    for field in ["documents", "issues", "prs"] {
        assert_eq!(encoded[field], json!([]));
    }
    for field in [
        "title_mode",
        "goal_mode",
        "status",
        "archived",
        "repositories",
        "documents",
        "issues",
        "prs",
    ] {
        let mut data = stream();
        data[field] = Value::Null;
        assert!(
            record_from_data(RecordKind::Stream, &data).is_err(),
            "{field}"
        );
    }
    let encoded = roundtrip(RecordKind::Session, &session());
    assert_eq!(encoded["title_mode"], "automatic");
    assert_eq!(encoded["status"], "active");
    assert_eq!(encoded["state"], "");
    assert_eq!(encoded["remaining"], "");
    assert_eq!(encoded["state_short"], Value::Null);
    assert_eq!(encoded["checkout"], Value::Null);
    for field in ["title_mode", "status", "state", "remaining"] {
        let mut data = session();
        data[field] = Value::Null;
        assert!(
            record_from_data(RecordKind::Session, &data).is_err(),
            "{field}"
        );
    }
    let mut data = session();
    data["checkout"] = json!({"repository": "repo", "ref": "main"});
    assert_eq!(
        roundtrip(RecordKind::Session, &data)["checkout"]["directory_within_checkout"],
        "."
    );
    data["checkout"]["directory_within_checkout"] = Value::Null;
    assert!(record_from_data(RecordKind::Session, &data).is_err());
}

#[test]
fn nested_descriptions_default_and_document_provenance_is_omitted_only_when_none() {
    let mut data = stream();
    data["repositories"] = json!({"repo": {"url": "url"}});
    data["documents"] = json!([{"repository": "repo", "ref": "main", "path": "doc"}]);
    data["issues"] = json!([{"url": "issue"}]);
    data["prs"] = json!([{"url": "pr"}]);
    let encoded = roundtrip(RecordKind::Stream, &data);
    for field in ["documents", "issues", "prs"] {
        assert_eq!(encoded[field][0]["description"], "");
        let mut invalid = data.clone();
        invalid[field][0]["description"] = Value::Null;
        assert!(
            record_from_data(RecordKind::Stream, &invalid).is_err(),
            "{field}"
        );
    }
    assert!(encoded["documents"][0].get("added_at").is_none());
    data["documents"][0]["added_at"] = Value::Null;
    assert!(
        roundtrip(RecordKind::Stream, &data)["documents"][0]
            .get("added_at")
            .is_none()
    );
    data["documents"][0]["added_at"] = json!("commit");
    assert_eq!(
        roundtrip(RecordKind::Stream, &data)["documents"][0]["added_at"],
        "commit"
    );
    let json = serde_json::to_string(&encoded).unwrap();
    assert!(json.contains("Café"));
    assert!(json.starts_with("{\"archived\":"));
}

#[test]
fn host_timestamps_preserve_python_precision_offsets_and_instant_equality() {
    for (input, expected) in [
        ("2025-01-02T03:04:05Z", "2025-01-02T03:04:05+00:00"),
        ("2025-01-02T03:04:05.1Z", "2025-01-02T03:04:05.100000+00:00"),
        (
            "2025-01-02T03:04:05.123456+00:00",
            "2025-01-02T03:04:05.123456+00:00",
        ),
        (
            "2025-01-02T04:04:05.001+01:00",
            "2025-01-02T04:04:05.001000+01:00",
        ),
    ] {
        assert_eq!(
            roundtrip(RecordKind::Host, &host(input))["checked_at"],
            expected
        );
    }
    assert_eq!(
        record_from_data(RecordKind::Host, &host("2025-01-02T03:04:05Z")).unwrap(),
        record_from_data(RecordKind::Host, &host("2025-01-02T04:04:05+01:00")).unwrap()
    );
    assert!(record_from_data(RecordKind::Host, &host("2025-01-02T03:04:05")).is_err());
    assert_eq!(
        record_from_data(RecordKind::Host, &host("2025-01-02T03:04:05.123456789Z"))
            .unwrap_err()
            .to_string(),
        "host.checked_at exceeds microsecond precision"
    );
}

#[test]
fn domain_validation_still_rejects_duplicate_collections() {
    for (field, item) in [
        (
            "documents",
            json!({"repository": "repo", "ref": "main", "path": "doc"}),
        ),
        ("issues", json!({"url": "issue"})),
        ("prs", json!({"url": "pr"})),
    ] {
        let mut data = stream();
        data["repositories"] = json!({"repo": {"url": "url"}});
        data[field] = json!([item.clone(), item]);
        assert_eq!(
            record_from_data(RecordKind::Stream, &data)
                .unwrap_err()
                .to_string(),
            format!("duplicate {field}")
        );
    }
    let mut data = stream();
    data["title"] = json!(1000.0);
    assert_eq!(
        record_from_data(RecordKind::Stream, &data)
            .unwrap_err()
            .to_string(),
        "stream.title must be a nonempty string"
    );
    data = stream();
    data["status"] = json!("invalid");
    assert!(
        record_from_data(RecordKind::Stream, &data)
            .unwrap_err()
            .to_string()
            .contains("unknown variant")
    );
    data = stream();
    data["goal_short"] = json!("summary");
    assert_eq!(
        record_from_data(RecordKind::Stream, &data)
            .unwrap_err()
            .to_string(),
        "stream.goal_short requires a goal"
    );
}
