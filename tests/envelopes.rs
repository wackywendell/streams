use std::collections::BTreeMap;
use streams_temper::operations::Actor;
use streams_temper::storage::StorageError;

use chrono::DateTime;
use serde_json::{Value, json};
use streams_temper::{
    model::{Host, Record, Stream, TitleMode},
    operations::Operation,
    sync::{
        baseline::{BASELINE, Baseline, decode_baseline, encode_baseline},
        envelope::{
            PREFIX, check_fingerprints, decode_operation, encode_operation, operation_digest,
        },
    },
    yaml_store::YamlStore,
};

const MESSAGE: &str = concat!(
    "Streams operation v1\n\n",
    r#"{"actor":"user","after":{"archived":false,"documents":[],"goal":null,"goal_mode":"automatic","goal_short":null,"id":"work","issues":[],"owner":{"host_id":"host"},"prs":[],"repositories":{},"status":"active","title":"Wörk 🦀","title_mode":"automatic"},"before":null,"id":"create","kind":"stream","origin":{},"ownership_token":null,"version":1}"#,
    "\n"
);

fn creation() -> Operation {
    let mut stream = Stream::new("work", "Wörk 🦀", None, "host").unwrap();
    stream.title_mode = TitleMode::Automatic;
    Operation::new(
        "create",
        None,
        Record::Stream(stream),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}

fn message(data: &Value) -> String {
    format!("{PREFIX}{data}\n")
}

fn data() -> Value {
    serde_json::from_str(MESSAGE.strip_prefix(PREFIX).unwrap()).unwrap()
}

#[test]
fn exact_canonical_message_and_empty_captures() {
    let operation = creation();
    assert_eq!(encode_operation(&operation, None).unwrap(), MESSAGE);
    assert_eq!(
        operation_digest(&operation),
        "280459f88fa83ef9eb081525700244f5917f5f4b8c9980a9a65fdee7586b972b"
    );
    let (decoded, origin) = decode_operation(MESSAGE).unwrap();
    assert_eq!(decoded, operation);
    assert!(origin.is_empty());
    assert_eq!(encode_operation(&decoded, Some(&origin)).unwrap(), MESSAGE);
    assert!(data().get("inputs").is_none());
    let mut explicit_empty = data();
    explicit_empty["inputs"] = json!([]);
    assert_eq!(
        decode_operation(&message(&explicit_empty)).unwrap().0,
        operation
    );
}

#[test]
fn captures_preserve_unicode_host_microseconds_and_origin() {
    let host = Record::Host(
        Host::new(
            "host",
            "Höst",
            DateTime::parse_from_rfc3339("2026-01-02T03:04:05.123400+00:00").unwrap(),
        )
        .unwrap(),
    );
    let operation = Operation::new(
        "create",
        None,
        creation().after().clone(),
        Actor::User,
        None,
        vec![host],
    )
    .unwrap();
    let origin = BTreeMap::from([("commit".into(), "évidence".into())]);
    let encoded = encode_operation(&operation, Some(&origin)).unwrap();
    assert!(encoded.contains("2026-01-02T03:04:05.123400+00:00"));
    assert!(encoded.contains("Höst"));
    assert_eq!(decode_operation(&encoded).unwrap(), (operation, origin));
}

#[test]
fn captured_input_order_remains_part_of_original_intent() {
    let checked_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap();
    let inputs = ["z", "a"]
        .map(|id| Record::Host(Host::new(id, id, checked_at).unwrap()))
        .to_vec();
    let operation = Operation::new(
        "create",
        None,
        creation().after().clone(),
        Actor::User,
        None,
        inputs.clone(),
    )
    .unwrap();
    assert_eq!(operation.inputs(), inputs);
    let encoded = encode_operation(&operation, None).unwrap();
    assert_eq!(decode_operation(&encoded).unwrap().0, operation);
    let mut reversed = inputs;
    reversed.reverse();
    let reordered = Operation::new(
        "create",
        None,
        creation().after().clone(),
        Actor::User,
        None,
        reversed,
    )
    .unwrap();
    assert_ne!(operation, reordered);
    assert_ne!(operation_digest(&operation), operation_digest(&reordered));
}

#[test]
fn positional_envelopes_and_captured_records_are_rejected() {
    let wire = data();
    decode_operation(&message(&wire)).unwrap();
    let positional = json!([
        wire["version"],
        wire["id"],
        wire["kind"],
        wire["before"],
        wire["after"],
        wire["actor"],
        wire["ownership_token"],
        wire["origin"],
        []
    ]);
    assert!(decode_operation(&message(&positional)).is_err());

    let host = json!({"id": "host", "label": "Captured", "checked_at": "2026-01-01T00:00:00Z"});
    let mut captured = wire.clone();
    captured["inputs"] = json!([{"kind": "host", "record": host}]);
    decode_operation(&message(&captured)).unwrap();
    captured["inputs"] = json!([["host", host]]);
    assert!(decode_operation(&message(&captured)).is_err());
    captured["inputs"] = json!([{
        "kind": "host", "record": ["host", "Captured", "2026-01-01T00:00:00Z"]
    }]);
    assert!(decode_operation(&message(&captured)).is_err());

    let mut nested = wire;
    nested["after"]["owner"] = json!(["host"]);
    assert!(decode_operation(&message(&nested)).is_err());
    nested = data();
    nested["before"] = nested["after"].clone();
    nested["after"]["title"] = json!("Changed");
    decode_operation(&message(&nested)).unwrap();
    nested["before"]["owner"] = json!(["host"]);
    assert!(decode_operation(&message(&nested)).is_err());
}

#[test]
fn positional_baseline_envelopes_are_rejected() {
    let files = BTreeMap::new();
    let object = json!({"version": 1, "host": "host", "creations": {}, "fingerprints": {}});
    decode_baseline(&format!("{BASELINE}{object}\n"), &files).unwrap();
    let positional = json!([1, "host", {}, {}]);
    assert!(decode_baseline(&format!("{BASELINE}{positional}\n"), &files).is_err());
}

#[test]
fn duplicate_json_keys_are_rejected_at_every_depth() {
    for malformed in [
        MESSAGE.replace("\"actor\":\"user\"", "\"actor\":\"user\",\"actor\":\"user\""),
        MESSAGE.replace("\"host_id\":\"host\"", "\"host_id\":\"host\",\"host_id\":\"host\""),
        MESSAGE.replace("\"origin\":{}", "\"origin\":{\"x\":\"a\",\"x\":\"b\"}"),
        MESSAGE.replace("\"origin\":{}", "\"inputs\":[{\"kind\":\"host\",\"record\":{\"id\":\"host\",\"id\":\"host\"}}],\"origin\":{}"),
    ] {
        let error = decode_operation(&malformed).unwrap_err().to_string();
        assert!(error.contains("duplicate JSON field"), "{error}");
    }
}

#[test]
fn unknown_keys_required_nullable_fields_and_versions_are_strict() {
    let mut unknown = data();
    unknown["extra"] = json!(true);
    assert!(decode_operation(&message(&unknown)).is_err());
    unknown.as_object_mut().unwrap().remove("extra");
    unknown["inputs"] = json!([{"kind":"stream", "record":data()["after"], "extra":true}]);
    assert!(decode_operation(&message(&unknown)).is_err());
    for field in ["before", "ownership_token", "origin", "version", "after"] {
        let mut missing = data();
        missing.as_object_mut().unwrap().remove(field);
        assert!(decode_operation(&message(&missing)).is_err(), "{field}");
    }
    for version in [
        json!(1.0),
        json!(true),
        json!("1"),
        json!(null),
        json!(0),
        json!(2),
        json!(-1),
    ] {
        let mut malformed = data();
        malformed["version"] = version;
        assert!(
            decode_operation(&message(&malformed)).is_err(),
            "{malformed}"
        );
    }
    for (field, value) in [
        ("origin", json!({"bad":1})),
        ("inputs", json!(null)),
        ("actor", json!("bad")),
    ] {
        let mut malformed = data();
        malformed[field] = value;
        assert!(decode_operation(&message(&malformed)).is_err());
    }
}

#[test]
fn fingerprints_validate_text_ids_and_lowercase_sha256() {
    let valid = BTreeMap::from([("create".into(), operation_digest(&creation()))]);
    check_fingerprints(&valid).unwrap();
    for malformed in [
        json!([]),
        json!({"create":1}),
        json!({"create":"abc"}),
        json!({"create":"A".repeat(64)}),
        json!({" ":"a".repeat(64)}),
        json!({"a\0b":"a".repeat(64)}),
    ] {
        let parsed = serde_json::from_value::<BTreeMap<String, String>>(malformed.clone());
        assert!(
            parsed.is_err() || check_fingerprints(&parsed.unwrap()).is_err(),
            "{malformed}"
        );
    }
}

#[test]
fn baseline_canonical_roundtrip_and_creation_evidence() {
    let operation = creation();
    let baseline = Baseline::new(
        "host",
        BTreeMap::from([("work".into(), MESSAGE.into())]),
        BTreeMap::from([("create".into(), operation_digest(&operation))]),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    store
        .with_transaction::<_, StorageError>(|tx| {
            let snapshot = tx.read()?;
            tx.save(operation.after(), &snapshot)?;
            Ok(())
        })
        .unwrap();
    let path = store.record_path(operation.after());
    let files = BTreeMap::from([(
        path.to_string_lossy().into_owned(),
        std::fs::read(dir.path().join(path)).unwrap(),
    )]);
    let encoded = encode_baseline(&baseline).unwrap();
    let expected = json!({"creations":{"work":MESSAGE}, "fingerprints":baseline.fingerprints, "host":"host", "version":1});
    assert_eq!(encoded, format!("{BASELINE}{expected}\n"));
    let (decoded, catalog, authorities) = decode_baseline(&encoded, &files).unwrap();
    assert_eq!(decoded, baseline);
    assert_eq!(encode_baseline(&decoded).unwrap(), encoded);
    assert_eq!(catalog.streams().len(), 1);
    assert_eq!(authorities["work"].token, "create");
    for (field, value) in [
        ("fingerprints", json!({"create":"bad"})),
        ("fingerprints", json!({"create":"a".repeat(64)})),
        ("creations", json!({})),
        (
            "creations",
            json!({"work":MESSAGE.replace("\"version\":1", "\"version\": 1")}),
        ),
        (
            "creations",
            json!({"work":MESSAGE.replace("\"host_id\":\"host\"", "\"host_id\":\"other\"" )}),
        ),
        ("host", json!("other")),
        ("version", json!(1.0)),
        ("version", json!(true)),
        ("version", json!(2)),
        ("extra", json!(true)),
    ] {
        let mut malformed = expected.clone();
        malformed[field] = value;
        assert!(
            decode_baseline(&format!("{BASELINE}{malformed}\n"), &files).is_err(),
            "{malformed}"
        );
    }
    assert!(
        Baseline::new(
            "host",
            BTreeMap::new(),
            BTreeMap::from([("create".into(), "bad".into())])
        )
        .is_err()
    );
}
