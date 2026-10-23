use super::super::decode;
use super::*;
use serde_json::json;

fn record_path(identity: &str) -> PathBuf {
    format!("{}.json", key(&[identity])).into()
}

fn association_wire() -> Value {
    json!({"version":1,"stream_id":"stream","workspace_id":"workspace","session_id":null,"command":null,"ready":true})
}

fn claim_wire() -> Value {
    json!({"version":1,"stream_id":"stream","workspace_id":"workspace"})
}

fn attempt_wire() -> Value {
    json!({"version":1,"stream_id":"stream","session_id":null,"command":null,"cwd":"/chosen","workspace_id":null,"effects":[],"launches":[]})
}

fn association(value: &Value) -> Result<Association> {
    decode::<AssociationWire>(&serde_json::to_vec(value).unwrap())?
        .validated(&record_path("workspace"))
}

fn claim(value: &Value) -> Result<EnrollmentClaim> {
    decode::<ClaimWire>(&serde_json::to_vec(value).unwrap())?.validated(&record_path("workspace"))
}

fn attempt(value: &Value) -> Result<PendingAttempt> {
    decode::<AttemptWire>(&serde_json::to_vec(value).unwrap())?.validated(&record_path("stream"))
}

#[test]
fn association_schema_identity_and_launch_pairs_are_validated_without_files() {
    let mut value = association_wire();
    assert_eq!(
        serde_json::to_value(association(&value).unwrap().wire()).unwrap(),
        value
    );
    value["session_id"] = json!("session");
    value["command"] = json!("pi --session '/saved'");
    value["ready"] = json!(false);
    assert_eq!(
        serde_json::to_value(association(&value).unwrap().wire()).unwrap(),
        value
    );

    for field in [
        "version",
        "stream_id",
        "workspace_id",
        "session_id",
        "command",
        "ready",
    ] {
        let mut value = association_wire();
        value.as_object_mut().unwrap().remove(field);
        assert!(association(&value).is_err(), "accepted missing {field}");
    }
    for (field, bad) in [
        ("version", json!(2)),
        ("version", json!(true)),
        ("version", json!(1.0)),
        ("session_id", json!("session")),
        ("command", json!("command")),
        ("ready", json!(1)),
        ("stream_id", json!(" ")),
        ("workspace_id", json!("different")),
        ("workspace_id", json!("work\u{0}space")),
        ("unknown", json!(null)),
    ] {
        let mut value = association_wire();
        value[field] = bad;
        assert!(association(&value).is_err(), "accepted {value}");
    }
    assert!(association(&json!([1, "stream", "workspace", null, null, true])).is_err());
}

#[test]
fn claim_schema_and_filename_identity_are_validated_without_files() {
    let value = claim(&claim_wire()).unwrap();
    assert_eq!(value.stream_id(), "stream");
    assert_eq!(value.workspace_id(), "workspace");
    for field in ["version", "stream_id", "workspace_id"] {
        let mut value = claim_wire();
        value.as_object_mut().unwrap().remove(field);
        assert!(claim(&value).is_err(), "accepted missing {field}");
    }
    for (field, bad) in [
        ("version", json!(0)),
        ("version", json!(1.0)),
        ("stream_id", json!(null)),
        ("stream_id", json!(" ")),
        ("workspace_id", json!("wrong")),
        ("extra", json!(true)),
    ] {
        let mut value = claim_wire();
        value[field] = bad;
        assert!(claim(&value).is_err(), "accepted {value}");
    }
    assert!(claim(&json!([1, "stream", "workspace"])).is_err());
}

#[test]
fn pending_attempt_schema_preserves_opaque_evidence_without_files() {
    let mut value = attempt_wire();
    value["effects"] = json!([null, 1, ["anything"], {"unknown":"evidence"}]);
    value["launches"] = json!([false]);
    assert_eq!(
        serde_json::to_value(attempt(&value).unwrap().wire()).unwrap(),
        value
    );
    for field in [
        "version",
        "stream_id",
        "session_id",
        "command",
        "cwd",
        "workspace_id",
        "effects",
        "launches",
    ] {
        let mut value = attempt_wire();
        value.as_object_mut().unwrap().remove(field);
        assert!(attempt(&value).is_err(), "accepted missing {field}");
    }
    for (field, bad) in [
        ("version", json!(2)),
        ("version", json!(1.0)),
        ("session_id", json!("session")),
        ("command", json!("command")),
        ("workspace_id", json!("")),
        ("cwd", json!(null)),
        ("cwd", json!(" ")),
        ("effects", json!({})),
        ("launches", json!(null)),
        ("stream_id", json!("wrong")),
        ("extra", json!([])),
    ] {
        let mut value = attempt_wire();
        value[field] = bad;
        assert!(attempt(&value).is_err(), "accepted {value}");
    }
    assert!(attempt(&json!([1, "stream", null, null, "/chosen", null, [], []])).is_err());
}
