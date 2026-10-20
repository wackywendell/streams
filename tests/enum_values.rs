use std::{
    fmt::{Debug, Display},
    str::FromStr,
};

use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use streams_temper::{
    codec::record_from_data,
    model::{NamingMode, RecordKind, Status},
    operations::{Actor, DomainOutcome},
    sync::{
        Freshness,
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
        (RecordKind::Host, "host"),
        (RecordKind::Session, "session"),
        (RecordKind::Stream, "stream"),
    ]);
    roundtrip(&[(Actor::User, "user"), (Actor::Automatic, "automatic")]);
    roundtrip(&[
        (DomainOutcome::Changes, "changes"),
        (DomainOutcome::NoChange, "no-change"),
        (DomainOutcome::Superseded, "superseded"),
        (DomainOutcome::Conflict, "conflict"),
    ]);
    assert_eq!(Status::default(), Status::Active);
    assert_eq!(NamingMode::default(), NamingMode::Automatic);
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
        (Freshness::LocalOnly, "local-only"),
        (Freshness::Cached, "cached"),
        (Freshness::Refreshed, "refreshed"),
        (Freshness::Failed, "failed"),
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
    let cooling = Timing::new(Some(1000.0), Some(1000.0), 2000.0).unwrap();
    for event in [SyncEvent::Force, SyncEvent::Explicit, SyncEvent::Shutdown] {
        assert!(policy.due(event, cooling, 1000.0, false).unwrap());
    }
    for event in [
        SyncEvent::Session,
        SyncEvent::Host,
        SyncEvent::Stream,
        SyncEvent::Start,
        SyncEvent::Attach,
    ] {
        assert!(!policy.due(event, cooling, 1000.0, true).unwrap());
    }
    let timing = Timing::new(Some(0.0), Some(0.0), 0.0).unwrap();
    assert!(
        !policy
            .due(SyncEvent::Session, timing, 301.0, false)
            .unwrap()
    );
    assert!(policy.due(SyncEvent::Session, timing, 301.0, true).unwrap());
}
