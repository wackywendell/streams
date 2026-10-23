use super::super::decode;
use super::*;
use serde_json::{Value, json};

fn current() -> Value {
    json!({"version":3,"session_id":"session","location":{"kind":"checkout","id":"saved-id","within":".","repository_url":null}})
}

fn validate(value: &Value) -> Result<()> {
    let selection = decode::<Current>(&serde_json::to_vec(value).unwrap())?;
    DirectoryBindings::validate(&selection)
}

#[test]
fn supported_identity_versions_and_locations_validate_without_files() {
    let mut value = current();
    assert!(validate(&value).is_ok());
    value["location"] = json!({"kind":"directory","cwd":"/not-created/binding-target"});
    assert!(validate(&value).is_ok());
    value["version"] = json!(2);
    value["stream_id"] = json!("stream");
    assert!(validate(&value).is_ok());
    value["session_id"] = Value::Null;
    assert!(validate(&value).is_ok());
    value.as_object_mut().unwrap().remove("stream_id");
    assert!(validate(&value).is_err());
}

#[test]
fn malformed_binding_wire_shapes_and_identity_versions_are_rejected() {
    let good = current();
    let mut cases = vec![
        Value::Null,
        json!([]),
        json!([2, "stream", "session", good["location"]]),
        json!({"version":1,"stream_id":"stream","session_id":"session","cwd":"/saved"}),
    ];
    for (key, value) in [
        ("version", json!(true)),
        ("version", json!(1)),
        ("version", json!(3.0)),
        ("version", json!(4)),
        ("stream_id", json!("wrong")),
        ("session_id", Value::Null),
        ("session_id", json!(" ")),
        ("extra", json!(1)),
    ] {
        let mut state = good.clone();
        state[key] = value;
        cases.push(state);
    }
    for key in ["version", "session_id", "location"] {
        let mut state = good.clone();
        state.as_object_mut().unwrap().remove(key);
        cases.push(state);
    }
    for state in cases {
        assert!(validate(&state).is_err(), "accepted {state}");
    }
}

#[test]
fn malformed_locations_and_missing_nullable_repository_are_rejected() {
    let good = current();
    let mut cases = Vec::new();
    for location in [
        Value::Null,
        json!(["directory", "/saved"]),
        json!({"kind":"directory","cwd":"relative"}),
        json!({"kind":"directory","cwd":"~/repo"}),
        json!({"kind":"directory","cwd":""}),
        json!({"kind":"directory","cwd":"/bad\u{7f}"}),
        json!({"kind":"directory","cwd":"/saved","extra":true}),
        json!({"kind":"checkout","id":"saved-id","within":"."}),
    ] {
        let mut state = good.clone();
        state["location"] = location;
        cases.push(state);
    }
    for (key, value) in [
        ("kind", json!("guess")),
        ("within", json!("../escape")),
        ("within", json!("/absolute")),
        ("id", json!("bad\u{7f}")),
        ("id", json!("")),
        ("repository_url", json!("bad\u{7f}")),
        ("extra", json!(1)),
    ] {
        let mut state = good.clone();
        state["location"][key] = value;
        cases.push(state);
    }
    let mut missing = good;
    missing["location"]
        .as_object_mut()
        .unwrap()
        .remove("repository_url");
    cases.push(missing);
    for state in cases {
        assert!(validate(&state).is_err(), "accepted {state}");
    }
}
