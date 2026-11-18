use serde_json::{Value, json};
use streams::{
    codec::{record_from_data, record_to_data},
    model::{RecordKind, Session, Stream},
};

fn stream() -> Value {
    json!({"id": "work", "title": "Café", "goal": null, "owner": {"host_id": "host"}, "references":[],
        "revision": 1, "updated_at_ms": 0})
}

fn session() -> Value {
    json!({"id": "session", "host_id": "host", "stream_id": "work", "membership_epoch": 0,
        "harness": "pi", "harness_session_id": "native", "title": "Session", "revision": 1,
        "updated_at_ms": 0})
}

#[test]
fn records_without_their_stamps_or_membership_epoch_are_refused() {
    for (kind, record) in [
        (RecordKind::Stream, stream()),
        (RecordKind::Session, session()),
    ] {
        record_from_data(kind, &record).unwrap();
        for field in ["revision", "updated_at_ms", "membership_epoch"] {
            if kind == RecordKind::Stream && field == "membership_epoch" {
                continue;
            }
            let mut missing = record.clone();
            missing.as_object_mut().unwrap().remove(field);
            let error = record_from_data(kind, &missing).unwrap_err();
            assert!(
                error.to_string().contains(field),
                "{kind:?} {field}: {error}"
            );
        }
    }
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
            "references",
            json!([reference()]),
            json!([["Reference", "Purpose", "brief", "active", {}, "revision"]]),
        ),
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
    assert!(encoded.get("status").is_none());
    assert_eq!(encoded["repositories"], json!({}));
    assert_eq!(encoded["goal_short"], Value::Null);
    assert_eq!(encoded["references"], json!([]));
    for field in ["title_mode", "goal_mode", "repositories", "references"] {
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
fn references_are_required_and_old_collections_are_rejected() {
    let mut missing = stream();
    missing.as_object_mut().unwrap().remove("references");
    assert!(record_from_data(RecordKind::Stream, &missing).is_err());
    assert!(record_from_data(RecordKind::Stream, &stream()).is_ok());
    for field in ["documents", "issues", "prs"] {
        let mut invalid = stream();
        invalid[field] = json!([]);
        assert!(
            record_from_data(RecordKind::Stream, &invalid).is_err(),
            "{field}"
        );
    }
    let mut data = stream();
    let mut document = reference();
    document["target"] =
        json!({"kind":"document","repository_url":"git@example.test:repo","path":"docs/design.md"});
    data["references"] = json!([document]);
    let encoded = roundtrip(RecordKind::Stream, &data);
    for field in ["ref", "commit", "source_session_id"] {
        assert!(encoded["references"][0]["target"].get(field).is_none());
        data["references"][0]["target"][field] = Value::Null;
        assert!(
            roundtrip(RecordKind::Stream, &data)["references"][0]["target"]
                .get(field)
                .is_none()
        );
        data["references"][0]["target"][field] = json!("established");
        assert_eq!(
            roundtrip(RecordKind::Stream, &data)["references"][0]["target"][field],
            "established"
        );
    }
    assert!(serde_json::to_string(&encoded).unwrap().contains("Café"));
}

fn reference() -> Value {
    json!({
        "name": "Reference", "purpose": "Define acceptance", "role": "brief", "state": "active",
        "target": {"kind": "link", "url": "https://EXAMPLE.test/brief"}
    })
}

#[test]
fn standalone_references_have_strict_shapes_and_exact_resource_keys() {
    use streams::model::{Reference, ReferenceTarget, ReferenceValue, ResourceKey};
    for kind in ["pr", "issue", "link", "document"] {
        let mut wire = reference();
        wire["target"] = if kind == "document" {
            json!({"kind":kind,"repository_url":"git@example.test:tools","path":"docs/design.md"})
        } else {
            json!({"kind":kind,"url":"https://EXAMPLE.test/item"})
        };
        let saved: Reference = serde_json::from_value(wire.clone()).unwrap();
        saved.validate().unwrap();
        assert_eq!(serde_json::to_value(&saved).unwrap(), wire);
        let key = saved.target.key();
        key.validate().unwrap();
        assert_eq!(
            serde_json::from_value::<ResourceKey>(json!(key)).unwrap(),
            key
        );
        assert_eq!(Reference::new(saved.value()).unwrap(), saved);
        if kind == "document" {
            let mut changed = saved.clone();
            if let ReferenceTarget::Document(document) = &mut changed.target {
                document.r#ref = Some("topic".into());
                document.commit = Some("new-commit".into());
                document.source_session_id = Some("source".into());
            }
            assert_eq!(changed.target.key(), key);
        } else {
            for target in [
                ReferenceTarget::Pr {
                    url: "https://EXAMPLE.test/item".into(),
                },
                ReferenceTarget::Issue {
                    url: "https://EXAMPLE.test/item".into(),
                },
                ReferenceTarget::Link {
                    url: "https://EXAMPLE.test/item".into(),
                },
            ] {
                assert_eq!(target.key(), key);
            }
            assert_ne!(
                ReferenceTarget::Link {
                    url: "https://example.test/item".into()
                }
                .key(),
                key
            );
        }
        for field in ["extra", "actor", "revision"] {
            let mut invalid = wire.clone();
            invalid[field] = json!(true);
            assert!(serde_json::from_value::<Reference>(invalid).is_err());
        }
        let mut invalid = wire.clone();
        invalid["target"]["extra"] = json!(true);
        assert!(serde_json::from_value::<Reference>(invalid).is_err());
        let mut invalid = json!(key);
        invalid["extra"] = json!(true);
        assert!(serde_json::from_value::<ResourceKey>(invalid).is_err());
        for field in ["name", "purpose", "role", "state", "target"] {
            let mut invalid = wire.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<Reference>(invalid).is_err(),
                "{field}"
            );
        }
        let mut value = wire;
        let desired: ReferenceValue = serde_json::from_value(value.clone()).unwrap();
        desired.validate().unwrap();
        assert_eq!(serde_json::to_value(desired).unwrap(), value);
        value["revision"] = json!("extra");
        assert!(serde_json::from_value::<ReferenceValue>(value).is_err());
    }
}

#[test]
fn standalone_document_recovery_is_nullable_optional_and_nonempty() {
    use streams::model::{Reference, RepositoryDocument};
    let mut wire = reference();
    wire["target"] = json!({"kind":"document","repository_url":"git@example.test:tools","path":"docs/design.md"});
    for field in ["ref", "commit", "source_session_id"] {
        for value in [Value::Null, json!("established")] {
            let mut input = wire.clone();
            input["target"][field] = value.clone();
            let saved: Reference = serde_json::from_value(input).unwrap();
            saved.validate().unwrap();
            let encoded = serde_json::to_value(saved).unwrap();
            if value.is_null() {
                assert!(encoded["target"].get(field).is_none());
            } else {
                assert_eq!(encoded["target"][field], value);
            }
        }
        for invalid in [
            json!(""),
            json!(" \n"),
            json!("\u{85}"),
            json!("bad\u{0}text"),
        ] {
            let mut input = wire.clone();
            input["target"][field] = invalid;
            let saved: Reference = serde_json::from_value(input).unwrap();
            assert!(saved.validate().is_err(), "{field}");
        }
    }
    for path in [
        "/absolute",
        "C:drive",
        "docs\\file",
        "../parent",
        "docs/../file",
        "",
        "\u{85}",
    ] {
        let mut input = wire.clone();
        input["target"]["path"] = json!(path);
        let saved: Reference = serde_json::from_value(input).unwrap();
        assert!(saved.validate().is_err(), "{path:?}");
    }
    let mut input = json!({"repository_url":"repo","path":"doc","description":"old relationship"});
    assert!(serde_json::from_value::<RepositoryDocument>(input.clone()).is_err());
    input.as_object_mut().unwrap().remove("description");
    assert!(
        serde_json::from_value::<RepositoryDocument>(input)
            .unwrap()
            .validate()
            .is_ok()
    );
}

#[test]
fn reference_text_validation_preserves_rust_unicode_acceptance() {
    use streams::model::Reference;
    for field in ["name", "purpose"] {
        for text in [
            "\u{feff}",
            "😀",
            "leading\nnewline",
            "--value='exact'",
            "\u{1b}",
        ] {
            let mut wire = reference();
            wire[field] = json!(text);
            let saved: Reference = serde_json::from_value(wire.clone()).unwrap();
            saved.validate().unwrap();
            assert_eq!(serde_json::to_value(saved).unwrap(), wire);
        }
        for text in ["", " \n", "\u{85}", "\u{a0}\u{3000}", "bad\u{0}text"] {
            let mut wire = reference();
            wire[field] = json!(text);
            let saved: Reference = serde_json::from_value(wire).unwrap();
            assert!(saved.validate().is_err(), "{field}: {text:?}");
        }
    }
    let raw = serde_json::to_string(&reference()).unwrap();
    assert!(serde_json::from_str::<Reference>(&raw.replace("Reference", "\\ud800")).is_err());
    assert!(serde_json::from_str::<Reference>(&raw.replace("Reference", "\\udfff")).is_err());
}

#[test]
fn document_reference_text_fields_share_exact_unicode_validation() {
    for field in [
        "repository_url",
        "path",
        "ref",
        "commit",
        "source_session_id",
    ] {
        for text in ["\u{feff}", "😀", "line\nvalue", "\u{1b}"] {
            let mut wire = reference();
            wire["target"] = json!({"kind":"document","repository_url":"repo","path":"doc"});
            wire["target"][field] = json!(text);
            let mut data = stream();
            data["references"] = json!([wire]);
            assert_eq!(
                roundtrip(RecordKind::Stream, &data)["references"][0]["target"][field],
                text,
                "{field}"
            );
        }
        for text in ["", " \n", "\u{85}", "bad\u{0}text"] {
            let mut wire = reference();
            wire["target"] = json!({"kind":"document","repository_url":"repo","path":"doc"});
            wire["target"][field] = json!(text);
            let mut data = stream();
            data["references"] = json!([wire]);
            assert!(
                record_from_data(RecordKind::Stream, &data).is_err(),
                "{field}: {text:?}"
            );
        }
    }
}

#[test]
fn reference_url_policy_matches_painting_and_preserves_saved_spelling() {
    use streams::model::{Reference, reference_url};
    for url in [
        "https://EXAMPLE.test/path",
        "http://localhost:8080/a%20b",
        "https://[::1]/",
        "https://example.test/%F0%9F%98%80",
    ] {
        assert!(reference_url(url).is_some(), "{url}");
        let mut wire = reference();
        wire["target"]["url"] = json!(url);
        let saved: Reference = serde_json::from_value(wire.clone()).unwrap();
        saved.validate().unwrap();
        assert_eq!(serde_json::to_value(saved).unwrap(), wire);
    }
    for url in [
        "file:///tmp/doc",
        "javascript:alert(1)",
        "https://example.com/%0Aevil",
        "https://example.com/%E2%80%AEevil",
        "https://example.com/%ZZ",
        "https://example.com/a\\b",
        "https://example.com/a b",
        "https://",
        "https://user@example.test",
        "https://user:secret@example.test",
        "https://:secret@example.test",
        "https://example.test/a b",
        "https://example.test/\\path",
        "https://example.test/\u{85}",
        "https://example.test/\u{feff}",
        "https://example.test/%",
        "https://example.test/%0",
        "https://example.test/%gg",
        "https://example.test/%FF",
        "https://example.test/%ED%A0%80",
        "https://example.test/%00",
        "https://example.test/%1b",
        "https://example.test/%E2%80%AE",
    ] {
        assert!(reference_url(url).is_none(), "{url}");
        let mut wire = reference();
        wire["target"]["url"] = json!(url);
        let saved: Reference = serde_json::from_value(wire).unwrap();
        assert!(saved.validate().is_err(), "{url}");
    }
}

#[test]
fn reference_result_wire_has_strict_action_outcome_and_property_shapes() {
    use streams::operations::references::ReferenceResult;
    for action in ["add", "update", "drop", "restore", "delete"] {
        for outcome in ["changes", "no-change", "superseded", "conflict"] {
            let wire = json!({"key":{"kind":"url","url":"https://example.test/item"},"desired":null,"action":action,"outcome":outcome,"reason":""});
            let result: ReferenceResult = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(serde_json::to_value(result).unwrap(), wire);
            for field in ["key", "action", "outcome", "reason"] {
                let mut invalid = wire.clone();
                invalid.as_object_mut().unwrap().remove(field);
                assert!(serde_json::from_value::<ReferenceResult>(invalid).is_err());
            }
            for field in ["action", "outcome"] {
                let mut invalid = wire.clone();
                invalid[field] = json!({wire[field].as_str().unwrap():null});
                assert!(serde_json::from_value::<ReferenceResult>(invalid).is_err());
            }
            let mut invalid = wire.clone();
            invalid["extra"] = json!(true);
            assert!(serde_json::from_value::<ReferenceResult>(invalid).is_err());
        }
    }
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
    for kind in ["pr", "issue", "link", "document"] {
        let mut first = reference();
        if kind == "document" {
            first["target"] = json!({"kind":"document","repository_url":"repo","path":"docs/design.md","ref":"main"});
        }
        let mut second = first.clone();
        if kind == "document" {
            second["target"]["ref"] = json!("other");
        } else {
            second["target"]["kind"] = json!(kind);
        }
        let mut data = stream();
        data["references"] = json!([first, second]);
        assert_eq!(
            record_from_data(RecordKind::Stream, &data)
                .unwrap_err()
                .to_string(),
            "duplicate reference resource key"
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
    let mut session_data = session();
    session_data["status"] = json!("invalid");
    assert!(
        record_from_data(RecordKind::Session, &session_data)
            .unwrap_err()
            .to_string()
            .contains("unknown variant")
    );
    // Stream records reject unknown `status` fields (format v4 does not store Stream.status).
    data = stream();
    data["status"] = json!("active");
    assert!(
        record_from_data(RecordKind::Stream, &data)
            .unwrap_err()
            .to_string()
            .contains("unknown field")
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
