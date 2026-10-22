//! Git subprocess deadlines must cover pipe traffic and surviving descendants.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use streams_temper::sync::git_io::{Git, GitError};
use tempfile::TempDir;

fn fixture(timeout: f64) -> (TempDir, Git) {
    let root = tempfile::tempdir().unwrap();
    let git = Git::new(root.path(), timeout);
    (root, git)
}

fn shell(git: &Git, script: &str, input: Option<&[u8]>) -> Result<Vec<u8>, GitError> {
    let alias = format!("alias.deadline-test=!{script}");
    git.run_raw(&["-c", &alias, "deadline-test"], input, None, &[0])
}

fn assert_timeout(start: Instant, error: GitError) {
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "deadline was not bounded"
    );
    assert!(error.uncertain);
    assert_eq!(error.code, None);
    assert!(error.message.contains("timeout"), "{error}");
}

#[test]
fn drains_both_outputs_while_feeding_large_input() {
    let (_root, git) = fixture(5.0);
    let input = vec![b'x'; 1024 * 1024];
    // Fill both output pipes before reading stdin, which is itself larger than a pipe.
    let output = shell(
        &git,
        "dd if=/dev/zero bs=65536 count=4 2>/dev/null; dd if=/dev/zero bs=65536 count=4 >&2 2>/dev/null; cat",
        Some(&input),
    )
    .unwrap();
    assert_eq!(&output[..262144], vec![0; 262144]);
    assert_eq!(&output[262144..], input);
}

#[test]
fn deadline_includes_blocked_stdin() {
    let (_root, git) = fixture(0.15);
    let input = vec![b'x'; 4 * 1024 * 1024];
    let start = Instant::now();
    assert_timeout(start, shell(&git, "sleep 30", Some(&input)).unwrap_err());
}

#[test]
fn deadline_includes_pipes_inherited_after_git_exits() {
    let (_root, git) = fixture(0.15);
    let start = Instant::now();
    // The alias and Git can exit successfully while the background process owns the pipes.
    assert_timeout(start, shell(&git, "sleep 30 & exit 0", None).unwrap_err());
}

#[test]
fn preserves_exit_code_and_stderr() {
    let (_root, git) = fixture(5.0);
    let error = shell(&git, "printf specific-failure >&2; exit 7", None).unwrap_err();
    assert_eq!(error.code, Some(7));
    assert!(!error.uncertain);
    assert!(error.message.contains("specific-failure"));
    assert!(
        git.run_raw(&["config", "--get", "streams.missing"], None, None, &[0, 1])
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        git.run_raw(&["rev-parse", "--verify", "HEAD"], None, None, &[128])
            .unwrap(),
        b""
    );
}

#[test]
fn reports_signal_without_inventing_exit_code() {
    let (_root, git) = fixture(5.0);
    // Signal Git itself, rather than the alias shell (whose status Git would translate).
    let error = shell(&git, "kill -KILL \"$PPID\"", None).unwrap_err();
    assert_eq!(error.code, None);
    assert!(error.uncertain);
}

#[test]
fn rejects_empty_arguments_and_invalid_timeouts() {
    let root = tempfile::tempdir().unwrap();
    let git = Git::new(root.path(), 1.0);
    assert!(
        git.run_raw(&[], None, None, &[0])
            .unwrap_err()
            .message
            .contains("argument")
    );
    assert!(git.run(&[], None, None, &[0]).is_err());
    for timeout in [
        0.0,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MAX,
    ] {
        let git = Git::new(root.path(), timeout);
        assert!(
            git.run_raw(&["--version"], None, None, &[0])
                .unwrap_err()
                .message
                .contains("timeout")
        );
        assert!(git.run(&["--version"], None, None, &[0]).is_err());
    }
}

#[test]
fn hook_can_call_raw_execution_without_recursion() {
    let (_root, git) = fixture(5.0);
    let git = git.with_hook(Arc::new(Mutex::new(
        |git: &Git, args: &[&str], input: Option<&[u8]>, index: Option<&Path>| {
            Some(git.run_raw(args, input, index, &[0]))
        },
    )));
    assert!(
        git.run(&["--version"], None, None, &[0])
            .unwrap()
            .starts_with(b"git version")
    );
}
