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
const NEXT: &str = "1234567890abcdef1234567890abcdef87654321";

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
        self.cache.join("v2").join(&self.id).join(CID)
    }
}

#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<ProcessRequest>>,
    outputs: Mutex<VecDeque<Result<Vec<u8>, String>>>,
}

impl Fake {
    fn new(outputs: &[Result<&str, &str>]) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            outputs: Mutex::new(
                outputs
                    .iter()
                    .map(|s| (*s).map(|s| s.as_bytes().to_vec()).map_err(str::to_owned))
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
            Ok(stdout) => Ok(ProcessOutput {
                status: ExitStatus::from_raw(0),
                stdout,
                stderr: Vec::new(),
            }),
            Err(message) => Err(ProcessError {
                message,
                code: None,
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
        fs::set_permissions(current, fs::Permissions::from_mode(0o700)).unwrap();
        if current.file_name().is_some_and(|s| s == "cache") {
            break;
        }
        path = current.parent();
    }
}

#[test]
fn jj_requests_are_observational_resolve_once_and_reuse_only_resolved_content() {
    let f = Fixture::new(true);
    let runner = Fake::new(&[Ok(CID), Ok("file\0"), Ok("a\r\nb\rc\n\n"), Ok(CID)]);
    let target = f.extract("docs/a.md", " main ", &runner).unwrap();
    assert_eq!(target.revision, " main ");
    assert_eq!(target.checkout_id, f.id);
    assert_eq!(target.root, f.revision_root());
    assert_eq!(fs::read(&target.file).unwrap(), b"a\r\nb\rc\n\n");
    assert_eq!(serde_json::to_value(&target).unwrap()["revision"], " main ");
    let args = runner.args();
    let prefix = [
        "--ignore-working-copy",
        "--at-operation=@",
        "--color=never",
        "--no-pager",
    ];
    for args in &args {
        assert_eq!(&args[..4], prefix);
    }
    assert_eq!(
        &args[0][4..],
        [
            "log",
            "--no-graph",
            "-r",
            "main",
            "-T",
            "commit_id ++ \"\\n\""
        ]
    );
    assert_eq!(
        &args[1][4..],
        [
            "file",
            "list",
            "-r",
            CID,
            "-T",
            "file_type ++ \"\\0\"",
            "--",
            "root-file:\"docs/a.md\""
        ]
    );
    assert_eq!(
        &args[2][4..],
        [
            "file",
            "show",
            "-r",
            CID,
            "-T",
            "\"\"",
            "--",
            "root-file:\"docs/a.md\""
        ]
    );
    assert_eq!(
        f.extract("docs/a.md", CID, &runner).unwrap().file,
        target.file
    );
    assert_eq!(runner.args().len(), 4);
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
    assert_eq!(fs::metadata(target.file).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::metadata(target.root).unwrap().mode() & 0o777, 0o700);
}

#[test]
fn jj_failures_never_dispatch_git_or_persist_absence() {
    for outputs in [
        vec![Err("jj log failed")],
        vec![Ok(CID), Err("jj list failed")],
        vec![Ok(CID), Ok("file\0"), Err("jj show failed")],
    ] {
        let f = Fixture::new(true);
        let runner = Fake::new(&outputs);
        assert!(matches!(
            f.extract("a", "bookmark", &runner),
            Err(RuntimeError::Inspection(_))
        ));
        assert!(!f.cache.exists());
        assert!(
            runner
                .calls
                .lock()
                .unwrap()
                .iter()
                .all(|r| r.program == Path::new("jj"))
        );
    }
}

#[test]
fn timeout_followed_by_success_is_not_cached_as_absence() {
    for jj in [false, true] {
        let f = Fixture::new(jj);
        let kind = if jj { "file\0" } else { "blob\0" };
        let runner = Fake::new(&[
            Ok(CID),
            Ok(kind),
            Err("configured native command timeout elapsed"),
            Ok(CID),
            Ok(kind),
            Ok("now exists"),
        ]);
        assert!(matches!(
            f.extract("a", CID, &runner),
            Err(RuntimeError::Inspection(_))
        ));
        assert!(!f.cache.exists());
        let target = f.extract("a", CID, &runner).unwrap();
        assert_eq!(fs::read(target.file).unwrap(), b"now exists");
        assert!(!target.root.join(".a.missing").exists());
    }
}

#[test]
fn resolution_requires_one_full_commit_even_for_hexadecimal_expressions() {
    for output in ["", &CID[..12], "not-a-commit", &format!("{CID}\n{NEXT}\n")] {
        let f = Fixture::new(true);
        let runner = Fake::new(&[Ok(output)]);
        assert!(matches!(
            f.extract("a", "all()", &runner),
            Err(RuntimeError::Invalid(_))
        ));
        assert_eq!(runner.args().len(), 1);
        assert!(!f.cache.exists());
    }
    for revision in [CID, &CID[..12], "abcdefabcdef", "HEAD"] {
        let f = Fixture::new(false);
        let runner = Fake::new(&[Err("cannot resolve")]);
        assert!(matches!(
            f.extract("a", revision, &runner),
            Err(RuntimeError::Inspection(_))
        ));
        assert_eq!(
            runner.args(),
            [vec![
                "rev-parse",
                "--verify",
                &format!("{revision}^{{commit}}")
            ]]
        );
        assert!(!f.cache.exists());
    }
}

#[test]
fn full_commit_ids_with_the_same_prefix_and_mutable_expressions_stay_distinct() {
    let f = Fixture::new(false);
    assert_eq!(&CID[..12], &NEXT[..12]);
    let runner = Fake::new(&[
        Ok(CID),
        Ok("blob\0"),
        Ok("one"),
        Ok(NEXT),
        Ok("blob\0"),
        Ok("two"),
        Ok(CID),
    ]);
    let first = f.extract("a", "main", &runner).unwrap();
    let second = f.extract("a", "main", &runner).unwrap();
    assert_ne!(first.root, second.root);
    assert_eq!(second.root.file_name().unwrap(), NEXT);
    assert_eq!(f.extract("a", "main", &runner).unwrap(), first);
    assert_eq!(fs::read(first.file).unwrap(), b"one");
    assert_eq!(fs::read(second.file).unwrap(), b"two");
}

#[test]
fn proven_missing_paths_are_distinct_from_failures_and_are_not_cached() {
    for jj in [false, true] {
        let f = Fixture::new(jj);
        let runner = Fake::new(&[Ok(CID), Ok(""), Ok(CID), Err("listing failed")]);
        assert!(matches!(
            f.extract("absent", CID, &runner),
            Err(RuntimeError::NotFound(_))
        ));
        assert!(matches!(
            f.extract("absent", CID, &runner),
            Err(RuntimeError::Inspection(_))
        ));
        assert!(!f.cache.exists());
    }
}

#[test]
fn old_cache_bytes_and_missing_markers_are_not_inputs() {
    let f = Fixture::new(false);
    let old = f.cache.join(&f.id).join(&CID[..12]);
    private_dir(&old);
    fs::write(old.join("a"), b"transformed\n").unwrap();
    fs::write(old.join(".a.missing"), b"").unwrap();
    let runner = Fake::new(&[Ok(CID), Ok("blob\0"), Ok("exact\r\n\n")]);
    let target = f.extract("a", CID, &runner).unwrap();
    assert_eq!(target.root, f.revision_root());
    assert_eq!(fs::read(target.file).unwrap(), b"exact\r\n\n");
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
        "version",
        "checkout",
        "revision",
        "intermediate",
        "file",
    ] {
        let f = Fixture::new(false);
        let outside = f._temp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let root = f.revision_root();
        let link = match location {
            "root" => f.cache.clone(),
            "version" => f.cache.join("v2"),
            "checkout" => f.cache.join("v2").join(&f.id),
            "revision" => root.clone(),
            "intermediate" => root.join("docs"),
            _ => root.join("docs/a"),
        };
        if link != f.cache {
            private_dir(link.parent().unwrap());
        }
        symlink(&outside, &link).unwrap();
        let needs_resolution = matches!(location, "revision" | "intermediate" | "file");
        let runner = Fake::new(if needs_resolution { &[Ok(CID)] } else { &[] });
        let error = f.extract("docs/a", CID, &runner).unwrap_err();
        assert!(
            !matches!(error, RuntimeError::NotFound(_)),
            "{location}: {error}"
        );
        assert_eq!(runner.args().len(), usize::from(needs_resolution));
        assert!(fs::read_dir(outside).unwrap().next().is_none());
    }
}

#[test]
fn fifo_and_nonprivate_cache_entries_fail_without_extraction_or_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new(false);
    let root = f.revision_root();
    private_dir(&root);
    let fifo = CString::new(root.join("a").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    let runner = Fake::new(&[Ok(CID)]);
    assert!(matches!(
        f.extract("a", CID, &runner),
        Err(RuntimeError::Invalid(_))
    ));
    assert_eq!(runner.args().len(), 1);

    let f = Fixture::new(false);
    private_dir(&f.revision_root());
    fs::write(f.revision_root().join("a"), "untrusted").unwrap();
    fs::set_permissions(
        f.revision_root().join("a"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    let runner = Fake::new(&[Ok(CID)]);
    assert!(matches!(
        f.extract("a", CID, &runner),
        Err(RuntimeError::Invalid(_))
    ));
    assert_eq!(runner.args().len(), 1);
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
            &Fake::new(&[Ok(CID), Ok("blob\0"), Ok("text")]),
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

fn jj(f: &Fixture, args: &[&str]) -> String {
    let output = Command::new("jj")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
        .env("HOME", f._temp.path())
        .env("JJ_CONFIG", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(args)
        .current_dir(&f.repo)
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
fn real_backends_preserve_exact_bytes_literal_paths_and_dirty_worktree() {
    for use_jj in [false, true] {
        let f = Fixture::new(false);
        fs::remove_dir(f.repo.join(".git")).unwrap();
        git(&f.repo, &["init", "--initial-branch=main"]);
        let files: &[(&str, &[u8])] = &[
            ("crlf", b"a\r\nb\r\n"),
            ("trailing", b"old\n\n\n"),
            ("empty", b""),
            ("no-newline", b"text"),
            ("binary", b"\0\xff\r\n"),
            ("a*[1]", b"literal wildcard"),
            ("-option", b"literal option"),
            ("a\"(1)", b"literal fileset"),
        ];
        for (path, content) in files {
            fs::write(f.repo.join(path), content).unwrap();
        }
        git(&f.repo, &["add", "."]);
        git(&f.repo, &["commit", "-m", "initial"]);
        let revision = git(&f.repo, &["rev-parse", "HEAD"]);
        let jj_before = if use_jj {
            jj(&f, &["git", "init", "--colocate"]);
            Some(jj(
                &f,
                &[
                    "--ignore-working-copy",
                    "log",
                    "-r",
                    "@",
                    "--no-graph",
                    "-T",
                    "commit_id",
                ],
            ))
        } else {
            None
        };
        fs::write(f.repo.join("crlf"), "uncommitted\n").unwrap();
        let before = git(&f.repo, &["status", "--porcelain"]);
        let index = fs::read(f.repo.join(".git/index")).unwrap();
        for (path, content) in files {
            let target = f.extract(path, &revision, &NativeProcessRunner).unwrap();
            assert_eq!(
                fs::read(&target.file).unwrap(),
                *content,
                "jj={use_jj}, path={path}"
            );
            assert_eq!(
                f.extract(path, &revision, &Fake::new(&[Ok(&revision)]))
                    .unwrap(),
                target
            );
        }
        assert!(matches!(
            f.extract("absent", &revision, &NativeProcessRunner),
            Err(RuntimeError::NotFound(_))
        ));
        assert_eq!(
            fs::read_to_string(f.repo.join("crlf")).unwrap(),
            "uncommitted\n"
        );
        assert_eq!(git(&f.repo, &["rev-parse", "HEAD"]), revision);
        assert_eq!(git(&f.repo, &["status", "--porcelain"]), before);
        assert_eq!(fs::read(f.repo.join(".git/index")).unwrap(), index);
        if let Some(before) = jj_before {
            assert_eq!(
                jj(
                    &f,
                    &[
                        "--ignore-working-copy",
                        "log",
                        "-r",
                        "@",
                        "--no-graph",
                        "-T",
                        "commit_id"
                    ]
                ),
                before
            );
        }
    }
}
