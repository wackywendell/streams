//! Git command construction and parsing without executing Git.
use super::*;
use std::collections::VecDeque;

struct Expected {
    args: Vec<String>,
    input: Option<Vec<u8>>,
    result: Result<Vec<u8>, GitError>,
}

fn reply(args: &[&str], output: &[u8]) -> Expected {
    Expected {
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        input: None,
        result: Ok(output.to_vec()),
    }
}

fn scripted(steps: Vec<Expected>) -> (Git, Arc<Mutex<VecDeque<Expected>>>) {
    let remaining: Arc<Mutex<VecDeque<Expected>>> = Arc::new(Mutex::new(steps.into()));
    let pending = remaining.clone();
    let git = Git::new("unused-no-io-root", 1.0).with_hook(Arc::new(Mutex::new(
        move |_: &Git, args: &[&str], input: Option<&[u8]>, index: Option<&Path>| {
            let step = pending
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected Git command");
            assert_eq!(args, step.args);
            assert_eq!(input, step.input.as_deref());
            assert!(
                index.is_none(),
                "must not redirect a read or push through an index"
            );
            Some(step.result)
        },
    )));
    (git, remaining)
}

#[test]
fn ancestry_compares_merge_base_and_preserves_failures() {
    let before = "a".repeat(40);
    let after = "b".repeat(40);
    for (output, expected) in [
        (format!("{before}\n"), true),
        ("other-base\n".into(), false),
        (String::new(), false),
    ] {
        let (git, pending) = scripted(vec![reply(
            &["merge-base", &before, &after],
            output.as_bytes(),
        )]);
        assert_eq!(git.ancestor(&before, &after).unwrap(), expected);
        assert!(pending.lock().unwrap().is_empty());
    }
    let error = GitError::new("merge-base failed", Some(2));
    let (git, pending) = scripted(vec![Expected {
        result: Err(error.clone()),
        ..reply(&["merge-base", &before, &after], b"")
    }]);
    assert_eq!(git.ancestor(&before, &after).unwrap_err(), error);
    assert!(pending.lock().unwrap().is_empty());
}

#[test]
fn target_checks_both_urls_and_rejects_mirror_pushes() {
    for (fetch, push, mirror, valid) in [
        ("expected", "expected", "", true),
        ("expected", "expected", "false", true),
        ("other", "expected", "", false),
        ("expected", "other", "", false),
        ("expected", "expected", "true", false),
        ("expected\nother", "expected", "", false),
    ] {
        let (git, pending) = scripted(vec![
            reply(&["symbolic-ref", "--quiet", "HEAD"], b"refs/heads/main\n"),
            reply(&["remote", "get-url", "--all", "origin"], fetch.as_bytes()),
            reply(
                &["remote", "get-url", "--push", "--all", "origin"],
                push.as_bytes(),
            ),
            reply(
                &["config", "--get", "remote.origin.mirror"],
                mirror.as_bytes(),
            ),
        ]);
        assert_eq!(git.check_target(Some("expected"), "main").is_ok(), valid);
        assert!(pending.lock().unwrap().is_empty());
    }
}

#[test]
fn target_rejects_changed_branch_and_unexpected_local_remote() {
    let (git, pending) = scripted(vec![reply(
        &["symbolic-ref", "--quiet", "HEAD"],
        b"refs/heads/other\n",
    )]);
    assert!(
        git.check_target(None, "main")
            .unwrap_err()
            .to_string()
            .contains("branch was changed")
    );
    assert!(pending.lock().unwrap().is_empty());
    for (remote, valid) in [("", true), ("origin\n", false)] {
        let (git, pending) = scripted(vec![
            reply(&["symbolic-ref", "--quiet", "HEAD"], b"refs/heads/main\n"),
            reply(&["remote"], remote.as_bytes()),
        ]);
        assert_eq!(git.check_target(None, "main").is_ok(), valid);
        assert!(pending.lock().unwrap().is_empty());
    }
}

#[test]
fn commit_reads_signing_policy_each_time_and_never_falls_back_unsigned() {
    let tree = "a".repeat(40);
    let parent = "b".repeat(40);
    let error = GitError::new("signer failed", Some(1));
    let mut signed = reply(&["commit-tree", &tree, "-p", &parent, "-S"], b"");
    signed.input = Some(b"original message\n".to_vec());
    signed.result = Err(error.clone());
    let mut unsigned = reply(&["commit-tree", &tree], b"created\n");
    unsigned.input = Some(b"second message\n".to_vec());
    let (git, pending) = scripted(vec![
        reply(&["config", "--bool", "--get", "commit.gpgsign"], b"true\n"),
        signed,
        reply(&["config", "--bool", "--get", "commit.gpgsign"], b"false\n"),
        unsigned,
    ]);
    assert_eq!(
        git.commit(&tree, "original message\n", Some(&parent))
            .unwrap_err(),
        error
    );
    assert_eq!(
        pending.lock().unwrap().len(),
        2,
        "no unsigned retry after signer failure"
    );
    assert_eq!(
        git.commit(&tree, "second message\n", None).unwrap(),
        "created"
    );
    assert!(pending.lock().unwrap().is_empty());
}

#[test]
fn blob_observer_batches_distinct_objects_and_caches_exact_bytes() {
    let oid = "a".repeat(40);
    let second = "b".repeat(40);
    let tree = [
        format!("100644 blob {oid}\tstreams/one.yaml\0"),
        format!("100644 blob {oid}\tstreams/copy.yaml\0"),
        format!("100644 blob {second}\tstreams/two.yaml\0"),
    ]
    .concat();
    let mut batch = format!("{oid} blob 4\n").into_bytes();
    batch.extend_from_slice(b"a\0b\n\n");
    batch.extend_from_slice(format!("{second} blob 5\nnext\n\n").as_bytes());
    let mut blobs = reply(&["cat-file", "--batch"], &batch);
    blobs.input = Some(format!("{oid}\n{second}\n").into_bytes());
    let (git, pending) = scripted(vec![
        reply(&["ls-tree", "-rz", "--full-tree", "first"], tree.as_bytes()),
        blobs,
        reply(
            &["ls-tree", "-rz", "--full-tree", "second"],
            tree.as_bytes(),
        ),
    ]);
    for revision in ["first", "second"] {
        let files = git.files(revision).unwrap();
        assert_eq!(files["streams/one.yaml"], b"a\0b\n");
        assert_eq!(files["streams/copy.yaml"], b"a\0b\n");
        assert_eq!(files["streams/two.yaml"], b"next\n");
    }
    assert!(pending.lock().unwrap().is_empty());
}
