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
    sync::{
        outcomes::{LocalStatus, SharingStatus},
        policy::{Policy, SyncEvent, Timing},
    },
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
    roundtrip(&[
        (SyncEvent::Session, "session"),
        (SyncEvent::Host, "host"),
        (SyncEvent::Stream, "stream"),
        (SyncEvent::Start, "start"),
        (SyncEvent::Attach, "attach"),
        (SyncEvent::Shutdown, "shutdown"),
        (SyncEvent::Explicit, "explicit"),
        (SyncEvent::Force, "force"),
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

#[test]
fn typed_sync_events_keep_cooldown_and_stream_priority() {
    let policy = Policy::default();
    let seconds = |value| streams_temper::sync::Seconds::try_from(value).unwrap();
    let cooling = Timing::new(
        Some(seconds(1000.0)),
        Some(seconds(1000.0)),
        seconds(2000.0),
    );
    for event in [SyncEvent::Force, SyncEvent::Explicit, SyncEvent::Shutdown] {
        assert!(policy.due(event, cooling, seconds(1000.0), false));
    }
    for event in [
        SyncEvent::Session,
        SyncEvent::Host,
        SyncEvent::Stream,
        SyncEvent::Start,
        SyncEvent::Attach,
    ] {
        assert!(!policy.due(event, cooling, seconds(1000.0), true));
    }
    let timing = Timing::new(Some(seconds(0.0)), Some(seconds(0.0)), seconds(0.0));
    assert!(!policy.due(SyncEvent::Session, timing, seconds(301.0), false));
    assert!(policy.due(SyncEvent::Session, timing, seconds(301.0), true));
}

#[test]
fn policy_deserialization_uses_constructor_validation_without_changing_schema() {
    let wire = serde_json::to_value(Policy::default()).unwrap();
    assert_eq!(
        serde_json::from_value::<Policy>(wire.clone()).unwrap(),
        Policy::default()
    );
    for field in [
        "stream_seconds",
        "session_seconds",
        "cooldown_seconds",
        "timeout_seconds",
    ] {
        let mut invalid = wire.clone();
        invalid[field] = json!(-1.0);
        assert!(
            serde_json::from_value::<Policy>(invalid).is_err(),
            "{field}"
        );
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<Policy>(missing).is_err(),
            "{field}"
        );
    }
    for (field, value) in [
        ("timeout_seconds", json!(0)),
        ("attempts", json!(0)),
        ("attempts", json!(4)),
    ] {
        let mut invalid = wire.clone();
        invalid[field] = value;
        assert!(
            serde_json::from_value::<Policy>(invalid).is_err(),
            "{field}"
        );
    }
    for attempts in 1..=3 {
        let valid = Policy::new(0.0, 0.0, 0.0, f64::MIN_POSITIVE, attempts).unwrap();
        assert_eq!(
            serde_json::from_value::<Policy>(serde_json::to_value(valid).unwrap()).unwrap(),
            valid
        );
    }
    let mut unknown = wire;
    unknown["extra"] = json!(true);
    assert_eq!(
        serde_json::from_value::<Policy>(unknown).unwrap(),
        Policy::default()
    );
    for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        assert!(Policy::new(value, 1.0, 1.0, 1.0, 1).is_err());
        assert!(Policy::new(1.0, value, 1.0, 1.0, 1).is_err());
        assert!(Policy::new(1.0, 1.0, value, 1.0, 1).is_err());
        assert!(Policy::new(1.0, 1.0, 1.0, value, 1).is_err());
    }
}

#[test]
fn timing_and_read_policy_use_validated_seconds() {
    use streams_temper::sync::Seconds;
    let seconds = |value| Seconds::try_from(value).unwrap();
    let policy = Policy::default();
    assert!(!policy.read_due(Some(seconds(1000.0)), seconds(2000.0), Seconds::ZERO, false));
    assert!(policy.read_due(Some(seconds(1000.0)), seconds(4600.0), Seconds::ZERO, false));
    assert!(!policy.read_due(None, seconds(1000.0), seconds(2000.0), false));
    assert!(policy.read_due(None, seconds(1000.0), seconds(2000.0), true));
    let timing = Timing::new(Some(seconds(-1.0)), None, Seconds::ZERO);
    let wire = serde_json::to_value(timing).unwrap();
    assert_eq!(
        serde_json::from_value::<Timing>(wire.clone()).unwrap(),
        timing
    );
    for field in ["last_sync", "last_push", "retry_after"] {
        for value in [json!("1"), json!(true), json!([]), json!({})] {
            let mut invalid = wire.clone();
            invalid[field] = value;
            assert!(
                serde_json::from_value::<Timing>(invalid).is_err(),
                "{field}"
            );
        }
    }
}
