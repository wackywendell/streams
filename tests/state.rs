use serde_json::{Value, json};
use streams_temper::sync::{
    Seconds,
    state::{GitConfig, PublicationAttempt, PublicationMarker, State, SyncMetadata},
};

fn with_state(root: &std::path::Path, f: impl FnOnce(&State<'_>)) {
    use streams_temper::{
        storage::{StorageError, Transaction},
        yaml_store::YamlStore,
    };
    YamlStore::new(root)
        .with_transaction::<_, StorageError>(|tx| {
            f(&State::from_journal(tx.journal()));
            Ok(())
        })
        .unwrap();
}

fn write(state: &State<'_>, name: &str, value: Value) {
    state
        .write(name, value.as_object().unwrap().clone())
        .unwrap();
}

#[test]
fn journal_rejects_nonregular_objects_without_blocking() {
    use std::{ffi::CString, fs, os::unix::ffi::OsStrExt};
    let dir = tempfile::tempdir().unwrap();
    with_state(dir.path(), |state| {
        state.write_metadata(&SyncMetadata::default()).unwrap();
        let path = dir.path().join(".catalog-sync/publication.json");
        fs::create_dir(&path).unwrap();
        assert!(state.read("publication.json").is_err());
        fs::remove_dir(&path).unwrap();
        let fifo = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(state.read("publication.json").is_err());
    });
}

#[test]
fn metadata_requires_nullable_fields_and_rejects_unknown_or_nonfinite_state() {
    let dir = tempfile::tempdir().unwrap();
    with_state(dir.path(), |state| {
        assert_eq!(state.metadata().unwrap(), SyncMetadata::default());
        state.write_metadata(&SyncMetadata::default()).unwrap();
        assert_eq!(state.metadata().unwrap(), SyncMetadata::default());
        let valid = state.read("sync.json").unwrap().unwrap();
        for field in [
            "confirmed",
            "last_sync",
            "last_push",
            "fetched_at",
            "retry_after",
            "last_confirmed_attempt",
            "refresh_retry_after",
            "refresh_error",
        ] {
            let mut broken = valid.clone();
            broken.remove(field);
            state.write("sync.json", broken).unwrap();
            assert!(state.metadata().is_err(), "missing {field}");
        }
        let mut unknown = valid;
        unknown.insert("typo".into(), Value::Null);
        state.write("sync.json", unknown).unwrap();
        assert!(state.metadata().is_err());
        for field in ["confirmed", "last_confirmed_attempt"] {
            for value in [json!("bad"), json!(42), json!([]), json!({})] {
                let mut broken = serde_json::to_value(SyncMetadata::default()).unwrap();
                broken[field] = value;
                write(state, "sync.json", broken);
                assert!(state.metadata().is_err(), "{field}");
            }
        }
        for field in [
            "last_sync",
            "last_push",
            "fetched_at",
            "retry_after",
            "refresh_retry_after",
        ] {
            for value in [json!("1"), json!(true), json!([]), json!({})] {
                let mut broken = serde_json::to_value(SyncMetadata::default()).unwrap();
                broken[field] = value;
                write(state, "sync.json", broken);
                assert!(state.metadata().is_err(), "{field}");
            }
        }
        let missing_baseline = SyncMetadata {
            last_confirmed_attempt: Some("a".repeat(40).parse().unwrap()),
            ..SyncMetadata::default()
        };
        assert!(state.write_metadata(&missing_baseline).is_err());
        write(
            state,
            "sync.json",
            serde_json::to_value(missing_baseline).unwrap(),
        );
        assert!(state.metadata().is_err());
    });
}

#[test]
fn git_config_deserialization_validates_constructor_invariants() {
    let valid = json!({"remote": null, "branch": "main", "host": "host"});
    let config: GitConfig = serde_json::from_value(valid.clone()).unwrap();
    assert_eq!(config.remote(), None);
    assert_eq!(config.branch(), "main");
    assert_eq!(config.host(), "host");
    assert_eq!(serde_json::to_value(config).unwrap(), valid);
    for (field, value) in [
        ("remote", json!("")),
        ("branch", json!("")),
        ("host", json!("")),
        ("branch", json!(null)),
        ("typo", json!(null)),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        assert!(
            serde_json::from_value::<GitConfig>(invalid).is_err(),
            "{field}"
        );
    }
    let mut missing = valid;
    missing.as_object_mut().unwrap().remove("remote");
    assert!(serde_json::from_value::<GitConfig>(missing).is_err());
}

#[test]
fn publication_confirmation_updates_preserve_validated_timestamps() {
    let attempt = PublicationAttempt::new(
        None,
        "a".repeat(40).parse().unwrap(),
        "b".repeat(40).parse().unwrap(),
        "main",
        "remote",
        vec!["create".into()],
        None,
    )
    .unwrap();
    let timestamp = Seconds::try_from(123.0).unwrap();
    let confirmed = attempt.clone().with_confirmed_at(Some(timestamp));
    assert_eq!(confirmed.confirmed_at(), Some(timestamp));
    assert_eq!(confirmed.clone().with_confirmed_at(None), attempt);
    let dir = tempfile::tempdir().unwrap();
    with_state(dir.path(), |state| {
        let marker = PublicationMarker::Attempting(confirmed);
        state.write_publication(&marker).unwrap();
        assert_eq!(state.publication().unwrap(), Some(marker));
    });
}

#[test]
fn config_and_publication_preserve_flat_wire_schema_and_phase_rules() {
    let dir = tempfile::tempdir().unwrap();
    with_state(dir.path(), |state| {
        let config = GitConfig::new(Some("remote".into()), "main", "host").unwrap();
        state.write_config(&config).unwrap();
        assert_eq!(state.config().unwrap(), Some(config));
        write(
            state,
            "config.json",
            json!({"branch":"main", "host":"host"}),
        );
        assert!(state.config().is_err()); // required remote may be null but not absent
        let attempt = PublicationAttempt::new(
            None,
            "a".repeat(40).parse().unwrap(),
            "b".repeat(40).parse().unwrap(),
            "main",
            "remote",
            vec!["create".into()],
            None,
        )
        .unwrap();
        let attempting = PublicationMarker::Attempting(attempt.clone());
        state.write_publication(&attempting).unwrap();
        assert_eq!(state.publication().unwrap(), Some(attempting));
        let wire = state.read("publication.json").unwrap().unwrap();
        assert_eq!(wire["phase"], "attempting");
        assert_eq!(wire["base"], Value::Null);
        assert_eq!(wire["confirmed_at"], Value::Null);
        assert!(!wire.contains_key("attempt"));
        let mut mapped_phase = wire.clone();
        mapped_phase.insert("phase".into(), json!({"attempting": null}));
        state.write("publication.json", mapped_phase).unwrap();
        assert!(state.publication().is_err());
        for field in ["base", "confirmed_at"] {
            let mut missing = wire.clone();
            missing.remove(field);
            state.write("publication.json", missing).unwrap();
            assert!(state.publication().is_err(), "missing {field}");
        }
        for field in ["base", "candidate", "local_tip", "confirmed_at"] {
            for value in [json!("invalid"), json!(true), json!([]), json!({})] {
                let mut broken = wire.clone();
                broken.insert(field.into(), value);
                state.write("publication.json", broken).unwrap();
                assert!(state.publication().is_err(), "{field}");
            }
        }
        for field in ["adoption_from", "adoption_to"] {
            for value in [Value::Null, json!("a".repeat(40))] {
                let mut broken = wire.clone();
                broken.insert(field.into(), value);
                state.write("publication.json", broken).unwrap();
                assert!(state.publication().is_err(), "{field}");
            }
        }

        let adopting = PublicationMarker::Adopting(attempt.adopting(
            "b".repeat(40).parse().unwrap(),
            "a".repeat(40).parse().unwrap(),
        ));
        state.write_publication(&adopting).unwrap();
        assert_eq!(state.publication().unwrap(), Some(adopting));
        let mut wire = state.read("publication.json").unwrap().unwrap();
        assert_eq!(wire["phase"], "adopting");
        assert_eq!(wire["adoption_from"], "b".repeat(40));
        assert_eq!(wire["adoption_to"], "a".repeat(40));
        wire.remove("adoption_to");
        state.write("publication.json", wire).unwrap();
        assert!(state.publication().is_err());
    });
}
