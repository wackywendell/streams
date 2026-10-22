use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
    checkout_id: String,
    revision: String,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cwd = temporary.path().join("working");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(cwd.join("docs")).unwrap();
        fs::write(home.join("gitconfig"), "").unwrap();
        let mut fixture = Self {
            _temporary: temporary,
            home: fs::canonicalize(home).unwrap(),
            cwd: fs::canonicalize(cwd).unwrap(),
            checkout_id: String::new(),
            revision: String::new(),
        };
        fixture.git(&["init", "--quiet", "--template="]);
        for (key, value) in [
            ("user.name", "Streams Test"),
            ("user.email", "streams@example.test"),
            ("commit.gpgSign", "false"),
            ("tag.gpgSign", "false"),
            ("core.hooksPath", "/dev/null"),
            ("gc.auto", "0"),
        ] {
            fixture.git(&["config", "--local", key, value]);
        }
        fs::write(fixture.cwd.join("docs/design.md"), "Historical design\n").unwrap();
        fixture.git(&["add", "docs/design.md"]);
        fixture.git(&["commit", "--quiet", "-m", "Original document"]);
        fixture.revision = fixture.git(&["rev-parse", "HEAD"]);
        fs::write(
            fixture.cwd.join("docs/design.md"),
            "Current committed design\n",
        )
        .unwrap();
        fixture.git(&["commit", "--quiet", "-am", "New document"]);
        fs::write(
            fixture.cwd.join("docs/design.md"),
            "Unsaved current design\n",
        )
        .unwrap();
        fs::write(fixture.cwd.join("untracked.txt"), "Keep me\n").unwrap();
        fixture.ok(&["init", "--sync", "none"]);
        fixture.checkout_id = fixture.ok(&[
            "checkout",
            "register",
            fixture.cwd.to_str().unwrap(),
            "--repository",
            "https://example.test/repo",
        ])["checkout"]["id"]
            .as_str()
            .unwrap()
            .into();
        fixture
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program);
        command
            .env_clear()
            .current_dir(&self.cwd)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"))
            .stdin(Stdio::null());
        command
    }

    fn git_output(&self, args: &[&str]) -> Output {
        self.command("git").args(args).output().unwrap()
    }

    fn git(&self, args: &[&str]) -> String {
        let output = self.git_output(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(env!("CARGO_BIN_EXE_streams"))
            .args(args)
            .arg("--json")
            .output()
            .unwrap()
    }

    fn ok(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(output.status.success(), "{args:?}: {output:?}");
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn document(&self, path: &str, revision: &str) -> Vec<String> {
        [
            "checkout",
            "document",
            &self.checkout_id,
            path,
            "--revision",
            revision,
        ]
        .map(str::to_owned)
        .into()
    }

    fn document_output(&self, path: &str, revision: &str) -> Output {
        let args = self.document(path, revision);
        self.run(&args.iter().map(String::as_str).collect::<Vec<_>>())
    }
}

#[test]
fn historical_document_is_direct_json_and_preserves_dirty_worktree_and_revision_spelling() {
    let f = Fixture::new();
    let head = f.git(&["rev-parse", "HEAD"]);
    let status = f.git(&["status", "--porcelain=v1", "--untracked-files=all"]);
    let index = fs::read(f.cwd.join(".git/index")).unwrap();
    let spelling = format!("  {}  ", f.revision);
    let output = f.document_output("docs/design.md", &spelling);
    assert!(output.status.success(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["checkout_id", "file", "revision", "root"]
    );
    assert_eq!(value["checkout_id"], f.checkout_id);
    assert_eq!(value["revision"], spelling);
    let root = Path::new(value["root"].as_str().unwrap());
    let file = Path::new(value["file"].as_str().unwrap());
    assert!(root.starts_with(f.home.join("state/streams/catalogs")));
    assert_eq!(root.file_name().unwrap(), &f.revision[..12]);
    assert_eq!(
        root.parent().unwrap().file_name().unwrap(),
        f.checkout_id.as_str()
    );
    assert_eq!(
        root.parent()
            .unwrap()
            .parent()
            .unwrap()
            .file_name()
            .unwrap(),
        "revisions"
    );
    assert_eq!(file, root.join("docs/design.md"));
    assert_eq!(fs::read_to_string(file).unwrap(), "Historical design\n");
    assert_eq!(f.git(&["rev-parse", "HEAD"]), head);
    assert_eq!(
        f.git(&["status", "--porcelain=v1", "--untracked-files=all"]),
        status
    );
    assert_eq!(fs::read(f.cwd.join(".git/index")).unwrap(), index);
    assert_eq!(
        fs::read_to_string(f.cwd.join("docs/design.md")).unwrap(),
        "Unsaved current design\n"
    );
    assert_eq!(
        fs::read_to_string(f.cwd.join("untracked.txt")).unwrap(),
        "Keep me\n"
    );
}

#[test]
fn immutable_document_and_missing_hint_are_reused_without_available_git_content() {
    let f = Fixture::new();
    let first = f.document_output("docs/design.md", &f.revision);
    assert!(first.status.success(), "{first:?}");
    let value: Value = serde_json::from_slice(&first.stdout).unwrap();
    let file = Path::new(value["file"].as_str().unwrap());
    let modified = fs::metadata(file).unwrap().modified().unwrap();
    let missing = f.document_output("docs/absent.md", &f.revision);
    assert_eq!(missing.status.code(), Some(2), "{missing:?}");
    let error: Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert!(error["error"]["message"].as_str().unwrap().contains("git"));
    let sentinel = file.parent().unwrap().join(".absent.md.missing");
    assert_eq!(fs::read(&sentinel).unwrap(), b"");
    let sentinel_modified = fs::metadata(&sentinel).unwrap().modified().unwrap();

    let object = f.git(&["rev-parse", &format!("{}:docs/design.md", f.revision)]);
    let object_path = f
        .cwd
        .join(".git/objects")
        .join(&object[..2])
        .join(&object[2..]);
    fs::rename(&object_path, f.home.join("offline-blob")).unwrap();
    assert!(
        !f.git_output(&["show", &format!("{}:docs/design.md", f.revision)])
            .status
            .success()
    );
    let repeated = f.document_output("docs/design.md", &f.revision);
    assert!(repeated.status.success(), "{repeated:?}");
    assert_eq!(
        serde_json::from_slice::<Value>(&repeated.stdout).unwrap(),
        value
    );
    assert_eq!(fs::metadata(file).unwrap().modified().unwrap(), modified);
    assert_eq!(fs::read_to_string(file).unwrap(), "Historical design\n");
    for _ in 0..2 {
        let repeated = f.document_output("docs/absent.md", &f.revision);
        assert_eq!(repeated.status.code(), Some(2), "{repeated:?}");
        let error: Value = serde_json::from_slice(&repeated.stdout).unwrap();
        assert_eq!(error["error"]["kind"], "not-found");
        assert_eq!(
            error["error"]["message"],
            format!(
                "document revision is unavailable: docs/absent.md at {}",
                f.revision
            )
        );
        assert_eq!(
            fs::metadata(&sentinel).unwrap().modified().unwrap(),
            sentinel_modified
        );
    }
}

#[test]
fn unsafe_paths_and_leading_dash_revisions_exit_two_without_changing_current_file() {
    let f = Fixture::new();
    for path in ["../design.md", "docs/../../design.md", "/tmp/design.md"] {
        let output = f.document_output(path, &f.revision);
        assert_eq!(output.status.code(), Some(2), "{path}: {output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(value.get("error").is_some(), "{value}");
    }
    for revision in ["--revision=-HEAD", "--revision= -HEAD "] {
        let output = f.run(&[
            "checkout",
            "document",
            &f.checkout_id,
            "docs/design.md",
            revision,
        ]);
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            value["error"]["message"],
            "invalid host-local state: revision must not start with a dash"
        );
    }
    let output = f.document_output("docs/design.md", "-HEAD");
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    assert_eq!(
        fs::read_to_string(f.cwd.join("docs/design.md")).unwrap(),
        "Unsaved current design\n"
    );
    assert_eq!(
        f.ok(&["checkout", "show", &f.checkout_id])["checkout"]["id"],
        json!(f.checkout_id)
    );
}
