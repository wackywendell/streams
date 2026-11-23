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
fn target_rejects_changed_branch_and_any_git_remote() {
    let (git, pending) = scripted(vec![reply(
        &["symbolic-ref", "--quiet", "HEAD"],
        b"refs/heads/other\n",
    )]);
    assert!(
        git.check_target("main")
            .unwrap_err()
            .to_string()
            .contains("branch was changed")
    );
    assert!(pending.lock().unwrap().is_empty());
    for (remote, valid) in [("", true), ("origin\n", false), ("upstream\n", false)] {
        let (git, pending) = scripted(vec![
            reply(&["symbolic-ref", "--quiet", "HEAD"], b"refs/heads/main\n"),
            reply(&["remote"], remote.as_bytes()),
        ]);
        assert_eq!(git.check_target("main").is_ok(), valid);
        assert!(pending.lock().unwrap().is_empty());
    }
}

#[test]
fn effective_remote_uses_offline_ls_remote_get_url() {
    let (git, pending) = scripted(vec![reply(
        &[
            "ls-remote",
            "--get-url",
            "--",
            "git@github.com:org/repo.git",
        ],
        b"git@alias.github.com:org/repo.git\n",
    )]);
    assert_eq!(
        git.effective_remote("git@github.com:org/repo.git").unwrap(),
        "git@alias.github.com:org/repo.git"
    );
    assert!(pending.lock().unwrap().is_empty());
}

#[test]
fn blob_observer_batches_distinct_objects_and_caches_exact_bytes() {
    let oid = "a".repeat(40);
    let second = "b".repeat(40);
    let tree = [
        format!("100644 blob {oid}\tstreams/one.json\0"),
        format!("100644 blob {oid}\tstreams/copy.json\0"),
        format!("100644 blob {second}\tstreams/two.json\0"),
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
        assert_eq!(files["streams/one.json"], b"a\0b\n");
        assert_eq!(files["streams/copy.json"], b"a\0b\n");
        assert_eq!(files["streams/two.json"], b"next\n");
    }
    assert!(pending.lock().unwrap().is_empty());
}
