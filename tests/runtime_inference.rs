use std::{
    collections::VecDeque,
    fs,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    sync::Mutex,
};
use streams_temper::{
    process::{ProcessError, ProcessOutput, ProcessRequest, ProcessRunner},
    runtime::checkouts::{CheckoutRegistry, RepositoryInspection},
};

fn inspect(
    path: &Path,
) -> Result<Option<RepositoryInspection>, streams_temper::runtime::RuntimeError> {
    Ok(path
        .join(".test-checkout")
        .is_file()
        .then(|| RepositoryInspection {
            root: path.to_path_buf(),
            relative_cwd: ".".into(),
        }))
}
struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    registry: CheckoutRegistry,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let registry = CheckoutRegistry::new(root.join("state"), "host")
            .unwrap()
            .with_inspector(inspect);
        Self {
            _temp: temp,
            root,
            registry,
        }
    }
    fn repo(&self, name: &str) -> PathBuf {
        let root = self.root.join(name);
        fs::create_dir(&root).unwrap();
        fs::write(root.join(".test-checkout"), "").unwrap();
        root
    }
}
#[derive(Default)]
struct Runner {
    replies: Mutex<VecDeque<Result<&'static str, &'static str>>>,
    calls: Mutex<Vec<ProcessRequest>>,
}
impl Runner {
    fn new(replies: &[Result<&'static str, &'static str>]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().copied().collect()),
            ..Self::default()
        }
    }
}
impl ProcessRunner for Runner {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessOutput, ProcessError> {
        self.calls.lock().unwrap().push(request.clone());
        match self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected discovery command")
        {
            Ok(output) => Ok(ProcessOutput {
                status: std::process::ExitStatus::from_raw(0),
                stdout: output.as_bytes().to_vec(),
                stderr: vec![],
            }),
            Err(message) => Err(ProcessError {
                message: message.into(),
                code: Some(1),
                stdout: vec![],
                stderr: vec![],
            }),
        }
    }
}

#[test]
fn explicit_bindings_never_probe_remotes_or_become_defaults() {
    let f = Fixture::new();
    let repo = f.repo("explicit");
    let checkout = f
        .registry
        .register(&repo, Some("exact://explicit"))
        .unwrap();
    let runner = Runner::default();
    assert_eq!(
        f.registry
            .bind_inferred_repositories(&checkout.id, &runner)
            .unwrap(),
        ["exact://explicit"]
    );
    assert!(runner.calls.lock().unwrap().is_empty());
    assert!(f.registry.resolve("exact://explicit", None, None).is_err());
}

#[test]
fn missing_origin_is_observational_and_not_an_implicit_binding() {
    let f = Fixture::new();
    let checkout = f.registry.register(&f.repo("unbound"), None).unwrap();
    let runner = Runner::new(&[Err("no origin")]);
    assert!(
        f.registry
            .bind_inferred_repositories(&checkout.id, &runner)
            .unwrap()
            .is_empty()
    );
    assert!(
        f.registry
            .get(&checkout.id)
            .unwrap()
            .repository_urls
            .is_empty()
    );
    let calls = runner.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].program, std::ffi::OsStr::new("git"));
    assert_eq!(
        calls[0].args,
        ["config", "--get", "remote.origin.url"].map(std::ffi::OsString::from)
    );
    assert_eq!(calls[0].timeout, 15.0);
    assert_eq!(
        calls[0]
            .env
            .get(std::ffi::OsStr::new("GIT_OPTIONAL_LOCKS"))
            .unwrap(),
        "0"
    );
    assert!(
        calls[0]
            .env
            .keys()
            .all(|key| !key.to_string_lossy().starts_with("GIT_") || key == "GIT_OPTIONAL_LOCKS")
    );
}

#[test]
fn jj_origin_fallback_is_readonly_and_deduplicates_exact_urls() {
    let f = Fixture::new();
    let repo = f.repo("jj");
    fs::create_dir_all(repo.join(".jj/repo")).unwrap();
    let checkout = f.registry.register(&repo, None).unwrap();
    let runner = Runner::new(&[
        Err("no colocated git"),
        Ok("backup ignored://backup\n origin exact://origin\norigin exact://origin\n"),
    ]);
    assert_eq!(
        f.registry
            .bind_inferred_repositories(&checkout.id, &runner)
            .unwrap(),
        ["exact://origin"]
    );
    assert_eq!(
        f.registry.get(&checkout.id).unwrap().repository_urls,
        ["exact://origin"]
    );
    let calls = runner.calls.lock().unwrap();
    assert_eq!(calls[1].program, std::ffi::OsStr::new("jj"));
    assert_eq!(
        calls[1].args,
        [
            "--ignore-working-copy",
            "--at-operation=@",
            "git",
            "remote",
            "list"
        ]
        .map(std::ffi::OsString::from)
    );
    assert!(f.registry.resolve("exact://origin", None, None).is_err());
}

#[test]
fn malformed_discovered_locators_are_errors_not_missing_origins() {
    let f = Fixture::new();
    let checkout = f
        .registry
        .register(&f.repo("invalid-origin"), None)
        .unwrap();
    let runner = Runner::new(&[Ok("https://example.test/a\tb")]);
    assert!(
        f.registry
            .bind_inferred_repositories(&checkout.id, &runner)
            .is_err()
    );
    assert!(
        f.registry
            .get(&checkout.id)
            .unwrap()
            .repository_urls
            .is_empty()
    );
}

#[test]
fn shared_jj_checkout_inherits_backing_bindings_before_origin() {
    let f = Fixture::new();
    let backing = f.repo("backing");
    fs::create_dir_all(backing.join(".jj/repo")).unwrap();
    f.registry
        .register(&backing, Some("exact://backing"))
        .unwrap();
    let shared = f.repo("shared");
    fs::create_dir(shared.join(".jj")).unwrap();
    fs::write(shared.join(".jj/repo"), "../../backing/.jj/repo\n").unwrap();
    let checkout = f.registry.register(&shared, None).unwrap();
    let runner = Runner::new(&[Ok("exact://backing\n")]);
    assert_eq!(
        f.registry
            .bind_inferred_repositories(&checkout.id, &runner)
            .unwrap(),
        ["exact://backing"]
    );
    assert_eq!(runner.calls.lock().unwrap().len(), 1);
    assert!(f.registry.resolve("exact://backing", None, None).is_err());
}
