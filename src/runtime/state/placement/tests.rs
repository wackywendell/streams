use super::*;
use crate::runtime::native_state::NativeState;
use serde_json::json;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::sync::atomic::AtomicBool;

const SCOPE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fixture {
    temp: tempfile::TempDir,
    state: RuntimeState,
    native: NativeState,
    cancelled: AtomicBool,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let state = RuntimeState::new(temp.path().join("runtime"), "host").unwrap();
        let native = NativeState::new(temp.path().join("native")).unwrap();
        Self {
            temp,
            state,
            native,
            cancelled: AtomicBool::new(false),
        }
    }

    fn put(&self, value: &Value) {
        let path = self.state.placement_path("stream").unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }
}

fn stream(host: &str) -> Stream {
    Stream::new("stream", "Work", None, host).unwrap()
}
fn placement() -> NativePlacement {
    NativePlacement::new(
        "stream",
        SCOPE,
        PlacementBinding::herdr("workspace", "lifetime").unwrap(),
    )
    .unwrap()
}
fn wire() -> Value {
    serde_json::to_value(placement().wire()).unwrap()
}
fn pending() -> Value {
    json!({"session_id":null,"command":null,"cwd":"/chosen","effects":[{"kind":"workspace_create"}],"launches":[]})
}

#[test]
fn missing_placement_is_inert_and_unrelated_records_are_not_discovered() {
    let f = Fixture::new();
    assert!(f.state.placement(SCOPE, "stream").unwrap().is_none());
    assert!(!f.state.root().exists());
    assert!(!f.native.directory().exists());
    for collection in [
        "associations",
        "enrollments",
        "attempts",
        "arrangements",
        "placements",
    ] {
        let path = f.state.root().join(collection).join("unrelated.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "corrupt unrelated record").unwrap();
    }
    assert!(f.state.placement(SCOPE, "stream").unwrap().is_none());
    assert!(!f.state.placement_path("stream").unwrap().exists());
    f.put(&wire());
    assert_eq!(
        f.state.placement(SCOPE, "stream").unwrap(),
        Some(placement())
    );
    fs::rename(f.state.root(), f.temp.path().join("old-runtime")).unwrap();
    assert!(f.state.placement(SCOPE, "stream").unwrap().is_none());
    assert!(!f.state.root().exists());
}

#[test]
fn binding_save_is_checked_global_and_unchanged_is_read_only() {
    let f = Fixture::new();
    let guard = f.native.lock(1.0, &f.cancelled).unwrap();
    let desired = placement();
    f.state
        .save_placement_binding(&guard, &stream("host"), None, &desired)
        .unwrap();
    assert_eq!(
        f.state.placement(SCOPE, "stream").unwrap(),
        Some(desired.clone())
    );
    assert!(f.state.placement(OTHER, "stream").is_err());
    let path = f.state.placement_path("stream").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let before = fs::read(&path).unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    f.state
        .save_placement_binding(&guard, &stream("host"), Some(&desired), &desired)
        .unwrap();
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert!(
        f.state
            .save_placement_binding(&guard, &stream("host"), None, &desired)
            .is_err()
    );
    for replacement in [
        NativePlacement::new(
            "stream",
            OTHER,
            PlacementBinding::herdr("workspace", "lifetime").unwrap(),
        )
        .unwrap(),
        NativePlacement::new(
            "stream",
            SCOPE,
            PlacementBinding::herdr("another", "lifetime").unwrap(),
        )
        .unwrap(),
        NativePlacement::new(
            "stream",
            SCOPE,
            PlacementBinding::herdr("workspace", "another").unwrap(),
        )
        .unwrap(),
        NativePlacement::new(
            "different",
            SCOPE,
            PlacementBinding::herdr("workspace", "lifetime").unwrap(),
        )
        .unwrap(),
    ] {
        assert!(
            f.state
                .save_placement_binding(&guard, &stream("host"), Some(&desired), &replacement)
                .is_err()
        );
    }
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn foreign_writer_refuses_before_runtime_read_or_mutation() {
    let f = Fixture::new();
    let guard = f.native.lock(1.0, &f.cancelled).unwrap();
    let error = f
        .state
        .save_placement_binding(&guard, &stream("foreign"), None, &placement())
        .unwrap_err();
    assert!(error.to_string().contains("fixed local writer"));
    assert!(!f.state.root().exists());
    f.put(&json!({"corrupt":true}));
    let path = f.state.placement_path("stream").unwrap();
    let before = fs::read(&path).unwrap();
    assert!(
        f.state
            .save_placement_binding(&guard, &stream("foreign"), None, &placement())
            .unwrap_err()
            .to_string()
            .contains("fixed local writer")
    );
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn pending_known_and_unknown_workspace_are_global_fences_not_binding_retries() {
    for known in [false, true] {
        let f = Fixture::new();
        let mut value = wire();
        value["pending"] = pending();
        if !known {
            value["binding"] = Value::Null;
        }
        f.put(&value);
        let loaded = f.state.placement(SCOPE, "stream").unwrap().unwrap();
        assert_eq!(
            loaded.pending().unwrap().workspace_id(),
            known.then_some("workspace")
        );
        assert_eq!(loaded.pending().unwrap().stream_id(), "stream");
        assert_eq!(serde_json::to_value(loaded.wire()).unwrap(), value);
        assert!(f.state.placement(OTHER, "stream").is_err());
        let path = f.state.placement_path("stream").unwrap();
        let before = fs::read(&path).unwrap();
        let guard = f.native.lock(1.0, &f.cancelled).unwrap();
        assert!(
            f.state
                .save_placement_binding(&guard, &stream("host"), Some(&loaded), &placement())
                .is_err()
        );
        assert!(
            f.state
                .save_placement_binding(&guard, &stream("host"), Some(&loaded), &loaded)
                .is_err()
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }
}

#[test]
fn cmux_roundtrip_has_no_fabricated_herdr_binding_capability() {
    let f = Fixture::new();
    for launch in [
        None,
        Some(LaunchReference::new("session", "pi --session '/saved'").unwrap()),
    ] {
        let binding = PlacementBinding::cmux("workspace", launch.clone()).unwrap();
        assert_eq!(binding.nonce(), None);
        assert_eq!(binding.launch(), launch.as_ref());
        let desired = NativePlacement::new("stream", SCOPE, binding).unwrap();
        f.put(&serde_json::to_value(desired.wire()).unwrap());
        assert_eq!(f.state.placement(SCOPE, "stream").unwrap(), Some(desired));
    }
}

#[test]
fn strict_placement_codec_rejects_missing_unknown_wrong_and_duplicate_fields() {
    let f = Fixture::new();
    for field in ["version", "stream_id", "scope", "binding", "pending"] {
        let mut value = wire();
        value.as_object_mut().unwrap().remove(field);
        f.put(&value);
        assert!(
            f.state.placement(SCOPE, "stream").is_err(),
            "accepted missing {field}"
        );
    }
    for (field, value) in [
        ("version", json!(1)),
        ("version", json!(2.0)),
        ("version", json!(true)),
        ("stream_id", json!("other")),
        ("stream_id", json!(" ")),
        ("scope", json!("bad")),
        ("binding", Value::Null),
        ("pending", json!({})),
        ("extra", Value::Null),
    ] {
        let mut bad = wire();
        bad[field] = value;
        f.put(&bad);
        assert!(
            f.state.placement(SCOPE, "stream").is_err(),
            "accepted {bad}"
        );
    }
    for binding in [
        json!({"frontend":"herdr","workspace_id":"workspace"}),
        json!({"frontend":"herdr","workspace_id":"workspace","nonce":""}),
        json!({"frontend":"herdr","workspace_id":"workspace","nonce":"lifetime","ready":true}),
        json!({"frontend":"cmux","workspace_id":"workspace","session_id":"session","command":null}),
        json!({"frontend":"cmux","workspace_id":"workspace","session_id":null,"command":null,"nonce":"lifetime"}),
    ] {
        let mut bad = wire();
        bad["binding"] = binding;
        f.put(&bad);
        assert!(
            f.state.placement(SCOPE, "stream").is_err(),
            "accepted {bad}"
        );
    }
    let path = f.state.placement_path("stream").unwrap();
    for bad in [
        serde_json::to_string(&wire()).unwrap().replacen(
            "\"version\":2",
            "\"version\":2,\"version\":2",
            1,
        ),
        serde_json::to_string(&wire()).unwrap().replacen(
            "\"nonce\":\"lifetime\"",
            "\"nonce\":\"lifetime\",\"nonce\":\"lifetime\"",
            1,
        ),
    ] {
        fs::write(&path, bad).unwrap();
        assert!(f.state.placement(SCOPE, "stream").is_err());
    }
}

#[test]
fn placement_paths_reject_aliases_and_special_files_without_following_them() {
    let f = Fixture::new();
    let outside = f.temp.path().join("outside");
    fs::write(&outside, "untouched").unwrap();
    let path = f.state.placement_path("stream").unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&outside, &path).unwrap();
    assert!(f.state.placement(SCOPE, "stream").is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "untouched");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(f.state.placement(SCOPE, "stream").is_err());
}
