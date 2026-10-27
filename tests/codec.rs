use serde_json::{Value, json};
use streams::{
    codec::{record_from_data, record_to_data},
    model::{Document, RecordKind, Session, Stream},
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
fn object_helpers_do_not_hide_duplicate_named_fields() {
    let valid = serde_json::to_string(&stream()).unwrap();
    let duplicate = valid.replace(
        r#""host_id":"host""#,
        r#""host_id":"host","host_id":"other""#,
    );
    assert_ne!(valid, duplicate);
    let error = serde_json::from_str::<Stream>(&duplicate).unwrap_err();
    assert!(error.to_string().contains("duplicate"), "{error}");
}

#[test]
fn finite_wire_enums_accept_only_their_canonical_strings() {
    use serde::{Serialize, de::DeserializeOwned};
    use streams::{
        config::Strategy,
        model::{NamingMode, Status},
        operations::{Actor, DomainOutcome},
        sync::outcomes::{LocalStatus, SharingStatus},
    };
    fn check<T: Serialize + DeserializeOwned + PartialEq + std::fmt::Debug>(value: T) {
        let wire = serde_json::to_value(&value).unwrap();
        let name = wire.as_str().unwrap();
        assert_eq!(serde_json::from_value::<T>(wire.clone()).unwrap(), value);
        for invalid in [
            json!({(name): null}),
            json!([name]),
            Value::Null,
            json!(1),
            json!(true),
            json!("unknown-wire-enum"),
        ] {
            assert!(
                serde_json::from_value::<T>(invalid.clone()).is_err(),
                "{invalid}"
            );
        }
    }
    check(Status::Active);
    check(NamingMode::Automatic);
    check(RecordKind::Stream);
    check(Actor::User);
    check(DomainOutcome::NoChange);
    check(Strategy::None);
    check(LocalStatus::RepairRequired);
    check(SharingStatus::LocalOnly);
}

#[test]
fn positional_record_arrays_are_not_wire_objects() {
    let timestamp = "2025-01-02T03:04:05+00:00";
    for (kind, object, positional) in [
        (
            RecordKind::Host,
            host(timestamp),
            json!(["host", "Laptop", timestamp]),
        ),
        (
            RecordKind::Stream,
            stream(),
            json!([
                "work", "Café", null, {"host_id": "host"}, "automatic", null,
                "automatic", "active", false, {}, [], [], []
            ]),
        ),
        (
            RecordKind::Session,
            session(),
            json!([
                "session",
                "host",
                "work",
                "pi",
                "native",
                "Session",
                "automatic",
                "active",
                "",
                null,
                "",
                null
            ]),
        ),
    ] {
        record_from_data(kind, &object).unwrap();
        let error = record_from_data(kind, &positional).unwrap_err();
        assert!(
            error.to_string().contains("must be an object"),
            "{kind}: {error}"
        );
    }
}

#[test]
fn positional_nested_arrays_are_not_wire_objects() {
    for (field, object, positional) in [
        ("owner", json!({"host_id": "host"}), json!(["host"])),
        (
            "repositories",
            json!({"repo": {"url": "url"}}),
            json!({"repo": ["url"]}),
        ),
        (
            "documents",
            json!([{"repository": "repo", "ref": "main", "path": "doc"}]),
            json!([["repo", "main", "doc", "", null, null]]),
        ),
        ("issues", json!([{"url": "url"}]), json!([["url", ""]])),
        ("prs", json!([{"url": "url"}]), json!([["url", ""]])),
    ] {
        let mut data = stream();
        data["repositories"] = json!({"repo": {"url": "url"}});
        data[field] = object;
        record_from_data(RecordKind::Stream, &data).unwrap();
        serde_json::from_value::<Stream>(data.clone()).unwrap();
        data[field] = positional;
        assert!(serde_json::from_value::<Stream>(data.clone()).is_err());
        let error = record_from_data(RecordKind::Stream, &data).unwrap_err();
        assert!(
            error.to_string().contains("must be an object"),
            "{field}: {error}"
        );
    }
    let mut data = session();
    data["checkout"] = json!({"repository": "repo", "ref": "main"});
    record_from_data(RecordKind::Session, &data).unwrap();
    serde_json::from_value::<Session>(data.clone()).unwrap();
    data["checkout"] = json!(["repo", "main", "."]);
    assert!(serde_json::from_value::<Session>(data.clone()).is_err());
    let error = record_from_data(RecordKind::Session, &data).unwrap_err();
    assert!(error.to_string().contains("must be an object"), "{error}");
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
    assert_eq!(encoded["title_mode"], "inherit");
    assert_eq!(encoded["goal_mode"], "automatic");
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
    assert_eq!(encoded["title_mode"], "inherit");
    assert_eq!(encoded["status"], "active");
    assert_eq!(encoded["recap"], "");
    assert_eq!(encoded["recap_short"], Value::Null);
    assert_eq!(encoded["remaining_short"], Value::Null);
    assert_eq!(encoded["state"], "");
    assert_eq!(encoded["remaining"], "");
    assert_eq!(encoded["state_short"], Value::Null);
    assert_eq!(encoded["checkout"], Value::Null);
    for field in ["title_mode", "status", "recap", "state", "remaining"] {
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
    for field in ["commit", "source_session_id"] {
        assert!(encoded["documents"][0].get(field).is_none());
        data["documents"][0][field] = Value::Null;
        assert!(
            roundtrip(RecordKind::Stream, &data)["documents"][0]
                .get(field)
                .is_none()
        );
        data["documents"][0][field] = json!("provenance");
        assert_eq!(
            roundtrip(RecordKind::Stream, &data)["documents"][0][field],
            "provenance"
        );
        for invalid in [json!(""), json!(" \n"), json!("bad\u{0}text"), json!(42)] {
            let mut invalid_data = data.clone();
            invalid_data["documents"][0][field] = invalid;
            assert!(
                record_from_data(RecordKind::Stream, &invalid_data).is_err(),
                "{field}"
            );
        }
    }
    for value in [Value::Null, json!("retired-commit")] {
        let mut invalid = data.clone();
        invalid["documents"][0]["added_at"] = value;
        let error = record_from_data(RecordKind::Stream, &invalid).unwrap_err();
        assert!(error.to_string().contains("unknown field `added_at`"));
    }
    let json = serde_json::to_string(&encoded).unwrap();
    assert!(json.contains("Café"));
    assert!(json.starts_with("{\"archived\":"));
}

#[test]
fn document_constructor_sets_commit_and_provenance_does_not_change_identity() {
    let document = Document::new("repo", "main", "doc", "", None).unwrap();
    assert_eq!(document.commit, None);
    assert_eq!(document.source_session_id, None);
    let mut document = Document::new("repo", "main", "doc", "", Some("initial".into())).unwrap();
    assert_eq!(document.commit.as_deref(), Some("initial"));
    assert_eq!(document.source_session_id, None);
    document.commit = Some("latest".into());
    document.source_session_id = Some("session".into());
    document.r#ref = "topic".into();
    document.description = "updated".into();
    document.validate().unwrap();
    assert_eq!(document.identity(), ("repo", "doc"));
    assert!(Document::new("repo", "main", "doc", "", Some("".into())).is_err());
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
