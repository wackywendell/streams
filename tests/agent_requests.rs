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
