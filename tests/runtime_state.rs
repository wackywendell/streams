//! Native runtime state contracts; every filesystem operation is confined to a temp directory.

use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use streams_temper::operations::Authority;
use streams_temper::runtime::state::{Association, LaunchReference, RuntimeState, herdr_scope};

const SCOPE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const WORKSPACE_KEY: &str = "54d2f7dd1bee8efcf49681349de679f40745c8dd5681d57414c5bcb75d499ba5";
const STREAM_KEY: &str = "e42d5ae88f18e4b353fb6b45ead902d587a9ea78ee7bae58621c5a19426309e1";

#[test]
fn cmux_scope_preserves_python_ascii_identity() {
    assert_eq!(
        streams_temper::runtime::state::cmux_scope(Path::new("/tmp/cmux-😀.sock")),
        "61613c57cc2cc3a1dcf2cee93359682c3b0cf4eae39b8bf9483affb75de72087"
    );
}

#[test]
fn herdr_scope_preserves_python_ascii_and_nullable_namespace() {
    let socket = Path::new("/tmp/streams-😀.sock");
    assert_eq!(
        herdr_scope(socket, None),
        "88c190d06d2fc57cb400a16fcb7b869e768760b29365649c61dc18f8a51f6ebc"
    );
    assert_eq!(
        herdr_scope(socket, Some("space / ☃")),
        "1dcb503ee65305358cf0ed778e2a8da304ef09559c49e841441f6768ba57dd87"
    );
}

fn fixture() -> tempfile::TempDir {
    let base = std::env::temp_dir().canonicalize().unwrap();
    tempfile::tempdir_in(base).unwrap()
}

fn state(root: &Path) -> RuntimeState {
    RuntimeState::new(root.join("runtime"), "host").unwrap()
}
fn authority() -> Authority {
    Authority::new("host", "effective-token").unwrap()
}
fn workspace_path(state: &RuntimeState, collection: &str) -> PathBuf {
    state
        .root()
        .join(collection)
        .join(SCOPE)
        .join(format!("{WORKSPACE_KEY}.json"))
}
fn attempt_path(state: &RuntimeState) -> PathBuf {
    state
        .root()
        .join("attempts")
        .join(SCOPE)
        .join(format!("{STREAM_KEY}.json"))
}
fn put(path: &Path, value: &Value) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
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

#[test]
fn python_hashes_paths_and_exact_record_shapes_roundtrip() {
    let temp = fixture();
    let state = state(temp.path());
    assert_eq!(
        state.root(),
        temp.path()
            .join("runtime/9a7e22984a1c6f4b91c8bfc2d3ae0b61ee51bd3681d11cb615b4683bcef860c0")
    );
    let unicode = RuntimeState::new(temp.path(), "höst😀").unwrap();
    assert_eq!(
        unicode.root(),
        temp.path()
            .join("b490dec8b2f0bfddf7b4e43f0297321a29cab2376e1386f36ebeaedc481278f6")
    );
    {
        let _conversation = state.conversation_lock("pi", "natïve😀").unwrap();
        assert!(state.root().join("agent-locks/4b0a701c6eb9879b701068cc8c85062fce9e52859e88f39a3671f4ea63c9816c.lock").is_file());
    }
    {
        let mut enrollment = state.enrollment(Some((SCOPE, "workspace")), None).unwrap();
        enrollment.claim("stream").unwrap();
        assert!(
            workspace_path(&state, "enrollments")
                .with_extension("lock")
                .is_file()
        );
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(workspace_path(&state, "enrollments")).unwrap()
            )
            .unwrap(),
            claim_wire()
        );
        enrollment.bind("stream", &authority()).unwrap();
    }
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(workspace_path(&state, "associations")).unwrap())
            .unwrap(),
        association_wire()
    );
    let association = state.association(SCOPE, "workspace").unwrap().unwrap();
    assert!(association.ready());
    assert_eq!(association.launch(), None);
    assert_eq!(association.stream_id(), "stream");
    assert_eq!(association.workspace_id(), "workspace");
    assert_eq!(
        state
            .enrollment_claim(SCOPE, "workspace")
            .unwrap()
            .unwrap()
            .stream_id(),
        "stream"
    );
    let mut stream = state.stream_lock(Some(SCOPE), "stream").unwrap();
    assert!(
        state
            .root()
            .join(format!("stream-locks/{STREAM_KEY}.lock"))
            .is_file()
    );
    let launch = LaunchReference::new("session", "pi --session '/saved'").unwrap();
    let mut attempt = stream
        .begin_attempt(Some(launch.clone()), "/chosen", Some("workspace"))
        .unwrap();
    let mut expected = attempt_wire();
    expected["session_id"] = json!("session");
    expected["command"] = json!("pi --session '/saved'");
    expected["workspace_id"] = json!("workspace");
    assert_eq!(
        serde_json::from_slice::<Value>(&fs::read(attempt_path(&state)).unwrap()).unwrap(),
        expected
    );
    // Existing Python array contents are opaque evidence, not a native command language.
    let effects = vec![json!(null), json!({"unknown-diagnostic":[1,true]})];
    let launches = vec![json!("opaque")];
    attempt
        .checkpoint(Some("workspace"), effects.clone(), launches.clone())
        .unwrap();
    let loaded = state.pending_attempt(SCOPE, "stream").unwrap().unwrap();
    assert_eq!(loaded.launch().unwrap(), &launch);
    assert_eq!(loaded.cwd(), "/chosen");
    assert_eq!(loaded.effects(), effects.as_slice());
    assert_eq!(loaded.launches(), launches.as_slice());
    attempt
        .write_association(&Association::new("stream", "workspace", Some(launch), true).unwrap())
        .unwrap();
    attempt.complete().unwrap();
    assert!(state.pending_attempt(SCOPE, "stream").unwrap().is_none());
}

#[test]
fn all_read_only_inventories_and_missing_reads_leave_no_state() {
    let temp = fixture();
    let state = state(temp.path());
    assert!(state.association(SCOPE, "workspace").unwrap().is_none());
    assert!(
        state
            .enrollment_claim(SCOPE, "workspace")
            .unwrap()
            .is_none()
    );
    assert!(state.pending_attempt(SCOPE, "stream").unwrap().is_none());
    for scope in [None, Some(SCOPE)] {
        assert!(state.associations(scope).unwrap().is_empty());
        assert!(state.enrollment_claims(scope).unwrap().is_empty());
        assert!(state.pending_attempts(scope).unwrap().is_empty());
    }
    assert!(!state.root().exists());
    assert!(!temp.path().join("runtime").exists());
}

#[test]
fn malformed_associations_fail_closed() {
    let temp = fixture();
    let state = state(temp.path());
    let path = workspace_path(&state, "associations");
    let mut cases = Vec::new();
    for field in ["version", "session_id", "command", "stream_id", "ready"] {
        let mut value = association_wire();
        value.as_object_mut().unwrap().remove(field);
        cases.push(value);
    }
    for (field, value) in [
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
        let mut record = association_wire();
        record[field] = value;
        cases.push(record);
    }
    cases.push(json!([1, "stream", "workspace", null, null, true]));
    for malformed in cases {
        put(&path, &malformed);
        assert!(
            state.association(SCOPE, "workspace").is_err(),
            "accepted {malformed}"
        );
        assert!(state.associations(None).is_err());
    }
    fs::write(&path, r#"{"version":1,"stream_id":"stream","stream_id":"stream","workspace_id":"workspace","session_id":null,"command":null,"ready":true}"#).unwrap();
    assert!(state.association(SCOPE, "workspace").is_err());
}

#[test]
fn malformed_claims_attempt_pairs_and_arrays_fail_closed() {
    let temp = fixture();
    let state = state(temp.path());
    let path = workspace_path(&state, "enrollments");
    for (field, bad) in [
        ("version", json!(0)),
        ("stream_id", json!(null)),
        ("workspace_id", json!("wrong")),
        ("extra", json!(true)),
    ] {
        let mut value = claim_wire();
        value[field] = bad;
        put(&path, &value);
        assert!(state.enrollment_claim(SCOPE, "workspace").is_err());
    }
    let path = attempt_path(&state);
    for field in [
        "session_id",
        "command",
        "workspace_id",
        "effects",
        "launches",
        "cwd",
        "version",
    ] {
        let mut value = attempt_wire();
        value.as_object_mut().unwrap().remove(field);
        put(&path, &value);
        assert!(
            state.pending_attempt(SCOPE, "stream").is_err(),
            "accepted missing {field}"
        );
    }
    for (field, bad) in [
        ("version", json!(2)),
        ("session_id", json!("session")),
        ("command", json!("command")),
        ("workspace_id", json!("")),
        ("cwd", json!(null)),
        ("effects", json!({})),
        ("launches", json!(null)),
        ("stream_id", json!("wrong")),
        ("extra", json!([])),
    ] {
        let mut value = attempt_wire();
        value[field] = bad;
        put(&path, &value);
        assert!(
            state.pending_attempt(SCOPE, "stream").is_err(),
            "accepted {value}"
        );
    }
    let mut value = attempt_wire();
    value["effects"] = json!([null, 1, ["anything"], {"unknown":"evidence"}]);
    value["launches"] = json!([false]);
    put(&path, &value);
    assert!(state.pending_attempt(SCOPE, "stream").is_ok());
    fs::write(&path, r#"{"version":1,"stream_id":"stream","session_id":null,"command":null,"cwd":"/chosen","workspace_id":null,"effects":[{"x":1,"x":2}],"launches":[]}"#).unwrap();
    assert!(state.pending_attempt(SCOPE, "stream").is_err());
}

#[test]
fn symlinked_records_and_special_files_are_rejected() {
    let temp = fixture();
    let state = state(temp.path());
    let path = workspace_path(&state, "associations");
    put(&path, &association_wire());
    let target = temp.path().join("target");
    fs::rename(&path, &target).unwrap();
    symlink(&target, &path).unwrap();
    assert!(state.association(SCOPE, "workspace").is_err());
    assert!(state.associations(None).is_err());
    fs::remove_file(&path).unwrap();
    let name = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(state.association(SCOPE, "workspace").is_err());
    assert!(state.stream_lock(Some(SCOPE), "other-stream").is_err());
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(state.association(SCOPE, "workspace").is_err());
    fs::remove_dir(&path).unwrap();
    let attempt = attempt_path(&state);
    fs::create_dir_all(attempt.parent().unwrap()).unwrap();
    let name = CString::new(attempt.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(state.pending_attempt(SCOPE, "stream").is_err());
    assert!(state.pending_attempts(None).is_err());
}

#[test]
fn symlink_scope_and_lock_are_rejected_without_following() {
    let temp = fixture();
    let state = state(temp.path());
    fs::create_dir_all(state.root().join("associations")).unwrap();
    let target = temp.path().join("outside");
    fs::create_dir(&target).unwrap();
    symlink(&target, state.root().join("associations").join(SCOPE)).unwrap();
    assert!(state.associations(None).is_err());
    assert!(state.association(SCOPE, "workspace").is_err());
    fs::remove_file(state.root().join("associations").join(SCOPE)).unwrap();
    fs::create_dir_all(state.root().join("stream-locks")).unwrap();
    let destination = target.join("lock");
    fs::write(&destination, "untouched").unwrap();
    symlink(
        &destination,
        state.root().join(format!("stream-locks/{STREAM_KEY}.lock")),
    )
    .unwrap();
    assert!(state.stream_lock(Some(SCOPE), "stream").is_err());
    assert_eq!(fs::read_to_string(destination).unwrap(), "untouched");
    let linked = temp.path().join("linked");
    symlink(temp.path().join("runtime"), &linked).unwrap();
    let state = RuntimeState::new(linked, "host").unwrap();
    assert!(state.pending_attempts(None).is_err());
}

#[test]
fn symlink_above_configured_base_is_allowed() {
    for existing_base in [false, true] {
        let temp = fixture();
        let target = temp.path().join("target");
        fs::create_dir(&target).unwrap();
        if existing_base {
            fs::create_dir_all(target.join("missing-parent/runtime")).unwrap();
        }
        let linked = temp.path().join("linked");
        symlink(&target, &linked).unwrap();
        let state = state(&linked.join("missing-parent"));
        assert!(state.root().starts_with(&target));
        assert!(state.associations(None).unwrap().is_empty());
        assert_eq!(
            target.join("missing-parent/runtime").exists(),
            existing_base
        );
        state
            .enrollment(Some((SCOPE, "workspace")), None)
            .unwrap()
            .bind("stream", &authority())
            .unwrap();
        assert!(
            state
                .association(SCOPE, "workspace")
                .unwrap()
                .unwrap()
                .ready()
        );
    }
}

#[test]
fn symlinks_at_configured_base_and_owned_descendants_are_rejected() {
    for level in 0..4 {
        let temp = fixture();
        let state = state(temp.path());
        let path = match level {
            0 => temp.path().join("runtime"),
            1 => state.root().to_path_buf(),
            2 => state.root().join("associations"),
            _ => state.root().join("associations").join(SCOPE),
        };
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let target = temp.path().join("outside");
        fs::create_dir(&target).unwrap();
        symlink(&target, &path).unwrap();
        assert!(state.association(SCOPE, "workspace").is_err());
        assert!(state.associations(None).is_err());
        assert!(
            state
                .stream_lock(Some(SCOPE), "stream")
                .and_then(|mut guard| {
                    guard.write_association(&Association::new("stream", "workspace", None, true)?)
                })
                .is_err()
        );
        assert!(fs::read_dir(target).unwrap().next().is_none());
    }
}

#[test]
fn scope_validation_and_record_construction_do_not_create_state() {
    let temp = fixture();
    let state = state(temp.path());
    for scope in [
        "",
        "../escape",
        "ABC",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/",
    ] {
        assert!(state.associations(Some(scope)).is_err());
        assert!(state.stream_lock(Some(scope), "stream").is_err());
        assert!(state.enrollment(Some((scope, "workspace")), None).is_err());
    }
    assert!(Association::new("", "workspace", None, true).is_err());
    assert!(LaunchReference::new("session", "").is_err());
    assert!(RuntimeState::new(temp.path(), "\0").is_err());
    assert!(!state.root().exists());
}

#[test]
fn associations_exclude_other_scopes_and_workspaces_including_unscoped_enrollment() {
    let temp = fixture();
    let state = state(temp.path());
    state
        .stream_lock(Some(SCOPE), "stream")
        .unwrap()
        .write_association(&Association::new("stream", "workspace", None, true).unwrap())
        .unwrap();
    assert!(state.stream_lock(Some(OTHER), "stream").is_err());
    assert!(state.stream_lock(None, "stream").is_err());
    let mut unscoped = state.enrollment(None, None).unwrap();
    assert!(unscoped.claim("stream").is_err());
    let mut enrollment = state
        .enrollment(Some((SCOPE, "another-workspace")), None)
        .unwrap();
    assert!(enrollment.claim("stream").is_err());
    let mut enrollment = state.enrollment(Some((OTHER, "workspace")), None).unwrap();
    assert!(enrollment.claim("stream").is_err());
}

#[test]
fn claims_exclude_open_other_scopes_and_workspaces() {
    let temp = fixture();
    let state = state(temp.path());
    {
        let mut enrollment = state.enrollment(Some((SCOPE, "workspace")), None).unwrap();
        enrollment.claim("stream").unwrap();
    }
    let mut stream = state.stream_lock(Some(SCOPE), "stream").unwrap();
    assert!(stream.check_open().is_err());
    assert!(stream.begin_attempt(None, "/chosen", None).is_err());
    drop(stream);
    assert!(state.stream_lock(Some(OTHER), "stream").is_err());
    assert!(state.stream_lock(None, "stream").is_err());
    let mut enrollment = state
        .enrollment(Some((SCOPE, "another-workspace")), None)
        .unwrap();
    assert!(enrollment.claim("stream").is_err());
    assert!(
        state
            .enrollment(Some((SCOPE, "workspace")), Some("different"))
            .is_err()
    );
}

#[test]
fn pending_attempts_block_every_scope_and_never_resume_from_loaded_evidence() {
    let temp = fixture();
    let state = state(temp.path());
    {
        let mut stream = state.stream_lock(Some(SCOPE), "stream").unwrap();
        let mut attempt = stream.begin_attempt(None, "/chosen", None).unwrap();
        attempt
            .checkpoint(
                Some("workspace"),
                vec![json!({"kind":"workspace_create"})],
                vec![],
            )
            .unwrap();
        assert!(attempt.complete().is_err());
    }
    for scope in [Some(SCOPE), Some(OTHER), None] {
        assert!(state.stream_lock(scope, "stream").is_err());
    }
    let loaded = state.pending_attempt(SCOPE, "stream").unwrap().unwrap();
    assert_eq!(loaded.workspace_id(), Some("workspace"));
    assert_eq!(loaded.effects().len(), 1);
    assert!(state.enrollment(Some((SCOPE, "workspace")), None).is_err());
    assert!(
        state
            .enrollment_claim(SCOPE, "workspace")
            .unwrap()
            .is_none()
    );
    let mut unscoped = state.enrollment(None, None).unwrap();
    assert!(unscoped.claim("stream").is_err());
}

#[test]
fn failed_bind_retains_claim_and_never_creates_association() {
    let temp = fixture();
    let state = state(temp.path());
    let mut enrollment = state.enrollment(Some((SCOPE, "workspace")), None).unwrap();
    enrollment.claim("stream").unwrap();
    let wrong_host = Authority::new("other-host", "token").unwrap();
    assert!(enrollment.bind("stream", &wrong_host).is_err());
    assert!(
        state
            .enrollment_claim(SCOPE, "workspace")
            .unwrap()
            .is_some()
    );
    assert!(state.association(SCOPE, "workspace").unwrap().is_none());
    // A deterministic write failure after claiming must not discard the surviving evidence.
    fs::create_dir_all(workspace_path(&state, "associations")).unwrap();
    assert!(enrollment.bind("stream", &authority()).is_err());
    assert_eq!(
        state
            .enrollment_claim(SCOPE, "workspace")
            .unwrap()
            .unwrap()
            .stream_id(),
        "stream"
    );
    assert!(enrollment.claim("replacement").is_err());
}

#[test]
fn open_created_binding_cannot_mint_automatic_enrollment_evidence() {
    let temp = fixture();
    let state = state(temp.path());
    let launch = LaunchReference::new("exact-session", "exact-command").unwrap();
    let association = Association::new("stream", "workspace", Some(launch.clone()), true).unwrap();
    state
        .stream_lock(Some(SCOPE), "stream")
        .unwrap()
        .write_association(&association)
        .unwrap();
    let mut enrollment = state.enrollment(Some((SCOPE, "workspace")), None).unwrap();
    assert_eq!(enrollment.stream_id(), Some("stream"));
    enrollment.claim("stream").unwrap();
    enrollment.bind("stream", &authority()).unwrap();
    assert!(
        state
            .enrollment_claim(SCOPE, "workspace")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        state.association(SCOPE, "workspace").unwrap().unwrap(),
        association
    );
    assert_eq!(launch.session_id(), "exact-session");
    assert_eq!(launch.command(), "exact-command");
}

#[test]
fn stream_lock_is_global_across_frontends_and_released_by_drop() {
    let temp = fixture();
    let state = state(temp.path());
    let first = state.stream_lock(Some(SCOPE), "stream").unwrap();
    let (started_tx, started_rx) = mpsc::channel();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let second_state = state.clone();
    let thread = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let _second = second_state.stream_lock(Some(OTHER), "stream").unwrap();
        acquired_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        acquired_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    // A distinct stream remains independent while the first stream is locked.
    let independent = state.stream_lock(Some(OTHER), "independent").unwrap();
    drop(independent);
    drop(first);
    acquired_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    thread.join().unwrap();
}

#[test]
fn abandoned_attempt_blocks_reuse_even_while_original_stream_lock_is_held() {
    let temp = fixture();
    let state = state(temp.path());
    let mut stream = state.stream_lock(Some(SCOPE), "stream").unwrap();
    drop(stream.begin_attempt(None, "/chosen", None).unwrap());
    assert!(stream.check_open().is_err());
    assert!(stream.begin_attempt(None, "/chosen", None).is_err());
}

#[test]
fn no_scope_enrollment_reserves_no_native_record_and_partial_bindings_block_enrollment() {
    let temp = fixture();
    let state = state(temp.path());
    let mut enrollment = state.enrollment(None, Some("stream")).unwrap();
    enrollment.claim("stream").unwrap();
    enrollment.bind("stream", &authority()).unwrap();
    assert_eq!(enrollment.stream_id(), Some("stream"));
    assert!(state.associations(None).unwrap().is_empty());
    assert!(state.enrollment_claims(None).unwrap().is_empty());
    assert!(state.pending_attempts(None).unwrap().is_empty());
    assert!(enrollment.claim("replacement").is_err());
    state
        .stream_lock(Some(SCOPE), "stream")
        .unwrap()
        .write_association(&Association::new("stream", "workspace", None, false).unwrap())
        .unwrap();
    assert!(state.enrollment(Some((SCOPE, "workspace")), None).is_err());
    assert!(
        state
            .stream_lock(Some(SCOPE), "stream")
            .unwrap()
            .check_open()
            .is_err()
    );
}

#[test]
fn binding_with_another_streams_claim_blocks_open_before_journaling() {
    let temp = fixture();
    let state = state(temp.path());
    put(&workspace_path(&state, "associations"), &association_wire());
    let mut claim = claim_wire();
    claim["stream_id"] = json!("other-stream");
    put(&workspace_path(&state, "enrollments"), &claim);
    let mut guard = state.stream_lock(Some(SCOPE), "stream").unwrap();
    assert!(guard.check_open().is_err());
    assert!(
        guard
            .begin_attempt(None, "/chosen", Some("workspace"))
            .is_err()
    );
    assert!(!attempt_path(&state).exists());
}

fn put_other_workspace_attempt(state: &RuntimeState, scope: &str) {
    let mut pending = attempt_wire();
    pending["stream_id"] = json!("other-stream");
    pending["workspace_id"] = json!("workspace");
    let filename = format!("{:x}.json", Sha256::digest(br#"["other-stream"]"#));
    put(
        &state.root().join("attempts").join(scope).join(filename),
        &pending,
    );
}

#[test]
fn another_streams_pending_workspace_blocks_open_and_association() {
    let temp = fixture();
    let state = state(temp.path());
    let mut guard = state.stream_lock(Some(SCOPE), "stream").unwrap();
    let binding = Association::new("stream", "workspace", None, true).unwrap();
    // Other scopes do not reserve this workspace identity.
    put_other_workspace_attempt(&state, OTHER);
    guard.write_association(&binding).unwrap();
    assert!(guard.check_open().is_ok());
    put_other_workspace_attempt(&state, SCOPE);
    assert!(guard.check_open().is_err());
    assert!(
        guard
            .begin_attempt(None, "/chosen", Some("workspace"))
            .is_err()
    );
    assert!(!attempt_path(&state).exists());
    let previous = fs::read(workspace_path(&state, "associations")).unwrap();
    assert!(guard.write_association(&binding).is_err());
    assert_eq!(
        fs::read(workspace_path(&state, "associations")).unwrap(),
        previous
    );
    assert!(state.enrollment(Some((SCOPE, "workspace")), None).is_err());
}

#[test]
fn checkpoint_rejects_bound_claimed_or_pending_workspace_without_changing_journal() {
    for conflict in ["association", "claim", "attempt"] {
        let temp = fixture();
        let state = state(temp.path());
        let mut guard = state.stream_lock(Some(SCOPE), "stream").unwrap();
        let mut attempt = guard.begin_attempt(None, "/chosen", None).unwrap();
        match conflict {
            "association" => {
                let mut binding = association_wire();
                binding["stream_id"] = json!("other-stream");
                put(&workspace_path(&state, "associations"), &binding);
            }
            "claim" => {
                let mut claim = claim_wire();
                claim["stream_id"] = json!("other-stream");
                put(&workspace_path(&state, "enrollments"), &claim);
            }
            _ => put_other_workspace_attempt(&state, SCOPE),
        }
        let previous = fs::read(attempt_path(&state)).unwrap();
        assert!(
            attempt
                .checkpoint(Some("workspace"), vec![json!("effect")], vec![])
                .is_err()
        );
        assert_eq!(fs::read(attempt_path(&state)).unwrap(), previous);
        assert_eq!(attempt.record().workspace_id(), None);
        assert!(attempt.record().effects().is_empty());
    }
}

#[test]
fn enrollment_rechecks_partial_binding_after_guard_creation() {
    for bind in [false, true] {
        let temp = fixture();
        let state = state(temp.path());
        let mut enrollment = state.enrollment(Some((SCOPE, "workspace")), None).unwrap();
        let mut partial = association_wire();
        partial["ready"] = json!(false);
        put(&workspace_path(&state, "associations"), &partial);
        if bind {
            assert!(enrollment.bind("stream", &authority()).is_err());
        } else {
            assert!(enrollment.claim("stream").is_err());
        }
        assert!(
            state
                .enrollment_claim(SCOPE, "workspace")
                .unwrap()
                .is_none()
        );
        assert!(
            !state
                .association(SCOPE, "workspace")
                .unwrap()
                .unwrap()
                .ready()
        );
        assert_eq!(enrollment.stream_id(), None);
    }
}

#[test]
fn duplicate_stream_bindings_and_filename_mismatches_block_use() {
    let temp = fixture();
    let state = state(temp.path());
    put(&workspace_path(&state, "associations"), &association_wire());
    let mut second = association_wire();
    second["workspace_id"] = json!("second");
    let filename = format!("{:x}.json", Sha256::digest(br#"["second"]"#));
    put(
        &state.root().join("associations").join(SCOPE).join(filename),
        &second,
    );
    assert!(
        state
            .stream_lock(Some(SCOPE), "stream")
            .unwrap()
            .check_open()
            .is_err()
    );
    let mut wrong = attempt_wire();
    wrong["stream_id"] = json!("different");
    put(&attempt_path(&state), &wrong);
    assert!(state.pending_attempts(None).is_err());
    assert!(state.stream_lock(Some(SCOPE), "unrelated").is_err());
}
