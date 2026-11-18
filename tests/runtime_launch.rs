use std::{collections::BTreeMap, fs};
use streams::{
    model::Session,
    runtime::{
        conversations::{ConversationRoots, Harness},
        launch::SessionCommand,
        workspace::native_environment,
    },
};

fn session(id: &str, host: &str, harness: &str, native: &str, title: &str) -> Session {
    Session::new(id, host, "stream", harness, native, title).unwrap()
}
#[test]
fn native_environment_pins_defaults_without_credentials_or_caller_identity() {
    let caller = BTreeMap::from([("HOME".into(), "/caller".into())]);
    let defaults = native_environment("stream", &caller).unwrap();
    assert_eq!(defaults.len(), 10);
    for (key, value) in [
        ("STREAMS_STREAM_ID", "stream"),
        ("HOME", "/caller"),
        ("XDG_CONFIG_HOME", "/caller/.config"),
        ("XDG_DATA_HOME", "/caller/.local/share"),
        ("XDG_STATE_HOME", "/caller/.local/state"),
        ("PI_CODING_AGENT_DIR", "/caller/.pi/agent"),
        ("CODEX_HOME", "/caller/.codex"),
        ("CLAUDE_CONFIG_DIR", "/caller/.claude"),
        ("PATH", "/bin:/usr/bin"),
        ("HERDR_BIN_PATH", "herdr"),
    ] {
        assert_eq!(defaults[key], value);
    }
    let mut supplied = caller;
    for (key, value) in [
        ("XDG_CONFIG_HOME", ""),
        ("XDG_DATA_HOME", ""),
        ("XDG_STATE_HOME", ""),
        ("PI_CODING_AGENT_DIR", ""),
        ("HERDR_BIN_PATH", ""),
        ("CODEX_HOME", ""),
        ("CLAUDE_CONFIG_DIR", ""),
        ("PATH", ""),
        ("HERDR_SOCKET_PATH", "/caller/socket"),
        ("PI_OFFLINE", "1"),
        ("PI_CLIENT_SESSION_ID", "secret"),
        ("HERDR_PANE_ID", "caller-pane"),
        ("CMUX_WORKSPACE_ID", "caller-workspace"),
        ("HERDR_SESSION", "caller-namespace"),
    ] {
        supplied.insert(key.into(), value.into());
    }
    let result = native_environment("stream", &supplied).unwrap();
    assert_eq!(result.len(), 12);
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "PI_CODING_AGENT_DIR",
        "HERDR_BIN_PATH",
    ] {
        assert_eq!(result[key], defaults[key]);
    }
    for key in ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "PATH"] {
        assert_eq!(result[key], "");
    }
    assert_eq!(result["HERDR_SOCKET_PATH"], "/caller/socket");
    assert_eq!(result["PI_OFFLINE"], "1");
    supplied.insert("PI_OFFLINE".into(), "bad\0value".into());
    assert!(native_environment("stream", &supplied).is_err());
}
#[test]
fn commands_keep_harness_specific_resume_arguments_and_exact_verified_locator() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for (harness, id, body, argument) in [
        (
            Harness::Pi,
            "native-pi",
            include_str!("fixtures/runtime/pi.jsonl"),
            "--session",
        ),
        (
            Harness::Codex,
            "native-codex",
            include_str!("fixtures/runtime/codex.jsonl"),
            "resume",
        ),
        (
            Harness::Claude,
            "native-claude",
            include_str!("fixtures/runtime/claude.jsonl"),
            "--resume",
        ),
    ] {
        let path = root.join(format!("saved {id}.jsonl"));
        fs::write(&path, body).unwrap();
        let saved = session(
            "session",
            "host",
            harness.as_ref(),
            id,
            "Title is not identity",
        );
        let command =
            SessionCommand::verified(&saved, Some(&path), &ConversationRoots::default()).unwrap();
        assert_eq!(command.harness(), harness);
        assert_eq!(command.locator(), path);
        assert_eq!(command.argv()[0], harness.as_ref());
        assert_eq!(command.argv()[1], argument);
        assert_eq!(
            command.argv()[2],
            if harness == Harness::Pi {
                path.to_str().unwrap()
            } else {
                id
            }
        );
        let mut wrong = saved;
        wrong.harness_session_id = "not-the-file-identity".into();
        assert!(
            SessionCommand::verified(&wrong, Some(&path), &ConversationRoots::default()).is_err()
        );
    }
}
#[test]
fn unknown_harness_never_launches() {
    assert!(
        SessionCommand::verified(
            &session("future", "host", "future-harness", "native", "Future"),
            None,
            &ConversationRoots::default()
        )
        .is_err()
    );
}
