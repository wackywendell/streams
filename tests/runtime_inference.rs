//! Directory inspection (Git/jj), document path normalization, and local document lookup.

use std::{collections::VecDeque, fs, os::unix::fs::symlink, sync::Mutex};

use streams::{
    application::{
        ApplicationError,
        documents::{
            DocumentLocation, VcsOutput, VcsRunner, inspect_directory_with_runner, locate,
            normalize_document_key,
        },
    },
    model::{RepositoryInfo, Session, SessionDirectory, Title},
};

struct FakeVcs {
    replies: Mutex<VecDeque<Option<VcsOutput>>>,
    calls: Mutex<Vec<(String, Vec<String>)>>,
}

impl FakeVcs {
    fn new(replies: Vec<Option<VcsOutput>>) -> Self {
        Self {
            replies: Mutex::new(replies.into()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn ok(stdout: &str) -> Option<VcsOutput> {
        Some(VcsOutput {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        })
    }

    fn err(status: i32, stderr: &str) -> Option<VcsOutput> {
        Some(VcsOutput {
            status,
            stdout: String::new(),
            stderr: stderr.into(),
        })
    }
}

impl VcsRunner for FakeVcs {
    fn run(&self, program: &str, args: &[&str]) -> Result<Option<VcsOutput>, ApplicationError> {
        self.calls.lock().unwrap().push((
            program.into(),
            args.iter().map(|s| (*s).to_owned()).collect(),
        ));
        Ok(self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected VCS invocation"))
    }
}

#[test]
fn inspect_directory_handles_git_clone_subdir_detached_and_no_remote() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = fs::canonicalize(temp.path()).unwrap().join("repo");
    let sub = repo_root.join("crates/app");
    fs::create_dir_all(&sub).unwrap();

    // Git clone with origin remote and active branch in subdirectory.
    let vcs = FakeVcs::new(vec![
        None, // jj unavailable -> fall through to git
        FakeVcs::ok(&format!("{}\n", repo_root.display())),
        FakeVcs::ok("git@github.com:org/repo.git\n"),
        FakeVcs::ok("feature-x\n"),
    ]);
    let dir = inspect_directory_with_runner(&sub, &vcs).unwrap();
    assert_eq!(dir.path, sub.to_str().unwrap());
    assert_eq!(
        dir.repository,
        Some(RepositoryInfo {
            url: "git@github.com:org/repo.git".into(),
            within: Some("crates/app".into()),
            branch: Some("feature-x".into()),
        })
    );

    // Detached HEAD gives branch: None.
    let vcs = FakeVcs::new(vec![
        None,
        FakeVcs::ok(&format!("{}\n", repo_root.display())),
        FakeVcs::ok("git@github.com:org/repo.git\n"),
        FakeVcs::err(1, ""),
    ]);
    let dir = inspect_directory_with_runner(&repo_root, &vcs).unwrap();
    assert_eq!(dir.repository.as_ref().unwrap().within, None);
    assert_eq!(dir.repository.as_ref().unwrap().branch, None);

    // Clone with no origin remote gives repository: None.
    let vcs = FakeVcs::new(vec![
        None,
        FakeVcs::ok(&format!("{}\n", repo_root.display())),
        FakeVcs::err(1, ""),
    ]);
    let dir = inspect_directory_with_runner(&repo_root, &vcs).unwrap();
    assert_eq!(dir.repository, None);

    // Plain directory outside any repo gives repository: None.
    let vcs = FakeVcs::new(vec![
        FakeVcs::err(1, "Error: There is no jj repo in \".\""),
        FakeVcs::err(128, "fatal: not a git repository"),
    ]);
    let dir = inspect_directory_with_runner(&repo_root, &vcs).unwrap();
    assert_eq!(dir.repository, None);

    // Unexpected VCS error fails inspection rather than silently dropping repository.
    let vcs = FakeVcs::new(vec![FakeVcs::err(2, "permission denied")]);
    assert!(inspect_directory_with_runner(&repo_root, &vcs).is_err());
}

#[test]
fn inspect_directory_handles_jj_single_bookmark_multiple_bookmarks_and_no_remote() {
    let temp = tempfile::tempdir().unwrap();
    let repo_root = fs::canonicalize(temp.path()).unwrap().join("jj-repo");
    fs::create_dir_all(&repo_root).unwrap();

    // Single local bookmark on nearest ancestor -> Some(bookmark).
    let vcs = FakeVcs::new(vec![
        FakeVcs::ok(&format!("{}\n", repo_root.display())),
        FakeVcs::ok("upstream git@example.com:u/r.git\norigin git@example.com:o/r.git\n"),
        FakeVcs::ok("cli-by-entity\n"),
    ]);
    let dir = inspect_directory_with_runner(&repo_root, &vcs).unwrap();
    assert_eq!(
        dir.repository,
        Some(RepositoryInfo {
            url: "git@example.com:o/r.git".into(),
            within: None,
            branch: Some("cli-by-entity".into()),
        })
    );

    // Multiple local bookmarks on nearest ancestor -> branch: None.
    let vcs = FakeVcs::new(vec![
        FakeVcs::ok(&format!("{}\n", repo_root.display())),
        FakeVcs::ok("origin git@example.com:o/r.git\n"),
        FakeVcs::ok("one\ntwo\n"),
    ]);
    let dir = inspect_directory_with_runner(&repo_root, &vcs).unwrap();
    assert_eq!(dir.repository.as_ref().unwrap().branch, None);

    // jj workspace with no git remote -> repository: None.
    let vcs = FakeVcs::new(vec![
        FakeVcs::ok(&format!("{}\n", repo_root.display())),
        FakeVcs::ok(""),
    ]);
    let dir = inspect_directory_with_runner(&repo_root, &vcs).unwrap();
    assert_eq!(dir.repository, None);
}

#[test]
fn locate_enforces_host_directory_containment_and_regular_files() {
    let temp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(temp.path()).unwrap();
    let work = base.join("work");
    let outside = base.join("outside");
    fs::create_dir_all(work.join("docs")).unwrap();
    fs::create_dir_all(&outside).unwrap();
    let doc = work.join("docs/plan.md");
    fs::write(&doc, "plan").unwrap();
    let secret = outside.join("secret.md");
    fs::write(&secret, "secret").unwrap();
    symlink(&secret, work.join("escape.md")).unwrap();
    symlink(&doc, work.join("link.md")).unwrap();

    let mut session = Session::new("host-a", "pi", "s1", "st1", Title::new("S", false)).unwrap();
    assert_eq!(
        locate("host-a", &session, "docs/plan.md").unwrap(),
        DocumentLocation::Unavailable {
            reason: "no directory here".into()
        }
    );

    session.directory = Some(SessionDirectory {
        path: work.to_str().unwrap().into(),
        repository: None,
    });

    // Different host -> no directory here.
    assert_eq!(
        locate("host-b", &session, "docs/plan.md").unwrap(),
        DocumentLocation::Unavailable {
            reason: "no directory here".into()
        }
    );

    // Local regular file and intra-directory symlink resolve.
    assert_eq!(
        locate("host-a", &session, "docs/plan.md").unwrap(),
        DocumentLocation::Path { path: doc.clone() }
    );
    assert_eq!(
        locate("host-a", &session, "link.md").unwrap(),
        DocumentLocation::Path { path: doc }
    );

    // Missing file, directory-not-file, and symlink escape.
    assert_eq!(
        locate("host-a", &session, "docs/missing.md").unwrap(),
        DocumentLocation::Unavailable {
            reason: "missing".into()
        }
    );
    assert_eq!(
        locate("host-a", &session, "docs").unwrap(),
        DocumentLocation::Unavailable {
            reason: "not a file".into()
        }
    );
    assert_eq!(
        locate("host-a", &session, "escape.md").unwrap(),
        DocumentLocation::Unavailable {
            reason: "outside the Session's directory".into()
        }
    );

    // Invalid path keys fail validation.
    assert!(locate("host-a", &session, "../secret.md").is_err());
    assert!(locate("host-a", &session, "/abs/path.md").is_err());
}

#[test]
fn normalize_document_key_handles_relative_absolute_symlinks_and_rejects_escapes() {
    let temp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(temp.path()).unwrap();
    let real_work = base.join("real-work");
    fs::create_dir_all(real_work.join("docs")).unwrap();
    fs::write(real_work.join("docs/real.md"), "ok").unwrap();
    symlink(real_work.join("docs/real.md"), real_work.join("link.md")).unwrap();
    let alias_work = base.join("alias-work");
    symlink(&real_work, &alias_work).unwrap();

    let dir = SessionDirectory {
        path: real_work.to_str().unwrap().into(),
        repository: None,
    };

    assert_eq!(
        normalize_document_key(Some(&dir), "docs/real.md").unwrap(),
        "docs/real.md"
    );
    assert_eq!(
        normalize_document_key(
            Some(&dir),
            real_work.join("docs/future.md").to_str().unwrap()
        )
        .unwrap(),
        "docs/future.md"
    );
    // Parent symlink resolves while keeping symlinked leaf name unchanged.
    assert_eq!(
        normalize_document_key(Some(&dir), alias_work.join("link.md").to_str().unwrap()).unwrap(),
        "link.md"
    );
    // Whole-component prefix check: `/real-work-other` is not inside `/real-work`.
    let sibling = base.join("real-work-other");
    fs::create_dir_all(&sibling).unwrap();
    assert!(
        normalize_document_key(Some(&dir), sibling.join("a.md").to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains(&dir.path)
    );
    // `..` is refused before any normalization.
    assert!(
        normalize_document_key(
            Some(&dir),
            real_work.join("docs/../real.md").to_str().unwrap()
        )
        .is_err()
    );
    assert!(normalize_document_key(Some(&dir), "../outside.md").is_err());
}
