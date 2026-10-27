use std::{
    fmt::{Debug, Display},
    str::FromStr,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use streams_temper::{
    codec::record_from_data,
    model::{NamingMode, RecordKind, Status, TitleMode},
    operations::{Actor, DomainOutcome},
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
    ]);
    roundtrip(&[
        (NamingMode::Automatic, "automatic"),
        (NamingMode::Manual, "manual"),
    ]);
    roundtrip(&[
        (TitleMode::Automatic, "automatic"),
        (TitleMode::Manual, "manual"),
        (TitleMode::Inherit, "inherit"),
    ]);
    roundtrip(&[
        (RecordKind::Host, "host"),
        (RecordKind::Session, "session"),
        (RecordKind::Stream, "stream"),
    ]);
    roundtrip(&[
        (Actor::User, "user"),
        (Actor::Automatic, "automatic"),
        (Actor::Native, "native"),
    ]);
    roundtrip(&[
        (DomainOutcome::Changes, "changes"),
        (DomainOutcome::NoChange, "no-change"),
        (DomainOutcome::Superseded, "superseded"),
        (DomainOutcome::Conflict, "conflict"),
    ]);
    assert_eq!(Status::default(), Status::Active);
    assert_eq!(NamingMode::default(), NamingMode::Automatic);
    assert_eq!(TitleMode::default(), TitleMode::Inherit);
    for default in [NamingMode::Automatic, NamingMode::Manual] {
        assert_eq!(TitleMode::Inherit.resolve(default), default);
        assert_eq!(TitleMode::Automatic.resolve(default), NamingMode::Automatic);
        assert_eq!(TitleMode::Manual.resolve(default), NamingMode::Manual);
    }
    assert!("inherit".parse::<NamingMode>().is_err());
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
        (LocalStatus::Persisted, "persisted"),
        (LocalStatus::Committed, "committed"),
        (LocalStatus::RepairRequired, "repair-required"),
    ]);
    roundtrip(&[
        (SharingStatus::LocalOnly, "local-only"),
        (SharingStatus::Pending, "pending"),
        (SharingStatus::Confirmed, "confirmed"),
        (SharingStatus::Blocked, "blocked"),
        (SharingStatus::Failed, "failed"),
        (SharingStatus::Uncertain, "uncertain"),
    ]);
}

#[test]
fn invalid_enum_fields_are_rejected_at_record_decode() {
    for field in ["status", "title_mode", "goal_mode"] {
        for invalid in [
            json!("unknown"),
            json!("ACTIVE"),
            json!(null),
            json!(true),
            json!(10),
        ] {
            let mut wire =
                json!({"id": "work", "title": "Work", "goal": null, "owner": {"host_id": "host"}});
            wire[field] = invalid;
            assert!(
                record_from_data(RecordKind::Stream, &wire).is_err(),
                "{field}"
            );
        }
    }
}
