//! Saved identity cases ported from test_catalog_arrangement.py.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use streams_temper::runtime::RuntimeError;
use streams_temper::runtime::conversations::{
    ConversationRoots, Harness, NativeIdentityKind, NativeSessionMetadata, conversation_file,
    file_identity, native_identity,
};

fn root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let path = fs::canonicalize(temp.path()).unwrap();
    (temp, path)
}

fn fixture(harness: Harness) -> (&'static str, &'static str) {
    match harness {
        Harness::Pi => ("native-pi", include_str!("fixtures/runtime/pi.jsonl")),
        Harness::Codex => ("native-codex", include_str!("fixtures/runtime/codex.jsonl")),
        Harness::Claude => (
            "native-claude",
            include_str!("fixtures/runtime/claude.jsonl"),
        ),
    }
}

fn roots(path: &Path) -> ConversationRoots {
    ConversationRoots {
        pi: vec![path.into()],
        codex: vec![path.into()],
        claude: vec![path.into()],
    }
}

#[test]
fn native_enums_have_strict_lowercase_string_and_serde_conversions() {
    for (harness, value) in [
        (Harness::Pi, "pi"),
        (Harness::Codex, "codex"),
        (Harness::Claude, "claude"),
    ] {
        assert_eq!(value.parse::<Harness>().unwrap(), harness);
        assert_eq!(harness.to_string(), value);
        assert_eq!(harness.as_ref(), value);
        assert_eq!(serde_json::to_value(harness).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<Harness>(serde_json::json!(value)).unwrap(),
            harness
        );
    }
    for (kind, value) in [
        (NativeIdentityKind::Id, "id"),
        (NativeIdentityKind::Path, "path"),
    ] {
        assert_eq!(value.parse::<NativeIdentityKind>().unwrap(), kind);
        assert_eq!(kind.to_string(), value);
        assert_eq!(kind.as_ref(), value);
        assert_eq!(serde_json::to_value(kind).unwrap(), value);
        assert_eq!(
            serde_json::from_value::<NativeIdentityKind>(serde_json::json!(value)).unwrap(),
            kind
        );
    }
    for invalid in [
        "",
        "PI",
        "Pi",
        "Codex",
        "CLAUDE",
        " pi",
        "pi ",
        "unsupported",
    ] {
        assert!(invalid.parse::<Harness>().is_err());
        assert!(serde_json::from_value::<Harness>(serde_json::json!(invalid)).is_err());
    }
    for invalid in [
        "",
        "ID",
        "Id",
        "Path",
        "PATH",
        " path",
        "id ",
        "unsupported",
    ] {
        assert!(invalid.parse::<NativeIdentityKind>().is_err());
        assert!(serde_json::from_value::<NativeIdentityKind>(serde_json::json!(invalid)).is_err());
    }
}

#[test]
fn legacy_native_wire_fixtures_verify_source_specific_identity_not_cwd_or_filename() {
    let (_temp, root) = root();
    for harness in [Harness::Pi, Harness::Codex, Harness::Claude] {
        let path = root.join("misleading-title.jsonl");
        let (id, contents) = fixture(harness);
        fs::write(&path, contents).unwrap();
        assert_eq!(file_identity(harness, &path).unwrap(), id);
        assert_eq!(
            conversation_file(harness, id, Some(&path), &ConversationRoots::default()).unwrap(),
            path
        );
        assert!(
            conversation_file(harness, "wrong-id", Some(&path), &roots(&root))
                .unwrap_err()
                .to_string()
                .contains("mismatch")
        );
        assert!(conversation_file(harness, "/not-association", None, &roots(&root)).is_err());
        for wrong_harness in [Harness::Pi, Harness::Codex, Harness::Claude]
            .into_iter()
            .filter(|other| *other != harness)
        {
            assert!(file_identity(wrong_harness, &path).is_err());
        }
    }
}

#[test]
fn malformed_headers_duplicate_keys_control_ids_and_oversized_headers_are_errors() {
    let (_temp, root) = root();
    let path = root.join("a.jsonl");
    let mut invalid = vec![
        Vec::new(),
        b"{bad".to_vec(),
        b"[]\n".to_vec(),
        b"[\"session\",\"a\"]\n".to_vec(),
        b"null\n".to_vec(),
        b"{\"type\":\"message\",\"id\":\"a\"}\n".to_vec(),
        b"{\"type\":\"session\",\"id\":\"a\",\"id\":\"a\"}\n".to_vec(),
        b"{\"type\":\"session\",\"id\":\"a\",\"unknown\":{\"x\":1,\"x\":1}}\n".to_vec(),
    ];
    for id in [
        serde_json::json!(null),
        serde_json::json!(false),
        serde_json::json!(1),
        serde_json::json!(""),
        serde_json::json!("  "),
        serde_json::json!("bad\x7f"),
        serde_json::json!("bad\n"),
    ] {
        invalid.push(serde_json::to_vec(&serde_json::json!({"type":"session", "id":id})).unwrap());
    }
    invalid.push(
        format!(
            "{{\"type\":\"session\",\"id\":\"a\",\"padding\":\"{}\"}}\n",
            "x".repeat(65536)
        )
        .into_bytes(),
    );
    for bytes in invalid {
        fs::write(&path, &bytes).unwrap();
        assert!(
            file_identity(Harness::Pi, &path).is_err(),
            "accepted {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    assert!(file_identity(Harness::Pi, Path::new("relative.jsonl")).is_err());
    assert!(file_identity(Harness::Pi, &root).is_err());
    assert!(file_identity(Harness::Pi, &root.join("missing.jsonl")).is_err());
}

#[test]
fn codex_payload_identity_is_required_and_not_taken_from_top_level() {
    let (_temp, root) = root();
    let path = root.join("codex.jsonl");
    for contents in [
        "{\"type\":\"session_meta\",\"id\":\"top-level-only\"}\n",
        "{\"type\":\"session_meta\",\"payload\":{\"id\":null}}\n",
        "{\"type\":\"session_meta\",\"payload\":[\"a\"]}\n",
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"a\",\"id\":\"b\"}}\n",
    ] {
        fs::write(&path, contents).unwrap();
        assert!(file_identity(Harness::Codex, &path).is_err());
    }
    fs::write(
        &path,
        "{\"type\":\"session_meta\",\"id\":\"ignored\",\"payload\":{\"id\":\"canonical\"}}\n",
    )
    .unwrap();
    assert_eq!(file_identity(Harness::Codex, &path).unwrap(), "canonical");
}

#[test]
fn claude_complete_identity_records_must_agree_and_partial_final_record_is_ignored() {
    let (_temp, root) = root();
    let path = root.join("claude.jsonl");
    for contents in [
        "{\"type\":\"summary\"}\n",
        "{\"sessionId\":null}\n",
        "{\"sessionId\":\"\"}\n",
        "{\"sessionId\":1}\n",
        "{\"sessionId\":\"a\"}\n{\"sessionId\":\"b\"}\n",
        "{\"sessionId\":\"a\",\"sessionId\":\"a\"}\n",
        "{\"sessionId\":\"a\"}\n{bad}\n",
        "{\"sessionId\":\"a\"}",
    ] {
        fs::write(&path, contents).unwrap();
        assert!(
            file_identity(Harness::Claude, &path).is_err(),
            "accepted {contents}"
        );
    }
    fs::write(
        &path,
        "{\"sessionId\":\"a\"}\n{\"sessionId\":\"a\"}\n{\"sessionId\":\"b\"",
    )
    .unwrap();
    assert_eq!(file_identity(Harness::Claude, &path).unwrap(), "a");
    fs::write(
        &path,
        format!(
            "{{\"sessionId\":\"a\"}}\n{{\"padding\":\"{}\"}}\n",
            "x".repeat(65536)
        ),
    )
    .unwrap();
    assert_eq!(file_identity(Harness::Claude, &path).unwrap(), "a");
}

#[test]
fn pi_only_reads_bounded_first_header_not_later_messages() {
    let (_temp, root) = root();
    let path = root.join("pi.jsonl");
    fs::write(
        &path,
        format!(
            "{{\"type\":\"session\",\"id\":\"a\"}}\n{}",
            "not-json".repeat(20000)
        ),
    )
    .unwrap();
    assert_eq!(file_identity(Harness::Pi, &path).unwrap(), "a");
    fs::write(&path, "{\"type\":\"session\",\"id\":\"a\"}").unwrap();
    assert_eq!(file_identity(Harness::Pi, &path).unwrap(), "a");
}

#[test]
fn native_metadata_checks_expected_harness_and_canonicalizes_path_identity() {
    let (_temp, root) = root();
    let path = root.join("locator.jsonl");
    for harness in [Harness::Pi, Harness::Codex, Harness::Claude] {
        let (id, contents) = fixture(harness);
        fs::write(&path, contents).unwrap();
        let metadata = NativeSessionMetadata {
            agent: harness,
            kind: NativeIdentityKind::Path,
            value: path.to_str().unwrap().into(),
        };
        let native = native_identity(&metadata, harness).unwrap();
        assert_eq!(native.id, id);
        assert_eq!(native.locator, Some(path.clone()));
        assert!(
            native_identity(
                &metadata,
                if harness == Harness::Pi {
                    Harness::Codex
                } else {
                    Harness::Pi
                }
            )
            .is_err()
        );
        let metadata = NativeSessionMetadata {
            agent: harness,
            kind: NativeIdentityKind::Id,
            value: id.into(),
        };
        let native = native_identity(&metadata, harness).unwrap();
        assert_eq!(native.id, id);
        assert!(native.locator.is_none());
    }
    for value in ["", "  ", "bad\x7f"] {
        let metadata = NativeSessionMetadata {
            agent: Harness::Pi,
            kind: NativeIdentityKind::Id,
            value: value.into(),
        };
        assert!(native_identity(&metadata, Harness::Pi).is_err());
    }
    let metadata = NativeSessionMetadata {
        agent: Harness::Pi,
        kind: NativeIdentityKind::Path,
        value: "relative.jsonl".into(),
    };
    assert!(native_identity(&metadata, Harness::Pi).is_err());
}

#[test]
fn discovery_verifies_hints_ignores_other_identity_but_does_not_hide_malformed_matches() {
    let (_temp, root) = root();
    let roots = roots(&root);
    let path = root.join("native-pi.jsonl");
    fs::write(&path, fixture(Harness::Pi).1).unwrap();
    let decoy = root.join("native-pi-decoy.jsonl");
    fs::write(&decoy, "{\"type\":\"session\",\"id\":\"unrelated\"}\n").unwrap();
    assert_eq!(
        conversation_file(Harness::Pi, "native-pi", None, &roots).unwrap(),
        path
    );
    assert!(conversation_file(Harness::Pi, "native-pi", Some(&decoy), &roots).is_err());
    fs::write(&decoy, "{bad").unwrap();
    assert!(conversation_file(Harness::Pi, "native-pi", None, &roots).is_err());
    fs::remove_file(&decoy).unwrap();
    fs::write(root.join("native-pi-second.jsonl"), fixture(Harness::Pi).1).unwrap();
    assert!(matches!(
        conversation_file(Harness::Pi, "native-pi", None, &roots),
        Err(RuntimeError::Ambiguous { .. })
    ));
}

#[test]
fn native_aliases_deduplicate_by_canonical_locator_and_directory_cycles_are_not_followed() {
    let (_temp, root) = root();
    let path = root.join("native-pi.jsonl");
    fs::write(&path, fixture(Harness::Pi).1).unwrap();
    let alias = root.join("native-pi-alias.jsonl");
    symlink(&path, &alias).unwrap();
    symlink(&root, root.join("cycle")).unwrap();
    assert_eq!(
        conversation_file(Harness::Pi, "native-pi", None, &roots(&root)).unwrap(),
        path
    );
    assert_eq!(
        conversation_file(Harness::Pi, "native-pi", Some(&alias), &roots(&root)).unwrap(),
        path
    );
    fs::remove_file(&path).unwrap();
    assert!(conversation_file(Harness::Pi, "native-pi", None, &roots(&root)).is_err());
}

#[test]
fn missing_roots_are_absence_but_malformed_roots_are_errors() {
    let (_temp, root) = root();
    let missing = roots(&root.join("missing"));
    assert!(matches!(
        conversation_file(Harness::Pi, "id", None, &missing),
        Err(RuntimeError::NotFound(_))
    ));
    let not_directory = root.join("file");
    fs::write(&not_directory, "not a directory").unwrap();
    assert!(matches!(
        conversation_file(Harness::Pi, "id", None, &roots(&not_directory)),
        Err(RuntimeError::Io(_))
    ));
}

#[test]
fn exact_saved_locator_bypasses_discovery() {
    let (_temp, root) = root();
    let unreadable_root = root.join("not-a-directory");
    fs::write(&unreadable_root, "").unwrap();
    let exact = root.join("exact.jsonl");
    fs::write(&exact, fixture(Harness::Pi).1).unwrap();
    assert_eq!(
        conversation_file(
            Harness::Pi,
            "native-pi",
            Some(&exact),
            &roots(&unreadable_root)
        )
        .unwrap(),
        exact
    );
}
