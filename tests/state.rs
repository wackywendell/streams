use serde_json::{Value, json};
use streams_temper::sync::state::{
    GitConfig, PublicationAttempt, PublicationMarker, State, SyncMetadata,
};

fn write(state: &State, name: &str, value: Value) {
    state
        .write(name, value.as_object().unwrap().clone())
        .unwrap();
}

#[test]
fn metadata_requires_nullable_fields_and_rejects_unknown_or_nonfinite_state() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::new(dir.path());
    assert_eq!(state.metadata().unwrap(), SyncMetadata::default());
    state.write_metadata(&SyncMetadata::default()).unwrap();
    let valid = state.read("sync.json").unwrap().unwrap();
    for field in [
        "confirmed",
        "last_sync",
        "last_push",
        "fetched_at",
        "retry_after",
    ] {
        let mut broken = valid.clone();
        broken.remove(field);
        state.write("sync.json", broken).unwrap();
        assert!(state.metadata().is_err(), "missing {field}");
    }
    let mut old = valid.clone();
    for field in [
        "last_confirmed_attempt",
        "refresh_retry_after",
        "refresh_error",
    ] {
        old.remove(field);
    }
    state.write("sync.json", old).unwrap();
    assert_eq!(state.metadata().unwrap(), SyncMetadata::default());
    let mut unknown = valid;
    unknown.insert("typo".into(), Value::Null);
    state.write("sync.json", unknown).unwrap();
    assert!(state.metadata().is_err());
    assert!(
        state
            .write_metadata(&SyncMetadata {
                last_push: Some(f64::NAN),
                ..SyncMetadata::default()
            })
            .is_err()
    );
}

#[test]
fn config_and_publication_preserve_flat_wire_schema_and_phase_rules() {
    let dir = tempfile::tempdir().unwrap();
    let state = State::new(dir.path());
    let config = GitConfig::new(Some("remote".into()), "main", "host").unwrap();
    state.write_config(&config).unwrap();
    assert_eq!(state.config().unwrap(), Some(config));
    write(
        &state,
        "config.json",
        json!({"branch":"main", "host":"host"}),
    );
    assert!(state.config().is_err()); // required remote may be null but not absent
    let attempt = PublicationAttempt::new(
        None,
        "a".repeat(40),
        "b".repeat(40),
        "main",
        "remote",
        vec!["create".into()],
        None,
    )
    .unwrap();
    let attempting = PublicationMarker::Attempting(attempt.clone());
    state.write_publication(&attempting).unwrap();
    assert_eq!(state.publication().unwrap(), Some(attempting));
    let mut wire = state.read("publication.json").unwrap().unwrap();
    assert_eq!(wire["phase"], "attempting");
    assert!(!wire.contains_key("attempt"));
    wire.insert("adoption_from".into(), Value::Null);
    state.write("publication.json", wire).unwrap();
    assert!(state.publication().is_err());

    let adopting =
        PublicationMarker::Adopting(attempt.adopting("b".repeat(40), "a".repeat(40)).unwrap());
    state.write_publication(&adopting).unwrap();
    assert_eq!(state.publication().unwrap(), Some(adopting));
    let mut wire = state.read("publication.json").unwrap().unwrap();
    wire.remove("adoption_to");
    state.write("publication.json", wire).unwrap();
    assert!(state.publication().is_err());
}
