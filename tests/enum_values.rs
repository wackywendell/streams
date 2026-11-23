use std::{
    fmt::{Debug, Display},
    str::FromStr,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use streams::{
    codec::record_from_data,
    config::CardMode,
    model::{NamingMode, RecordKind, ReferenceKind, ReferenceRole, ReferenceStatus, Status},
    runtime::conversations::{Harness, NativeIdentityKind},
    sync::outcomes::{LocalStatus, SharingStatus},
};

fn roundtrip<T>(cases: &[(T, &str)])
where
    T: Copy + Debug + Eq + Display + AsRef<str> + FromStr + Serialize + DeserializeOwned,
    T::Err: Debug,
{
    for &(value, spelling) in cases {
        assert_eq!(value.to_string(), spelling);
        assert_eq!(value.as_ref(), spelling);
        assert_eq!(spelling.parse::<T>().unwrap(), value);
        assert_eq!(serde_json::to_value(value).unwrap(), json!(spelling));
        assert_eq!(serde_json::from_value::<T>(json!(spelling)).unwrap(), value);
        assert!(serde_json::from_value::<T>(json!({(spelling): null})).is_err());
    }
    for invalid in ["", "unknown", "ACTIVE", "active ", " active"] {
        assert!(invalid.parse::<T>().is_err(), "{invalid:?}");
        assert!(serde_json::from_value::<T>(json!(invalid)).is_err());
    }
    for invalid in [json!(null), json!(1), json!(true), json!([])] {
        assert!(serde_json::from_value::<T>(invalid).is_err());
    }
}

#[test]
fn domain_enums_have_one_strict_wire_spelling() {
    roundtrip(&[
        (Status::Active, "active"),
        (Status::Waiting, "waiting"),
        (Status::Blocked, "blocked"),
        (Status::Parked, "parked"),
        (Status::Done, "done"),
        (Status::Closed, "closed"),
    ]);
    roundtrip(&[
        (NamingMode::Automatic, "automatic"),
        (NamingMode::Manual, "manual"),
    ]);
    roundtrip(&[
        (CardMode::Automatic, "automatic"),
        (CardMode::Manual, "manual"),
    ]);
    roundtrip(&[
        (RecordKind::Host, "host"),
        (RecordKind::Session, "session"),
        (RecordKind::Stream, "stream"),
    ]);
    assert_eq!(Status::default(), Status::Active);
    assert_eq!(NamingMode::default(), NamingMode::Automatic);
    assert_eq!(CardMode::default(), CardMode::Automatic);
    assert!("inherit".parse::<NamingMode>().is_err());
    assert!("inherit".parse::<CardMode>().is_err());
}

#[test]
fn reference_kinds_roles_and_statuses_have_one_strict_wire_spelling() {
    roundtrip(&[
        (ReferenceKind::Pr, "pr"),
        (ReferenceKind::Issue, "issue"),
        (ReferenceKind::Link, "link"),
        (ReferenceKind::Document, "document"),
    ]);
    roundtrip(&[
        (ReferenceRole::Deliverable, "deliverable"),
        (ReferenceRole::Dependency, "dependency"),
        (ReferenceRole::Brief, "brief"),
        (ReferenceRole::Background, "background"),
    ]);
    roundtrip(&[
        (ReferenceStatus::Active, "active"),
        (ReferenceStatus::Fulfilled, "fulfilled"),
        (ReferenceStatus::Dropped, "dropped"),
    ]);
}

#[test]
fn native_enums_require_string_wire_values() {
    roundtrip(&[
        (Harness::Pi, "pi"),
        (Harness::Codex, "codex"),
        (Harness::Claude, "claude"),
    ]);
    roundtrip(&[
        (NativeIdentityKind::Id, "id"),
        (NativeIdentityKind::Path, "path"),
    ]);
}

#[test]
fn sync_enums_preserve_hyphenated_wire_values() {
    roundtrip(&[
        (LocalStatus::Unchanged, "unchanged"),
        (LocalStatus::Committed, "committed"),
        (LocalStatus::RepairRequired, "repair-required"),
    ]);
    roundtrip(&[
        (SharingStatus::LocalOnly, "local-only"),
        (SharingStatus::Confirmed, "confirmed"),
        (SharingStatus::Blocked, "blocked"),
        (SharingStatus::Failed, "failed"),
        (SharingStatus::Uncertain, "uncertain"),
    ]);
}

#[test]
fn invalid_enum_fields_are_rejected_at_record_decode() {
    for invalid in [
        json!("unknown"),
        json!("ACTIVE"),
        json!(null),
        json!(true),
        json!(10),
    ] {
        let mut wire = json!({
            "host_id": "host",
            "harness": "pi",
            "id": "sess",
            "stream_id": "work",
            "title": {"text": "Work", "manual": false},
            "status": "active",
            "recap": null,
            "state": null,
            "remaining": null,
            "directory": null,
            "references": {},
            "artifacts": {},
            "revision": 1,
            "updated_at_ms": 0
        });
        wire["status"] = invalid;
        assert!(record_from_data(RecordKind::Session, &wire).is_err());
    }
}
