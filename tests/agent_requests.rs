use serde_json::{Value, json};
use streams_temper::{
    agent::parse_update,
    codec::record_to_data,
    model::{Host, NamingMode, Record, Stream},
    operations::Actor,
};

fn stream() -> Record {
    Record::Stream(Stream::new("work", "Captured", Some("Old goal".into()), "host").unwrap())
}

fn request(before: &Record, changes: Value, actor: Actor) -> Value {
    json!({
        "operation_id": "original-operation",
        "catalog_source": "physical-source",
        "kind": before.kind(),
        "actor": actor,
        "before": record_to_data(before),
        "changes": changes,
        "ownership_token": (actor == Actor::Automatic).then_some("create"),
        "inputs": [],
    })
}

#[test]
fn parsed_intent_keeps_original_identity_base_and_supporting_inputs() {
    let before = stream();
    let supporting = Record::Host(
        Host::new(
            "host",
            "Captured host",
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap(),
        )
        .unwrap(),
    );
    let mut wire = request(&before, json!({"goal": "Generated"}), Actor::Automatic);
    wire["inputs"] = json!([{ "kind": "host", "record": record_to_data(&supporting) }]);
    let operation = parse_update(&wire.to_string(), "physical-source").unwrap();
    assert_eq!(operation.id(), "original-operation");
    assert_eq!(operation.before(), Some(&before));
    assert_eq!(operation.inputs(), &[supporting]);
    assert_eq!(operation.ownership_token(), Some("create"));
    assert_ne!(operation.after(), &before);
}

#[test]
fn positional_requests_and_captured_inputs_are_rejected() {
    let wire = request(&stream(), json!({"title": "Chosen"}), Actor::User);
    parse_update(&wire.to_string(), "physical-source").unwrap();
    let positional = json!([
        wire["operation_id"],
        wire["catalog_source"],
        wire["kind"],
        wire["actor"],
        wire["before"],
        wire["changes"],
        wire["ownership_token"],
        wire["inputs"]
    ]);
    assert!(parse_update(&positional.to_string(), "physical-source").is_err());

    let host = json!({"id": "host", "label": "Captured", "checked_at": "2026-01-01T00:00:00Z"});
    let mut captured = wire;
    captured["inputs"] = json!([{"kind": "host", "record": host}]);
    parse_update(&captured.to_string(), "physical-source").unwrap();
    captured["inputs"] = json!([["host", host]]);
    assert!(parse_update(&captured.to_string(), "physical-source").is_err());
    captured["inputs"] = json!([{
        "kind": "host", "record": ["host", "Captured", "2026-01-01T00:00:00Z"]
    }]);
    assert!(parse_update(&captured.to_string(), "physical-source").is_err());
    captured["inputs"] = json!([]);
    captured["before"]["owner"] = json!(["host"]);
    assert!(parse_update(&captured.to_string(), "physical-source").is_err());
}

#[test]
fn automatic_manual_text_is_filtered_before_domain_construction() {
    let Record::Stream(mut before) = stream() else {
        unreachable!()
    };
    before.title = "My title".into();
    before.title_mode = NamingMode::Manual;
    let wire = request(
        &Record::Stream(before.clone()),
        json!({"title": "Bot title", "goal": "New goal"}),
        Actor::Automatic,
    );
    let operation = parse_update(&wire.to_string(), "physical-source").unwrap();
    let Record::Stream(after) = operation.after() else {
        unreachable!()
    };
    assert_eq!(after.title, "My title");
    assert_eq!(after.title_mode, NamingMode::Manual);
    assert_eq!(after.goal.as_deref(), Some("New goal"));
    assert!(!operation.changed_fields().contains("title"));
}

#[test]
fn clearing_goal_normalizes_dependent_summary_before_validation() {
    let Record::Stream(mut before) = stream() else {
        unreachable!()
    };
    before.goal_short = Some("Old summary".into());
    let wire = request(&Record::Stream(before), json!({"goal": null}), Actor::User);
    let operation = parse_update(&wire.to_string(), "physical-source").unwrap();
    let Record::Stream(after) = operation.after() else {
        unreachable!()
    };
    assert!(after.goal.is_none());
    assert!(after.goal_short.is_none());
    assert_eq!(after.goal_mode, NamingMode::Manual);
}

#[test]
fn source_fences_and_request_allowlists_fail_closed() {
    let wire = request(&stream(), json!({}), Actor::User);
    assert!(parse_update(&wire.to_string(), "other-physical-source").is_err());
    for field in [
        "operation_id",
        "catalog_source",
        "kind",
        "actor",
        "before",
        "changes",
    ] {
        let mut broken = wire.clone();
        broken.as_object_mut().unwrap().remove(field);
        assert!(
            parse_update(&broken.to_string(), "physical-source").is_err(),
            "{field}"
        );
    }
    for actor in [Actor::Automatic, Actor::User] {
        for field in ["id", "owner", "owner_host_id", "repositories", "checkout"] {
            let wire = request(&stream(), json!({field: "not editable"}), actor);
            assert!(
                parse_update(&wire.to_string(), "physical-source").is_err(),
                "{actor}:{field}"
            );
        }
    }
    let mut prohibited = request(&stream(), json!({"status": "done"}), Actor::Automatic);
    assert!(parse_update(&prohibited.to_string(), "physical-source").is_err());
    prohibited["changes"] = json!({});
    prohibited["unexpected"] = true.into();
    assert!(parse_update(&prohibited.to_string(), "physical-source").is_err());
}

#[test]
fn host_updates_reject_even_empty_changes_after_the_source_fence() {
    let host = Record::Host(
        Host::new(
            "host",
            "Host",
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap(),
        )
        .unwrap(),
    );
    for actor in [Actor::User, Actor::Automatic] {
        let wire = request(&host, json!({}), actor).to_string();
        assert_eq!(
            parse_update(&wire, "physical-source")
                .unwrap_err()
                .to_string(),
            "agent updates require stream or session kind",
        );
        assert!(
            parse_update(&wire, "changed-source")
                .unwrap_err()
                .to_string()
                .starts_with("catalog source changed since capture"),
        );
    }
}

#[test]
fn duplicate_json_keys_and_malformed_captures_are_not_silently_accepted() {
    let wire = request(&stream(), json!({}), Actor::User).to_string();
    let duplicate = wire.replace(
        "\"changes\":{}",
        "\"changes\":{\"title\":\"A\",\"title\":\"B\"}",
    );
    assert_ne!(wire, duplicate);
    assert!(parse_update(&duplicate, "physical-source").is_err());
    assert!(parse_update(&format!("{wire} trailing"), "physical-source").is_err());
    let mut malformed: Value = serde_json::from_str(&wire).unwrap();
    malformed["inputs"] = json!([{ "kind": "stream", "record": record_to_data(&stream()) }]);
    assert!(parse_update(&malformed.to_string(), "physical-source").is_err());
    malformed["inputs"] = Value::Null;
    assert!(parse_update(&malformed.to_string(), "physical-source").is_err());
}
