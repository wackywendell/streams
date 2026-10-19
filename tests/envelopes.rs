use std::collections::BTreeMap;

use chrono::DateTime;
use serde_json::{Value, json};
use streams_temper::{
    model::{Host, Record, Stream},
    operations::Operation,
    sync::{
        baseline::{BASELINE, Baseline, decode_baseline, encode_baseline},
        envelope::{
            PREFIX, check_fingerprints, decode_fingerprints, decode_operation, encode_operation,
            operation_digest,
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
    Operation::new(
        "create",
        None,
        Record::Stream(Stream::new("work", "Wörk 🦀", None, "host").unwrap()),
        "user",
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
        "user",
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
    assert_eq!(decode_fingerprints(&json!(valid)).unwrap(), valid);
    for malformed in [
        json!([]),
        json!({"create":1}),
        json!({"create":"abc"}),
        json!({"create":"A".repeat(64)}),
        json!({" ":"a".repeat(64)}),
        json!({"a\0b":"a".repeat(64)}),
    ] {
        assert!(decode_fingerprints(&malformed).is_err(), "{malformed}");
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
        .with_transaction(|tx| {
            let snapshot = tx.read()?;
            tx.save(operation.after(), &snapshot)?;
            Ok(())
        })
        .unwrap();
    let path = store.record_path(&operation.target());
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
    assert_eq!(catalog.streams.len(), 1);
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
