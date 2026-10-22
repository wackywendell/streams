//! The shared native runner retains Git's pipe/deadline safety without shell wrappers.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use streams_temper::process::{NativeProcessRunner, ProcessRequest, ProcessRunner};

fn fixture(script: &str, timeout: f64) -> ProcessRequest {
    ProcessRequest {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        cwd: None,
        input: None,
        env: BTreeMap::from([("PATH".into(), "/usr/bin:/bin".into())]),
        allowed: vec![0],
        timeout,
    }
}

#[test]
fn runner_supports_explicit_environment_cwd_stdin_and_allowed_exit_codes() {
    let root = tempfile::tempdir().unwrap();
    let mut request = fixture(
        "printf '%s\\n' \"$EXPLICIT_VALUE\"; pwd -P; cat; printf diagnostic >&2; exit 7",
        2.0,
    );
    request.cwd = Some(root.path().to_path_buf());
    request.env.insert("EXPLICIT_VALUE".into(), "scoped".into());
    request.input = Some(b"input-bytes".to_vec());
    request.allowed = vec![7];
    let output = NativeProcessRunner.run(&request).unwrap();
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stderr, b"diagnostic");
    assert_eq!(
        output.stdout,
        format!(
            "scoped\n{}\ninput-bytes",
            root.path().canonicalize().unwrap().display()
        )
        .as_bytes()
    );
    request.allowed = vec![0];
    let error = NativeProcessRunner.run(&request).unwrap_err();
    assert_eq!(error.code, Some(7));
    assert!(error.stdout.starts_with(b"scoped"));
    assert_eq!(error.stderr, b"diagnostic");
}

#[test]
fn runner_drains_both_pipes_while_feeding_stdin() {
    let mut request = fixture(
        "dd if=/dev/zero bs=65536 count=4 2>/dev/null; dd if=/dev/zero bs=65536 count=4 >&2 2>/dev/null; cat",
        3.0,
    );
    request.input = Some(vec![b'x'; 1024 * 1024]);
    let output = NativeProcessRunner.run(&request).unwrap();
    assert_eq!(output.stderr, vec![0; 262144]);
    assert_eq!(&output.stdout[..262144], vec![0; 262144]);
    assert_eq!(&output.stdout[262144..], request.input.unwrap());
}

#[test]
fn timeout_preserves_partial_output() {
    let request = fixture(
        "printf created-workspace; printf native-diagnostic >&2; sleep 30",
        1.0,
    );
    let start = Instant::now();
    let error = NativeProcessRunner.run(&request).unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(3));
    assert!(error.message.contains("timeout"));
    assert_eq!(error.code, None);
    assert_eq!(error.stdout, b"created-workspace");
    assert_eq!(error.stderr, b"native-diagnostic");
}

#[test]
fn deadline_includes_inherited_pipes_and_blocked_input() {
    for script in ["sleep 30 & exit 0", "sleep 30"] {
        let mut request = fixture(script, 0.15);
        request.input = Some(vec![b'x'; 1024 * 1024]);
        let start = Instant::now();
        let error = NativeProcessRunner.run(&request).unwrap_err();
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(error.message.contains("timeout"));
        assert_eq!(error.code, None);
    }
}

#[test]
fn invalid_timeouts_and_missing_program_fail_without_inventing_success() {
    for timeout in [0.0, -1.0, f64::NAN, f64::INFINITY, f64::MAX] {
        assert!(
            NativeProcessRunner
                .run(&fixture("exit 0", timeout))
                .unwrap_err()
                .message
                .contains("timeout")
        );
    }
    let mut request = fixture("", 1.0);
    request.program = "/nonexistent/fake-native-command".into();
    let error = NativeProcessRunner.run(&request).unwrap_err();
    assert_eq!(error.code, None);
    assert!(error.stdout.is_empty());
}
