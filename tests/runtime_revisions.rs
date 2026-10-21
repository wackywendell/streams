use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::Mutex;

use streams_temper::process::{
    NativeProcessRunner, ProcessError, ProcessOutput, ProcessRequest, ProcessRunner,
};
use streams_temper::runtime::checkouts::canonical_directory;
use streams_temper::runtime::{CheckoutRegistry, RepositoryInspection, RuntimeError};
use tempfile::TempDir;

const CID: &str = "1234567890abcdef1234567890abcdef12345678";

fn inspect(path: &Path) -> Result<Option<RepositoryInspection>, RuntimeError> {
    Ok(Some(RepositoryInspection {
        root: canonical_directory(path)?,
        relative_cwd: ".".into(),
    }))
}

struct Fixture {
    _temp: TempDir,
    repo: PathBuf,
    cache: PathBuf,
    registry: CheckoutRegistry,
    id: String,
}

impl Fixture {
    fn new(jj: bool) -> Self {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(repo.join(if jj { ".jj" } else { ".git" })).unwrap();
        let registry = CheckoutRegistry::new(temp.path().join("state"), "test")
            .unwrap()
            .with_inspector(inspect);
        let id = registry.register(&repo, None).unwrap().id;
        let cache = canonical_directory(temp.path()).unwrap().join("cache");
        Self {
            _temp: temp,
            repo: canonical_directory(&repo).unwrap(),
            cache,
            registry,
            id,
        }
    }

    fn extract(
        &self,
        path: &str,
        revision: &str,
        runner: &dyn ProcessRunner,
    ) -> Result<streams_temper::runtime::revisions::RevisionDocument, RuntimeError> {
        self.registry
            .revision_document(&self.id, path, revision, runner, 3.0, &self.cache)
    }

    fn revision_root(&self) -> PathBuf {
        self.cache.join(&self.id).join(&CID[..12])
    }
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<ProcessRequest>>,
    outputs: Mutex<VecDeque<Result<String, String>>>,
}

impl Fake {
    fn new(outputs: &[Result<&str, &str>]) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            outputs: Mutex::new(
                outputs
                    .iter()
                    .map(|s| (*s).map(str::to_owned).map_err(str::to_owned))
                    .collect(),
            ),
        }
    }
    fn args(&self) -> Vec<Vec<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.args.iter().map(|s| s.to_str().unwrap().into()).collect())
            .collect()
    }
}

impl ProcessRunner for Fake {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessOutput, ProcessError> {
        self.calls.lock().unwrap().push(request.clone());
        match self
            .outputs
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected VCS dispatch")
        {
            Ok(output) => Ok(ProcessOutput {
                status: ExitStatus::from_raw(0),
                stdout: output.into_bytes(),
                stderr: Vec::new(),
            }),
            Err(message) => Err(ProcessError {
                message,
                code: Some(1),
                stdout: Vec::new(),
                stderr: Vec::new(),
            }),
        }
    }
}

fn private_dir(path: &Path) {
    fs::create_dir_all(path).unwrap();
    let mut path = Some(path);
    while let Some(current) = path {
        if current.file_name().is_some_and(|s| s == "cache") {
            fs::set_permissions(current, fs::Permissions::from_mode(0o700)).unwrap();
            break;
        }
        fs::set_permissions(current, fs::Permissions::from_mode(0o700)).unwrap();
        path = current.parent();
    }
}

#[test]
fn jj_first_exact_observational_requests_and_cached_immutable() {
    let f = Fixture::new(true);
    let runner = Fake::new(&[Ok(CID), Ok("a\r\nb\rc\n\n")]);
    let target = f.extract("docs/a.md", " main ", &runner).unwrap();
    assert_eq!(target.revision, " main ");
    assert_eq!(target.checkout_id, f.id);
    assert_eq!(target.root, f.revision_root());
    assert_eq!(fs::read_to_string(&target.file).unwrap(), "a\nb\nc\n");
    assert_eq!(serde_json::to_value(&target).unwrap()["revision"], " main ");
    assert_eq!(
        runner.args(),
        vec![
            vec![
                "--ignore-working-copy",
                "--at-operation=@",
                "log",
                "--no-graph",
                "-r",
                "main",
                "-T",
                "commit_id ++ \"\\n\""
            ],
            vec![
                "--ignore-working-copy",
                "--at-operation=@",
                "file",
                "show",
                "-r",
                CID,
                "docs/a.md"
            ],
        ]
    );
    for request in runner.calls.lock().unwrap().iter() {
        assert_eq!(request.program, Path::new("jj"));
        assert_eq!(request.cwd.as_deref(), Some(f.repo.as_path()));
        assert_eq!(
            request
                .env
                .get(std::ffi::OsStr::new("GIT_OPTIONAL_LOCKS"))
                .unwrap(),
            "0"
        );
        assert_eq!(request.allowed, [0]);
        assert_eq!(request.timeout, 3.0);
        assert!(request.input.is_none());
    }
    let no_calls = Fake::default();
    assert_eq!(
        f.extract("docs/a.md", CID, &no_calls).unwrap().file,
        target.file
    );
    assert!(no_calls.args().is_empty());
    assert_eq!(fs::metadata(target.file).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(target.root).unwrap().mode() & 0o777, 0o700);
}

#[test]
fn jj_resolution_and_extraction_fall_back_to_git() {
    let f = Fixture::new(true);
    let runner = Fake::new(&[
        Err("jj log failed"),
        Ok(CID),
        Err("jj show failed"),
        Ok("git text"),
    ]);
    let target = f.extract("a", "bookmark", &runner).unwrap();
    assert_eq!(fs::read_to_string(target.file).unwrap(), "git text\n");
    let args = runner.args();
    assert_eq!(args[1], ["rev-parse", "--verify", "bookmark^{commit}"]);
    assert_eq!(args[3], ["show", &format!("{CID}:a")]);
    assert_eq!(
        runner
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.program.to_str().unwrap())
            .collect::<Vec<_>>(),
        ["jj", "git", "jj", "git"]
    );
}

#[test]
fn mutable_expressions_reresolve_and_unresolved_negatives_are_not_sticky() {
    let f = Fixture::new(false);
    let runner = Fake::new(&[
        Err("unresolved"),
        Err("not yet"),
        Ok(CID),
        Ok("now exists"),
        Ok(CID),
    ]);
    let error = f.extract("a", "future", &runner).unwrap_err();
    assert!(matches!(error, RuntimeError::NotFound(_)));
    assert!(!f.cache.exists());
    assert_eq!(
        fs::read_to_string(f.extract("a", "future", &runner).unwrap().file).unwrap(),
        "now exists\n"
    );
    f.extract("a", "future", &runner).unwrap();
    assert_eq!(runner.args().len(), 5);
}

#[test]
fn unresolved_success_uses_python_sha256_key_and_is_not_reused() {
    let f = Fixture::new(false);
    let runner = Fake::new(&[Err("unresolved"), Ok("one"), Err("unresolved"), Ok("two")]);
    let target = f.extract("a", "HEAD", &runner).unwrap();
    // hashlib.sha256(b"HEAD").hexdigest()[:12]
    assert_eq!(target.root.file_name().unwrap(), "b5180223165a");
    assert_eq!(f.extract("a", "HEAD", &runner).unwrap().file, target.file);
    assert_eq!(fs::read_to_string(target.file).unwrap(), "two\n");
    assert_eq!(runner.args().len(), 4);
}

#[test]
fn invalid_requests_dispatch_nothing_and_create_nothing() {
    let f = Fixture::new(false);
    let runner = Fake::default();
    for path in [".", "..", "../a", "/a", "a/../b", "a//b", "a\\b", "C:a"] {
        assert!(f.extract(path, CID, &runner).is_err(), "{path}");
    }
    for revision in ["", " ", " --help", "a\nb"] {
        assert!(f.extract("a", revision, &runner).is_err());
    }
    assert!(
        f.registry
            .revision_document("unknown", "a", CID, &runner, 3.0, &f.cache)
            .is_err()
    );
    assert!(
        f.registry
            .revision_document(&f.id, "a", CID, &runner, f64::NAN, &f.cache)
            .is_err()
    );
    assert!(runner.args().is_empty());
    assert!(!f.cache.exists());
}

#[test]
fn symlink_cache_root_descendants_and_leaves_are_corruption_not_absence() {
    for location in [
        "root",
        "checkout",
        "revision",
        "intermediate",
        "file",
        "missing",
    ] {
        let f = Fixture::new(false);
        let outside = f._temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let root = f.revision_root();
        let link = match location {
            "root" => f.cache.clone(),
            "checkout" => f.cache.join(&f.id),
            "revision" => root.clone(),
            "intermediate" => root.join("docs"),
            "file" => root.join("docs/a"),
            _ => root.join("docs/.a.missing"),
        };
        if link != f.cache {
            private_dir(link.parent().unwrap());
        }
        symlink(&outside, &link).unwrap();
        let runner = Fake::default();
        let revision = if matches!(location, "root" | "checkout") {
            "main"
        } else {
            CID
        };
        let error = f.extract("docs/a", revision, &runner).unwrap_err();
        assert!(
            !matches!(error, RuntimeError::NotFound(_)),
            "{location}: {error}"
        );
        assert!(runner.args().is_empty());
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }
}

#[test]
fn fifo_and_nonprivate_cache_entries_fail_without_vcs_or_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    for leaf in ["a", ".a.missing"] {
        let f = Fixture::new(false);
        let root = f.revision_root();
        private_dir(&root);
        let fifo = CString::new(root.join(leaf).as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let error = f.extract("a", CID, &Fake::default()).unwrap_err();
        assert!(!matches!(error, RuntimeError::NotFound(_)));
    }
    let f = Fixture::new(false);
    private_dir(&f.revision_root());
    fs::write(f.revision_root().join("a"), "untrusted").unwrap();
    fs::set_permissions(
        f.revision_root().join("a"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    assert!(f.extract("a", CID, &Fake::default()).is_err());
}

#[test]
fn mutable_revision_tracks_changed_commit_and_missing_hints_are_validated() {
    let f = Fixture::new(false);
    let next = "abcdef1234567890abcdef1234567890abcdef12";
    let runner = Fake::new(&[Ok(CID), Err("missing"), Ok(next), Ok("new file")]);
    assert!(matches!(
        f.extract("a", "main", &runner),
        Err(RuntimeError::NotFound(_))
    ));
    let target = f.extract("a", "main", &runner).unwrap();
    assert_eq!(target.root.file_name().unwrap(), &next[..12]);
    assert_eq!(fs::read_to_string(target.file).unwrap(), "new file\n");
    let missing = f.revision_root().join(".a.missing");
    fs::write(missing, "corrupt").unwrap();
    assert!(matches!(
        f.extract("a", CID, &Fake::default()),
        Err(RuntimeError::Invalid(_))
    ));
}

#[test]
fn contradictory_missing_hints_and_unresolved_hints_are_corruption() {
    use sha2::{Digest, Sha256};
    for revision in [CID, "unresolved"] {
        let f = Fixture::new(false);
        let root = if revision == CID {
            f.revision_root()
        } else {
            f.cache
                .join(&f.id)
                .join(&format!("{:x}", Sha256::digest(revision.as_bytes()))[..12])
        };
        private_dir(&root);
        let missing = root.join(".a.missing");
        fs::write(&missing, b"").unwrap();
        fs::set_permissions(&missing, fs::Permissions::from_mode(0o600)).unwrap();
        let runner = if revision == CID {
            let file = root.join("a");
            fs::write(&file, b"cached\n").unwrap();
            fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
            Fake::default()
        } else {
            Fake::new(&[Err("unresolved")])
        };
        assert!(matches!(
            f.extract("a", revision, &runner),
            Err(RuntimeError::Invalid(_))
        ));
    }
}

#[test]
fn aliases_above_cache_boundary_are_allowed() {
    let f = Fixture::new(false);
    let alias = f._temp.path().join("alias");
    symlink(f._temp.path(), &alias).unwrap();
    let target = f
        .registry
        .revision_document(
            &f.id,
            "a",
            CID,
            &Fake::new(&[Ok("text")]),
            3.0,
            &alias.join("cache"),
        )
        .unwrap();
    assert_eq!(target.root, f.revision_root());
}

fn git(repo: &Path, args: &[&str]) -> String {
    let environment: Vec<_> = std::env::vars_os()
        .filter(|(key, _)| !key.to_string_lossy().starts_with("GIT_"))
        .collect();
    let output = Command::new("git")
        .env_clear()
        .envs(environment)
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
        ])
        .args(args)
        .current_dir(repo)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().into()
}

#[test]
fn real_git_historical_and_missing_cache_preserve_worktree() {
    let f = Fixture::new(false);
    fs::remove_dir(f.repo.join(".git")).unwrap();
    git(&f.repo, &["init", "--initial-branch=main"]);
    fs::write(f.repo.join("doc"), "old\n\n").unwrap();
    git(&f.repo, &["add", "doc"]);
    git(&f.repo, &["commit", "-m", "initial"]);
    let revision = git(&f.repo, &["rev-parse", "HEAD"]);
    fs::write(f.repo.join("doc"), "uncommitted\n").unwrap();
    let before = git(&f.repo, &["status", "--porcelain"]);
    let target = f.extract("doc", &revision, &NativeProcessRunner).unwrap();
    assert_eq!(fs::read_to_string(&target.file).unwrap(), "old\n");
    assert_eq!(
        f.extract("doc", &revision, &Fake::default()).unwrap(),
        target
    );
    let error = f
        .extract("absent", &revision, &NativeProcessRunner)
        .unwrap_err();
    assert!(matches!(error, RuntimeError::NotFound(_)));
    assert!(error.to_string().contains("git"));
    assert_eq!(fs::read(target.root.join(".absent.missing")).unwrap(), b"");
    assert!(matches!(
        f.extract("absent", &revision, &Fake::default()),
        Err(RuntimeError::NotFound(_))
    ));
    assert_eq!(
        fs::read_to_string(f.repo.join("doc")).unwrap(),
        "uncommitted\n"
    );
    assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), revision);
    assert_eq!(git(&f.repo, &["status", "--porcelain"]), before);
}
