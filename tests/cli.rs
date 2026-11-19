use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Fixture {
    _temporary: tempfile::TempDir,
    home: PathBuf,
    cwd: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().unwrap();
        let home = temporary.path().join("home");
        let cwd = temporary.path().join("working");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        fs::write(home.join("gitconfig"), "[user]\n name = Streams Test\n email = streams@example.test\n[commit]\n gpgSign = false\n").unwrap();
        Self {
            _temporary: temporary,
            home: fs::canonicalize(home).unwrap(),
            cwd: fs::canonicalize(cwd).unwrap(),
        }
    }
    fn root(&self) -> PathBuf {
        self.home.join("data/streams/catalog")
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_streams"));
        command
            .env_clear()
            .current_dir(&self.cwd)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", self.home.join("config"))
            .env("XDG_DATA_HOME", self.home.join("data"))
            .env("XDG_STATE_HOME", self.home.join("state"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.home.join("gitconfig"));
        command
    }
    fn run(&self, args: &[&str], input: Option<&str>) -> (i32, Value) {
        let mut command = self.command();
        command
            .args(args)
            .arg("--json")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if input.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = command.spawn().unwrap();
        if let Some(input) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        let output = child.wait_with_output().unwrap();
        let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "{args:?}: {error}; stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        });
        (output.status.code().unwrap(), value)
    }
    fn ok(&self, args: &[&str]) -> Value {
        let (code, value) = self.run(args, None);
        assert_eq!(code, 0, "{args:?}: {value}");
        value
    }
    fn init(&self) {
        self.ok(&["init", "--sync", "none"]);
    }
    fn create(&self, title: &str) -> Value {
        self.ok(&["create", title])["stream"].clone()
    }
}

#[test]
fn record_local_offline_create_saves_without_publication_or_transport_admission() {
    let f = Fixture::new();
    let remote = f.home.join("unavailable.git");
    f.ok(&[
        "init",
        "--sync",
        "git",
        "--remote",
        remote.to_str().unwrap(),
    ]);
    let transport = f.root().join(".catalog-local/transport.json");
    let before = fs::read(&transport).unwrap();
    let saved = f.ok(&["create", "Saved offline", "--goal", "Keep local"]);
    assert_eq!(saved["save"]["local"], "persisted");
    assert_eq!(saved["save"]["domain"]["outcome"], "changes");
    assert!(saved.get("publication").is_none());
    assert!(saved["save"].get("sharing").is_none());
    assert_eq!(saved["stream"]["goal"], "Keep local");
    assert!(saved["stream"].get("status").is_none());
    assert_eq!(saved["stream"]["title_mode"], "manual");
    assert_eq!(saved["stream"]["goal_mode"], "manual");
    assert_eq!(
        f.ok(&["show", saved["stream"]["id"].as_str().unwrap()])["stream"],
        saved["stream"]
    );
    assert_eq!(fs::read(&transport).unwrap(), before);
    fs::write(&transport, "unreadable transport configuration").unwrap();
    let another = f.ok(&["create", "Still local"]);
    assert_eq!(another["save"]["local"], "persisted");
    assert!(another.get("publication").is_none());
    assert_eq!(
        fs::read(transport).unwrap(),
        b"unreadable transport configuration"
    );
    assert_eq!(f.ok(&["list"])["streams"].as_array().unwrap().len(), 2);
}

#[test]
fn rejected_reads_adoption_and_missing_sync_leave_uninitialized_catalog_untouched() {
    let f = Fixture::new();
    for args in [
        vec!["list"],
        vec!["init"],
        vec!["init", "--sync", "git", "--adopt"],
        vec![
            "agent",
            "context",
            "--harness",
            "pi",
            "--session-id",
            "unknown",
        ],
    ] {
        let (code, value) = f.run(&args, None);
        assert_eq!(code, 2, "{value}");
        assert!(value.get("error").is_some());
        assert!(!f.root().exists());
        assert!(!f.home.join("config/streams/host.json").exists());
    }
}

#[test]
fn explicit_current_reads_need_no_supported_or_available_frontend() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("Current");
    let id = stream["id"].as_str().unwrap();
    assert_eq!(f.ok(&["show", id, "--frontend", "cmux"])["stream"], stream);
    for frontend in ["auto", "cmux"] {
        let output = f
            .command()
            .env("STREAMS_STREAM_ID", id)
            .args(["show", "--current", "--frontend", frontend, "--json"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["stream"], stream);
    }
}

#[test]
fn session_attach_captures_explicit_native_id_and_keeps_local_directory_state_private() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("Work");
    let id = stream["id"].as_str().unwrap();
    let attached = f.ok(&[
        "session",
        "attach",
        id,
        "--harness",
        "pi",
        "--session-id",
        "opaque-native-id",
        "--title",
        "Contribution",
    ]);
    let session = &attached["session"];
    let session_id = session["id"].as_str().unwrap();
    assert_eq!(session["harness_session_id"], "opaque-native-id");
    assert_eq!(
        f.ok(&["session", "list", "--stream", id])["sessions"],
        json!([session])
    );
    assert_eq!(f.ok(&["session", "show", session_id])["session"], *session);
    let directory = f.ok(&["session", "directory", session_id]);
    assert_eq!(directory["directory"]["cwd"], json!(f.cwd));
    let changed = f.ok(&[
        "session",
        "update",
        session_id,
        "--recap",
        "Added search filters",
        "--recap-short",
        "Search",
        "--state",
        "Working",
        "--state-short",
        "Review",
        "--remaining-short",
        "Land",
        "--remaining-file",
        "-",
    ]);
    assert_eq!(changed["session"]["recap"], "Added search filters");
    assert_eq!(changed["session"]["recap_short"], "Search");
    assert_eq!(changed["session"]["state"], "Working");
    assert_eq!(changed["session"]["state_short"], "Review");
    assert_eq!(changed["session"]["remaining_short"], "Land");
    let files = catalog_files(&f.root());
    assert!(
        !files
            .iter()
            .any(|(_, bytes)| String::from_utf8_lossy(bytes).contains(f.cwd.to_str().unwrap()))
    );
    let context = f.ok(&[
        "agent",
        "context",
        "--harness",
        "pi",
        "--session-id",
        "opaque-native-id",
    ]);
    assert_eq!(context["session"]["id"], session_id);
    assert_eq!(context["stream"]["id"], id);
    assert_eq!(context["checkouts"], json!([]));
    assert_eq!(context["document_locations"], json!([]));
}

#[test]
fn membership_move_keeps_checkout_selection_and_never_uses_destination_alias_url() {
    use streams::{
        application, file_store::records::FileRecordStore, operations::membership::MoveIntent,
    };
    let f = Fixture::new();
    f.init();
    let initialized = Command::new("git")
        .args(["init", "--initial-branch=main"])
        .arg(&f.cwd)
        .env("GIT_CONFIG_GLOBAL", f.home.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(initialized.status.success());
    let source = f.create("Source");
    let destination = f.create("Destination");
    let source_id = source["id"].as_str().unwrap();
    let destination_id = destination["id"].as_str().unwrap();
    let source_url = "https://example.test/original.git";
    f.ok(&["update", source_id, "--add-repository", "app", source_url]);
    let destination = f.ok(&[
        "update",
        destination_id,
        "--add-repository",
        "app",
        "https://example.test/different.git",
    ])["stream"]
        .clone();
    let attached = f.ok(&[
        "session",
        "attach",
        source_id,
        "--harness",
        "pi",
        "--session-id",
        "native",
        "--title",
        "Work",
        "--repository",
        "app",
        "--ref",
        "main",
    ]);
    let session = &attached["session"];
    let session_id = session["id"].as_str().unwrap();
    let selected = f.ok(&["session", "directory", session_id])["directory"].clone();
    let checkout_id = selected["checkout_id"].as_str().unwrap();
    let host = session["host_id"].as_str().unwrap();
    let store = FileRecordStore::new(f.root(), host).unwrap();
    let intent = MoveIntent::new(host, "pi", "native", destination_id).unwrap();
    application::move_session_observed(&store, &intent).unwrap();
    assert_eq!(
        f.ok(&["session", "directory", session_id])["directory"],
        selected
    );
    let selected_again = f.ok(&[
        "session",
        "directory",
        session_id,
        "--checkout",
        checkout_id,
    ]);
    assert_eq!(selected_again["directory"], selected);
    let (code, _) = f.run(
        &[
            "session",
            "directory",
            session_id,
            "--checkout",
            checkout_id,
            "--repository",
            "app",
        ],
        None,
    );
    assert_eq!(code, 2);
    assert_eq!(
        f.ok(&["session", "directory", session_id])["directory"],
        selected
    );
    let moved = f.ok(&["session", "show", session_id]);
    assert_eq!(moved["session"]["checkout"]["repository_url"], source_url);
    assert_eq!(f.ok(&["show", destination_id])["stream"], destination);
}

#[test]
fn record_local_cli_initializes_enrolls_reports_and_retries_without_scans_or_publication() {
    use streams::{
        file_store::records::relative_record_path, model::RecordKind,
        storage::records::RecordAddress,
    };
    let f = Fixture::new();
    f.init();
    let args = [
        "agent",
        "context",
        "--harness",
        "pi",
        "--session-id",
        "11234567-89ab-4def-8123-456789abcdef",
    ];
    let initial = f.ok(&args);
    assert!(initial["session"].is_null());
    assert!(initial["stream"].is_null());
    let enrollment = json!({"version":2,"catalog_source":initial["catalog_source"],
        "actor":"user","harness":"pi","harness_session_id":args[5],
        "stream":{"title":"Work","goal":"Original"},"session":{"title":"Contribution"},"cwd":f.cwd});
    let (code, enrolled) = f.run(&["agent", "enroll"], Some(&enrollment.to_string()));
    assert_eq!(code, 0, "{enrolled}");
    assert!(enrolled.get("authority").is_none());
    assert!(enrolled.get("publication").is_none());
    let context = f.ok(&args);
    let listed = f.ok(&["list"]);
    let mut expected_listed = context["stream"].clone();
    expected_listed["status"] = json!("active");
    assert_eq!(listed["streams"], json!([expected_listed]));
    let shown = f.ok(&["show", context["stream"]["id"].as_str().unwrap()]);
    assert_eq!(shown["stream"], context["stream"]);
    assert_eq!(shown["status"], json!("active"));
    assert_eq!(shown["sessions"], json!([context["session"]]));
    assert_eq!(
        f.ok(&["check", context["stream"]["id"].as_str().unwrap()]),
        json!({"valid":true,"streams":1,"sessions":1})
    );
    let publication = f.ok(&["sync"]);
    assert_eq!(publication["sharing"], "local-only");
    assert!(publication["domain"].is_null());
    let address = RecordAddress::new(
        context["host_id"].as_str().unwrap(),
        RecordKind::Stream,
        context["stream"]["id"].as_str().unwrap(),
    )
    .unwrap();
    let path = f.root().join(relative_record_path(&address).unwrap());
    let malformed = f.root().join("hosts/unrelated/streams");
    fs::create_dir_all(&malformed).unwrap();
    for index in 0..64 {
        fs::write(malformed.join(format!("bad-{index}.json")), "malformed").unwrap();
    }
    fs::write(
        f.root().join(".catalog-local/transport.json"),
        "malformed transport",
    )
    .unwrap();
    let request = json!({"version":2,"catalog_source":context["catalog_source"],
        "actor":"user","kind":"stream","before":context["stream"],"changes":{"goal":"Captured"},"inputs":[],
        "title_default":context["title_defaults"]["stream"]});
    let (code, first) = f.run(&["agent", "apply"], Some(&request.to_string()));
    assert_eq!(code, 0, "{first}");
    assert_eq!(first["save"]["domain"]["outcome"], "changes");
    assert_eq!(first["stream"]["goal"], "Captured");
    assert!(first["save"].get("sharing").is_none());
    assert!(first.get("publication").is_none());
    let accepted = fs::read(&path).unwrap();
    let (code, retry) = f.run(&["agent", "apply"], Some(&request.to_string()));
    assert_eq!(code, 0, "{retry}");
    assert_eq!(retry["save"]["domain"]["outcome"], "no-change");
    assert_eq!(retry["save"]["local"], "unchanged");
    assert_eq!(fs::read(&path).unwrap(), accepted);
    let mut wrong_source = request;
    wrong_source["catalog_source"] = json!("wrong");
    let (code, error) = f.run(&["agent", "apply"], Some(&wrong_source.to_string()));
    assert_eq!(code, 2, "{error}");
    assert_eq!(fs::read(&path).unwrap(), accepted);
    let current = f.ok(&args);
    assert_eq!(current["stream"], first["stream"]);
    let shown = f
        .command()
        .env(
            "STREAMS_STREAM_ID",
            current["stream"]["id"].as_str().unwrap(),
        )
        .args(["show", "--current", "--json"])
        .output()
        .unwrap();
    assert!(
        shown.status.success(),
        "{}",
        String::from_utf8_lossy(&shown.stderr)
    );
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    assert_eq!(shown["stream"], current["stream"]);
    assert_eq!(shown["status"], json!("active"));
    assert!(shown.get("sessions").is_none());
    let id = current["stream"]["id"].as_str().unwrap();
    assert!(uuid::Uuid::parse_str(id).is_ok());
    let (code, _) = f.run(&["show", id], None);
    assert_eq!(
        code, 2,
        "a positional UUID is discovery and must not bypass malformed unrelated records"
    );
    let session_request = json!({"version":2,"catalog_source":current["catalog_source"],
        "actor":"automatic","kind":"session","before":current["session"],"changes":{"state":"Complete"},"inputs":[],
        "title_default":current["title_defaults"]["session"]});
    let (code, saved) = f.run(&["agent", "apply"], Some(&session_request.to_string()));
    assert_eq!(code, 0, "{saved}");
    assert_eq!(saved["session"]["state"], "Complete");
    assert_eq!(f.ok(&args)["session"], saved["session"]);
    let edited = f
        .command()
        .env("STREAMS_STREAM_ID", id)
        .args([
            "update",
            "--current",
            "--title",
            "Human title",
            "--add-repository",
            "app",
            "https://example.test/app.git",
        ])
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        edited.status.success(),
        "{}",
        String::from_utf8_lossy(&edited.stdout)
    );
    let edited: Value = serde_json::from_slice(&edited.stdout).unwrap();
    assert_eq!(edited["save"]["local"], "persisted");
    assert!(edited["save"].get("sharing").is_none());
    assert!(edited.get("publication").is_none());
    assert_eq!(edited["stream"]["title"], "Human title");
    assert_eq!(edited["stream"]["title_mode"], "manual");
    assert_eq!(
        f.ok(&args)["stream"]["repositories"]["app"]["url"],
        "https://example.test/app.git"
    );
    let attached = f
        .command()
        .env("STREAMS_STREAM_ID", id)
        .args([
            "session",
            "attach",
            "--current",
            "--harness",
            "pi",
            "--session-id",
            "point-human",
            "--title",
            "Point attachment",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        attached.status.success(),
        "{}",
        String::from_utf8_lossy(&attached.stdout)
    );
    let attached: Value = serde_json::from_slice(&attached.stdout).unwrap();
    assert_eq!(attached["session"]["stream_id"], id);
    assert_eq!(attached["save"]["local"], "persisted");
    assert!(attached["save"].get("sharing").is_none());
    assert!(f.ok(&["checkout", "list"])["checkouts"].is_array());
}

#[test]
fn record_local_session_commands_pin_checkout_identity_and_preserve_host_qualified_discovery() {
    use streams::{
        application,
        file_store::records::{FileRecordStore, relative_record_path},
        model::{Record, Session, Stream},
        storage::records::RecordAddress,
    };
    let f = Fixture::new();
    let initialized = Command::new("git")
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &f.home)
        .env("GIT_CONFIG_GLOBAL", f.home.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .current_dir(&f.cwd)
        .args(["init", "--quiet"])
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    f.init();
    let source = f.ok(&[
        "agent",
        "context",
        "--harness",
        "pi",
        "--session-id",
        "primary",
    ]);
    let proposal = json!({"version":2,"catalog_source":source["catalog_source"],"harness":"pi","harness_session_id":"primary","stream":{"title":"Effort"},"session":{"title":"Primary"},"actor":"user","cwd":f.cwd});
    let (code, enrolled) = f.run(&["agent", "enroll"], Some(&proposal.to_string()));
    assert_eq!(code, 0, "{enrolled}");
    let id = enrolled["stream"]["id"].as_str().unwrap();
    let host = enrolled["session"]["host_id"].as_str().unwrap();
    let original = "https://example.test/original.git";
    f.ok(&["update", id, "--add-repository", "app", original]);
    let attached = f.ok(&[
        "session",
        "attach",
        id,
        "--harness",
        "pi",
        "--session-id",
        "manual",
        "--title",
        "Human lane",
        "--repository",
        "app",
        "--ref",
        "main",
    ]);
    let session_id = attached["session"]["id"].as_str().unwrap();
    assert_eq!(session_id, Session::id_for(host, "pi", "manual"));
    assert_eq!(attached["session"]["checkout"]["repository_url"], original);
    assert_eq!(attached["session"]["title_mode"], "manual");
    let checkouts = f.ok(&["checkout", "list"]);
    assert_eq!(checkouts["checkouts"].as_array().unwrap().len(), 1);
    let checkout_id = checkouts["checkouts"][0]["id"].as_str().unwrap();
    assert_eq!(
        f.ok(&["checkout", "show", checkout_id])["checkout"],
        checkouts["checkouts"][0]
    );
    assert!(attached.get("publication").is_none());
    assert!(attached["save"].get("sharing").is_none());
    let qualified = format!("{host}/{session_id}");
    assert_eq!(
        f.ok(&["session", "show", &qualified])["session"],
        attached["session"]
    );
    let updated = f.ok(&[
        "session", "update", &qualified, "--state", "Progress", "--status", "blocked",
    ]);
    assert_eq!(updated["session"]["state"], "Progress");
    assert_eq!(updated["session"]["stream_id"], id);
    assert_eq!(
        updated["session"]["checkout"],
        attached["session"]["checkout"]
    );
    assert_eq!(updated["save"]["domain"]["outcome"], "changes");
    assert_eq!(
        f.ok(&["session", "directory", &qualified])["directory"]["cwd"],
        f.cwd.to_str().unwrap()
    );
    let selected = f.cwd.join("chosen");
    fs::create_dir(&selected).unwrap();
    assert_eq!(
        f.ok(&[
            "session",
            "directory",
            &qualified,
            "--cwd",
            selected.to_str().unwrap()
        ])["directory"]["cwd"],
        selected.to_str().unwrap()
    );
    f.ok(&[
        "update",
        id,
        "--remove-repository",
        "app",
        "--add-repository",
        "app",
        "https://example.test/replacement.git",
    ]);
    let (code, _) = f.run(
        &[
            "session",
            "directory",
            &qualified,
            "--cwd",
            f.cwd.to_str().unwrap(),
            "--repository",
            "app",
        ],
        None,
    );
    assert_eq!(
        code, 2,
        "a renamed repository alias cannot replace the independent checkout URL"
    );
    assert_eq!(
        f.ok(&["session", "show", &qualified])["session"]["checkout"]["repository_url"],
        original
    );

    let mut other = proposal.clone();
    other["harness_session_id"] = json!("other");
    let (code, other) = f.run(&["agent", "enroll"], Some(&other.to_string()));
    assert_eq!(code, 0, "{other}");
    let (code, _) = f.run(
        &[
            "session",
            "attach",
            other["stream"]["id"].as_str().unwrap(),
            "--harness",
            "pi",
            "--session-id",
            "manual",
            "--title",
            "Do not move",
        ],
        None,
    );
    assert_eq!(code, 2);
    assert_eq!(
        f.ok(&["session", "show", &qualified])["session"],
        updated["session"]
    );

    let remote_directory = tempfile::tempdir().unwrap();
    let remote = FileRecordStore::new(remote_directory.path(), "replica-host").unwrap();
    remote.initialize().unwrap();
    let remote_stream =
        Record::Stream(Stream::new(id, "Replica effort", None, "replica-host").unwrap());
    let remote_session = Record::Session(
        Session::new(
            Session::id_for("replica-host", "pi", "manual"),
            "replica-host",
            id,
            "pi",
            "manual",
            "Human lane",
        )
        .unwrap(),
    );
    for record in [&remote_stream, &remote_session] {
        application::create_record(&remote, record).unwrap();
        let relative = relative_record_path(&RecordAddress::for_record(record)).unwrap();
        let target = f.root().join(&relative);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(remote_directory.path().join(&relative), target).unwrap();
    }
    let Record::Session(remote_session) = remote_session else {
        unreachable!()
    };
    let remote_ref = format!("replica-host/{}", remote_session.id);
    assert_eq!(
        f.ok(&["session", "show", &remote_ref])["session"]["host_id"],
        "replica-host"
    );
    let (code, ambiguity) = f.run(&["session", "show", "Human lane"], None);
    assert_eq!(code, 2);
    assert_eq!(ambiguity["error"]["kind"], "ambiguous");
    let mut candidates: Vec<_> = ambiguity["error"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    candidates.sort();
    let mut expected = vec![qualified.clone(), remote_ref.clone()];
    expected.sort();
    assert_eq!(candidates, expected);
    let local_stream = format!("{host}/{id}");
    assert_eq!(
        f.ok(&["session", "list", "--stream", &local_stream])["sessions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    for intent in [
        vec!["session", "update", &remote_ref, "--state", "Never"],
        vec![
            "session",
            "directory",
            &remote_ref,
            "--cwd",
            f.cwd.to_str().unwrap(),
        ],
    ] {
        assert_eq!(
            f.run(&intent, None).0,
            2,
            "foreign Sessions are readable, not writable"
        );
    }
    let missing = f.cwd.join("missing");
    let (code, partial) = f.run(
        &[
            "session",
            "attach",
            &local_stream,
            "--harness",
            "pi",
            "--session-id",
            "partial",
            "--title",
            "Partial lane",
            "--cwd",
            missing.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code, 2);
    assert_eq!(partial["save"]["local"], "persisted");
    assert_eq!(partial["save"]["domain"]["outcome"], "changes");
    assert!(partial["completion_error"]["message"].is_string());
    assert_eq!(
        f.ok(&[
            "session",
            "show",
            partial["session"]["id"].as_str().unwrap()
        ])["session"],
        partial["session"]
    );
}

#[test]
fn record_local_create_and_attachment_keep_their_receipts_when_directory_completion_fails() {
    let f = Fixture::new();
    f.init();
    let missing = f.home.join("missing-cwd");
    let (code, value) = f.run(
        &["create", "Saved", "--cwd", missing.to_str().unwrap()],
        None,
    );
    assert_eq!(code, 2, "{value}");
    assert_eq!(value["save"]["local"], "persisted");
    assert!(value["completion_error"]["message"].is_string());
    assert!(value.get("publication").is_none());
    let stream = value["stream"]["id"].as_str().unwrap();
    assert_eq!(f.ok(&["show", stream])["stream"], value["stream"]);
    let (code, attached) = f.run(
        &[
            "session",
            "attach",
            stream,
            "--harness",
            "pi",
            "--session-id",
            "completion-native",
            "--title",
            "Saved session",
            "--cwd",
            missing.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(code, 2, "{attached}");
    assert_eq!(attached["save"]["local"], "persisted");
    assert!(attached["completion_error"]["message"].is_string());
    assert!(attached.get("publication").is_none());
    assert_eq!(
        f.ok(&[
            "session",
            "show",
            attached["session"]["id"].as_str().unwrap()
        ])["session"],
        attached["session"]
    );
}

#[test]
fn initialized_catalog_cannot_be_adopted_or_converted_in_place() {
    let f = Fixture::new();
    f.init();
    f.create("Existing");
    let before = catalog_files(&f.root());
    for args in [
        vec!["init", "--sync", "git"],
        vec!["init", "--sync", "git", "--adopt"],
    ] {
        let (code, rejected) = f.run(&args, None);
        assert_eq!(code, 2, "{rejected}");
        assert_eq!(catalog_files(&f.root()), before);
        assert!(!f.root().join(".git").exists());
    }
}

#[test]
fn frontend_policy_errors_keep_typed_candidates_without_native_calls() {
    let f = Fixture::new();
    let output = f
        .command()
        .env("CMUX_WORKSPACE_ID", "cmux-work")
        .env("HERDR_ENV", "1")
        .env("HERDR_PANE_ID", "herdr-pane")
        .args(["frontend", "get", "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["kind"], "ambiguous");
    assert_eq!(error["error"]["candidates"], json!(["cmux", "herdr"]));
    assert!(!f.root().exists());
    assert_eq!(
        f.ok(&["frontend", "get", "--frontend", "hc"]),
        json!({"configured":"auto","requested":"hc","resolved":"hc"})
    );
    let unknown = f
        .command()
        .args(["frontend", "get", "--frontend", "third-party", "--json"])
        .output()
        .unwrap();
    assert_ne!(unknown.status.code(), Some(0), "{unknown:?}");
}

#[test]
fn local_catalog_does_not_require_git_in_plain_directories() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let bin = f.home.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        "#!/bin/sh\nprintf called >> \"$STREAMS_TEST_GIT_PROBE\"\nexit 99\n",
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o700)).unwrap();
    let probe = f.home.join("git-called");
    for args in [
        vec!["init", "--sync", "none"],
        vec!["create", "No subprocess"],
        vec!["list"],
        vec!["update", "No subprocess", "--goal", "Still local"],
        vec!["check"],
        vec!["sync"],
    ] {
        let output = f
            .command()
            .env("PATH", &bin)
            .env("STREAMS_TEST_GIT_PROBE", &probe)
            .args(&args)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let data: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(data.get("runtime").is_none(), "{data}");
        assert!(!probe.exists());
    }
}

#[test]
fn reading_a_git_catalog_starts_no_git_process() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let remote = f.home.join("unavailable.git");
    f.ok(&[
        "init",
        "--sync",
        "git",
        "--remote",
        remote.to_str().unwrap(),
    ]);
    let stream = f.create("Listed");
    f.create("Another");
    f.run(&["sync"], None);
    let real = String::from_utf8(
        Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let bin = f.home.join("bin");
    fs::create_dir(&bin).unwrap();
    let git = bin.join("git");
    fs::write(
        &git,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$STREAMS_TEST_GIT_PROBE\"\nexec {} \"$@\"\n",
            real.trim()
        ),
    )
    .unwrap();
    fs::set_permissions(&git, fs::Permissions::from_mode(0o700)).unwrap();
    let probe = f.home.join("git-called");
    let id = stream["id"].as_str().unwrap();
    for args in [
        vec!["list"],
        vec!["list", "--all"],
        vec!["show", id],
        vec!["tidy", "--days", "3650"],
    ] {
        let output = f
            .command()
            .env("PATH", &bin)
            .env("STREAMS_TEST_GIT_PROBE", &probe)
            .args(&args)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        let data: Value = serde_json::from_slice(&output.stdout).unwrap();
        if args[0] == "list" {
            let listed = data["streams"].as_array().unwrap();
            assert_eq!(listed.len(), 2, "{data}");
            assert!(listed.iter().all(|s| s["updated_at_ms"].as_u64() > Some(0)));
        }
        assert!(
            !probe.exists(),
            "{args:?} started Git: {}",
            fs::read_to_string(&probe).unwrap()
        );
    }
}

#[test]
fn session_references_and_candidates_keep_the_host_qualified_identity() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("Sessions");
    let local = f.ok(&[
        "session",
        "attach",
        stream["id"].as_str().unwrap(),
        "--harness",
        "pi",
        "--session-id",
        "native",
        "--title",
        "Contribution",
    ])["session"]
        .clone();
    let id = local["id"].as_str().unwrap();
    let host = local["host_id"].as_str().unwrap();
    let foreign_host = "00000000-0000-4000-8000-000000000000";
    let mut foreign = local.clone();
    foreign["host_id"] = json!(foreign_host);
    foreign["harness_session_id"] = json!("foreign-native");
    let foreign_id = streams::model::Session::id_for(foreign_host, "pi", "foreign-native");
    foreign["id"] = json!(foreign_id);
    let mut foreign_stream = stream.clone();
    foreign_stream["owner"]["host_id"] = json!(foreign_host);
    let stream_address = streams::storage::records::RecordAddress::new(
        foreign_host,
        streams::model::RecordKind::Stream,
        stream["id"].as_str().unwrap(),
    )
    .unwrap();
    let stream_path = f
        .root()
        .join(streams::file_store::records::relative_record_path(&stream_address).unwrap());
    fs::create_dir_all(stream_path.parent().unwrap()).unwrap();
    fs::write(
        stream_path,
        serde_json::to_vec_pretty(&foreign_stream).unwrap(),
    )
    .unwrap();
    let address = streams::storage::records::RecordAddress::new(
        foreign_host,
        streams::model::RecordKind::Session,
        &foreign_id,
    )
    .unwrap();
    let path = f
        .root()
        .join(streams::file_store::records::relative_record_path(&address).unwrap());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec_pretty(&foreign).unwrap()).unwrap();
    assert_eq!(f.ok(&["session", "show", id])["session"], local);
    let (code, error) = f.run(&["session", "show", "Contribution"], None);
    assert_eq!(code, 2, "{error}");
    assert_eq!(error["error"]["kind"], "ambiguous", "{error}");
    let mut candidates: Vec<_> = error["error"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    candidates.sort();
    let qualified = format!("{host}/{id}");
    let foreign_qualified = format!("{foreign_host}/{foreign_id}");
    let mut expected = vec![qualified.clone(), foreign_qualified.clone()];
    expected.sort();
    assert_eq!(candidates, expected);
    assert_eq!(f.ok(&["session", "show", &qualified])["session"], local);
    assert_eq!(
        f.ok(&["session", "show", &foreign_qualified])["session"],
        foreign
    );
    assert_eq!(
        f.ok(&["session", "update", &qualified, "--state", "Active"])["session"]["state"],
        "Active"
    );
    assert_eq!(
        f.ok(&["session", "directory", &qualified])["directory"]["cwd"],
        f.cwd.to_str().unwrap()
    );
}

#[test]
fn invalid_utf8_captured_input_is_rejected_before_any_record_changes() {
    let f = Fixture::new();
    f.init();
    f.create("Unchanged");
    let before = catalog_files(&f.root());
    let mut child = f
        .command()
        .args(["agent", "apply", "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(&[0xff]).unwrap();
    let output = child.wait_with_output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(error["error"]["message"].is_string());
    assert_eq!(catalog_files(&f.root()), before);
}

#[test]
fn reference_commands_save_without_a_session_and_keep_exact_revisions() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("References");
    let id = stream["id"].as_str().unwrap();
    let added = f.ok(&[
        "reference",
        "add",
        id,
        "--kind",
        "document",
        "--repository-url",
        "git@example.test:tools",
        "--path",
        "docs/plan.md",
        "--name",
        "Plan",
        "--purpose",
        "Guide work",
        "--role",
        "brief",
        "--state",
        "dropped",
        "--ref",
        "topic",
        "--commit",
        "commit",
        "--source-session",
        "source",
    ]);
    assert_eq!(added["save"]["local"], "persisted");
    assert_eq!(
        f.ok(&["session", "list", "--stream", id])["sessions"],
        json!([])
    );
    let before = catalog_files(&f.root());
    let listed = f.ok(&["reference", "list", id]);
    assert_eq!(listed["references"][0]["state"], "dropped");
    assert_eq!(listed["references"][0]["target"]["commit"], "commit");
    assert_eq!(catalog_files(&f.root()), before);
    let changed = f.ok(&[
        "reference",
        "edit",
        id,
        "--repository-url",
        "git@example.test:tools",
        "--path",
        "docs/plan.md",
        "--name",
        "Updated",
        "--clear-ref",
    ]);
    let reference = &changed["stream"]["references"][0];
    assert_eq!(reference["state"], "dropped");
    assert!(reference["target"].get("ref").is_none());
    assert_eq!(reference["target"]["commit"], "commit");
    let revision = changed["stream"]["revision"].clone();
    let same = f.ok(&[
        "reference",
        "edit",
        id,
        "--repository-url",
        "git@example.test:tools",
        "--path",
        "docs/plan.md",
        "--name",
        "Updated",
    ]);
    assert_eq!(same["save"]["domain"]["outcome"], "no-change");
    assert_eq!(same["stream"]["revision"], revision);
    let (code, _) = f.run(
        &[
            "reference",
            "edit",
            id,
            "--repository-url",
            "git@example.test:tools",
            "--path",
            "docs/plan.md",
            "--state",
            "active",
        ],
        None,
    );
    assert_eq!(code, 2);
    let restored = f.ok(&[
        "reference",
        "restore",
        id,
        "--repository-url",
        "git@example.test:tools",
        "--path",
        "docs/plan.md",
        "--state",
        "fulfilled",
    ]);
    assert_eq!(restored["stream"]["references"][0]["state"], "fulfilled");
    let corrected = f.ok(&[
        "reference",
        "edit",
        id,
        "--repository-url",
        "git@example.test:tools",
        "--path",
        "docs/plan.md",
        "--target-repository-url",
        "other",
        "--target-path",
        "accepted.md",
    ]);
    assert_eq!(
        corrected["stream"]["references"][0]["target"]["commit"],
        "commit"
    );
    let web = f.ok(&[
        "reference",
        "edit",
        id,
        "--repository-url",
        "other",
        "--path",
        "accepted.md",
        "--kind",
        "link",
        "--target-url",
        "https://example.test/accepted",
    ]);
    assert_eq!(
        web["stream"]["references"][0]["target"],
        json!({"kind":"link","url":"https://example.test/accepted"})
    );
    f.ok(&[
        "reference",
        "drop",
        id,
        "--url",
        "https://example.test/accepted",
    ]);
    assert_eq!(
        f.ok(&["reference", "list", id])["references"][0]["state"],
        "dropped"
    );
    let before = catalog_files(&f.root());
    assert_eq!(
        f.run(
            &[
                "reference",
                "delete",
                id,
                "--url",
                "https://example.test/accepted"
            ],
            None
        )
        .0,
        2
    );
    assert_eq!(catalog_files(&f.root()), before);
    f.ok(&[
        "reference",
        "delete",
        id,
        "--url",
        "https://example.test/accepted",
        "--confirm",
    ]);
    assert_eq!(f.ok(&["reference", "list", id])["references"], json!([]));
}

#[test]
fn reference_cli_preserves_saved_text_and_escapes_human_output_and_selectors() {
    let f = Fixture::new();
    f.init();
    let stream = f.create("Exact text");
    let id = stream["id"].as_str().unwrap();
    let repository = "git@example.test:a'b\u{202e}";
    let path = "docs/line\nfile=é.md";
    let name = "Name\u{1b}[31m\u{e0001}";
    let purpose = "Purpose\u{2028}next\u{2029}last";
    f.ok(&[
        "reference",
        "add",
        id,
        "--kind",
        "document",
        "--repository-url",
        repository,
        "--path",
        path,
        "--name",
        name,
        "--purpose",
        purpose,
        "--role",
        "background",
    ]);
    let listed = f.ok(&["reference", "list", id]);
    assert_eq!(listed["references"][0]["name"], name);
    assert_eq!(listed["references"][0]["purpose"], purpose);
    let output = f
        .command()
        .args(["reference", "list", id])
        .output()
        .unwrap();
    assert!(output.status.success());
    let plain = String::from_utf8(output.stdout).unwrap();
    assert!(!plain.chars().any(|c| c != '\n'
        && matches!(
            unicode_general_category::get_general_category(c),
            unicode_general_category::GeneralCategory::Control
                | unicode_general_category::GeneralCategory::Format
                | unicode_general_category::GeneralCategory::LineSeparator
                | unicode_general_category::GeneralCategory::ParagraphSeparator
        )));
    let selector = plain
        .lines()
        .find_map(|line| line.trim().strip_prefix("selector-json: "))
        .unwrap();
    let selector: Value = serde_json::from_str(selector).unwrap();
    assert_eq!(
        selector,
        json!({"kind":"document","repository_url":repository,"path":path})
    );
    f.ok(&[
        "reference",
        "edit",
        id,
        "--repository-url",
        selector["repository_url"].as_str().unwrap(),
        "--path",
        selector["path"].as_str().unwrap(),
        "--purpose",
        "Accepted",
    ]);
    f.ok(&[
        "reference",
        "add",
        id,
        "--kind",
        "pr",
        "--url",
        "https://example.test/a'b?x=y",
        "--name",
        "PR",
        "--purpose",
        "Ship",
        "--role",
        "deliverable",
    ]);
    let before = catalog_files(&f.root());
    assert_eq!(
        f.run(
            &[
                "reference",
                "add",
                id,
                "--kind",
                "issue",
                "--url",
                "https://example.test/a'b?x=y",
                "--name",
                "Issue",
                "--purpose",
                "Track",
                "--role",
                "dependency"
            ],
            None
        )
        .0,
        2
    );
    assert_eq!(catalog_files(&f.root()), before);
}

#[test]
fn live_and_capture_hints_share_selected_direct_urls_before_any_references() {
    use crate::support::herdr::{FakeHerdr, Reply};
    let f = Fixture::new();
    f.init();
    let stream = f.create("Shared hints");
    let id = stream["id"].as_str().unwrap();
    let attached = f.ok(&[
        "session",
        "attach",
        id,
        "--harness",
        "pi",
        "--session-id",
        "native",
        "--title",
        "Work",
    ]);
    let session_id = attached["session"]["id"].as_str().unwrap();
    assert!(attached["session"]["checkout"].is_null());
    let url = "https://example.test/tools.git";
    let checkout_root = f.home.join("checkout");
    fs::create_dir(&checkout_root).unwrap();
    let initialized = Command::new("git")
        .args(["init", "--quiet"])
        .arg(&checkout_root)
        .env("GIT_CONFIG_GLOBAL", f.home.join("gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        initialized.status.success(),
        "{}",
        String::from_utf8_lossy(&initialized.stderr)
    );
    let registered = f.ok(&[
        "checkout",
        "register",
        checkout_root.to_str().unwrap(),
        "--repository",
        url,
    ]);
    let checkout = registered["checkout"]["id"].as_str().unwrap();
    f.ok(&["session", "directory", session_id, "--checkout", checkout]);
    let transcript = f.home.join("native.jsonl");
    fs::write(&transcript, "{\"type\":\"session\",\"id\":\"native\"}\n").unwrap();
    let snapshot = json!({"workspaces":[{"workspace_id":"w","label":"Work","number":1}],
        "tabs":[{"workspace_id":"w","tab_id":"t","label":"Work","number":1}],
        "panes":[{"workspace_id":"w","tab_id":"t","pane_id":"p","terminal_id":"terminal"}],
        "layouts":[{"workspace_id":"w","tab_id":"t","panes":[{"pane_id":"p"}]}],
        "agents":[{"workspace_id":"w","tab_id":"t","pane_id":"p","terminal_id":"terminal","agent":"pi","agent_session":{"agent":"pi","kind":"path","value":transcript}}]});
    let socket_root = tempfile::tempdir().unwrap();
    streams::frontend::herdr::HerdrNativeSnapshot::parse(&snapshot).unwrap();
    let server =
        FakeHerdr::start(
            &socket_root.path().join("native.sock"),
            move |method, _| match method {
                "events.subscribe" => Reply::Result(json!({"type":"subscription_started"})),
                "session.snapshot" => {
                    Reply::Result(json!({"type":"session_snapshot","snapshot":snapshot}))
                }
                _ => panic!("unexpected native request: {method}"),
            },
        );
    let capture = || {
        let output = f
            .command()
            .env("HERDR_SOCKET_PATH", &server.socket)
            .env("HERDR_ENV", "1")
            .env("HERDR_PANE_ID", "p")
            .args(["agent", "capture-inputs", "--workspace-id", "w", "--json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    let live = || {
        f.ok(&[
            "agent",
            "context",
            "--harness",
            "pi",
            "--session-id",
            "native",
        ])
    };
    let before = catalog_files(&f.root());
    let observed = live();
    let frozen = capture();
    let captured = &frozen["samples"][0]["catalog"];
    assert_eq!(observed["stream"]["references"], json!([]));
    assert_eq!(observed["stream"]["repositories"], json!({}));
    assert_eq!(
        observed["checkouts"],
        json!([{"repository_url":url,"checkout_id":checkout,"root":checkout_root}])
    );
    assert_eq!(captured["checkouts"], observed["checkouts"]);
    assert_eq!(
        captured["document_locations"],
        observed["document_locations"]
    );
    assert_eq!(catalog_files(&f.root()), before);
    fs::write(checkout_root.join("plan.md"), "Plan").unwrap();
    f.ok(&[
        "reference",
        "add",
        id,
        "--kind",
        "document",
        "--repository-url",
        url,
        "--path",
        "plan.md",
        "--name",
        "Plan",
        "--purpose",
        "Guide work",
        "--role",
        "brief",
    ]);
    let observed = live();
    let frozen = capture();
    assert_eq!(
        frozen["samples"][0]["catalog"]["document_locations"],
        observed["document_locations"]
    );
    assert_eq!(
        observed["document_locations"][0]["file"],
        json!(checkout_root.join("plan.md"))
    );
    fs::remove_file(checkout_root.join("plan.md")).unwrap();
    fs::rename(&checkout_root, f.home.join("moved-checkout")).unwrap();
    let observed = live();
    let frozen = capture();
    assert_eq!(observed["checkouts"], json!([]));
    assert_eq!(
        frozen["samples"][0]["catalog"]["checkouts"],
        observed["checkouts"]
    );
    assert_eq!(
        frozen["samples"][0]["catalog"]["document_locations"],
        observed["document_locations"]
    );
    assert!(observed["document_locations"][0]["error"].is_string());
    assert!(
        server
            .methods()
            .iter()
            .all(|method| ["ping", "events.subscribe", "session.snapshot"]
                .contains(&method.as_str()))
    );
}

fn catalog_files(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), files);
            } else {
                files.push((
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                ));
            }
        }
    }
    let mut files = Vec::new();
    for directory in ["streams", "sessions", "conversations", "hosts"] {
        let path = root.join(directory);
        if path.exists() {
            visit(root, &path, &mut files);
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

#[test]
fn config_cli_supports_toml_list_get_set_unset_path_and_model() {
    let f = Fixture::new();
    let expected_path = f.home.join("config/streams/config.toml");
    assert_eq!(f.ok(&["config", "path"]), expected_path.to_str().unwrap());

    let plain_path = f.command().args(["config", "path"]).output().unwrap();
    assert_eq!(plain_path.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&plain_path.stdout).trim(),
        expected_path.to_str().unwrap()
    );

    // Defaults and overrides before any file exists.
    assert_eq!(f.ok(&["config", "--overrides"]), json!({}));
    assert_eq!(f.ok(&["config", "list", "--overrides"]), json!({}));
    let initial = f.ok(&["config"]);
    assert_eq!(initial["herdr"]["viewer"]["mode"], "editor");
    assert_eq!(initial["herdr"]["viewer"]["placement"], "pane-right");
    assert_eq!(initial["pi"]["reporting"]["frequency"], 1);
    assert_eq!(
        f.ok(&["config", "get", "herdr.viewer"]),
        json!({"mode":"editor","placement":"pane-right"})
    );

    // Plain table get outputs TOML.
    let plain_table = f
        .command()
        .args(["config", "get", "herdr.viewer"])
        .output()
        .unwrap();
    assert_eq!(plain_table.status.code(), Some(0));
    let parsed_table: toml::Table =
        toml::from_str(&String::from_utf8_lossy(&plain_table.stdout)).unwrap();
    assert_eq!(parsed_table["mode"].as_str(), Some("editor"));
    assert_eq!(parsed_table["placement"].as_str(), Some("pane-right"));

    // Atomic model set and rejection of partial model writes.
    let (code, _) = f.run(
        &["config", "set", "pi.reporting.model.provider", "openai"],
        None,
    );
    assert_ne!(code, 0);
    assert_eq!(
        f.ok(&["config", "set", "pi.reporting.model", "openai/gpt-5"]),
        json!({"provider": "openai", "id": "gpt-5"})
    );
    assert_eq!(
        f.ok(&["config", "set", "pi.reporting.model.id", "gpt-5-mini"]),
        "gpt-5-mini"
    );
    assert_eq!(
        f.ok(&["config", "get", "pi.reporting.model"]),
        json!({"provider": "openai", "id": "gpt-5-mini"})
    );
    let (code, _) = f.run(&["config", "unset", "pi.reporting.model.provider"], None);
    assert_ne!(code, 0);
    assert_eq!(
        f.ok(&["config", "unset", "pi.reporting.model"]),
        Value::Null
    );
    let (code, _) = f.run(&["config", "get", "pi.reporting.model"], None);
    assert_ne!(code, 0);

    // Typed set/unset + --overrides.
    assert_eq!(
        f.ok(&["config", "set", "herdr.viewer.mode", "native"]),
        "native"
    );
    assert_eq!(
        f.ok(&["config", "set", "herdr.viewer.placement", "floating"]),
        "floating"
    );
    assert_eq!(
        f.ok(&["config", "set", "pi.reporting.frequency", "4"]),
        json!(4)
    );
    assert_eq!(
        f.ok(&["config", "set", "pi.naming.session", "false"]),
        json!(false)
    );
    assert_eq!(
        f.ok(&["config", "list", "--overrides"]),
        json!({
            "herdr": {"viewer": {"mode": "native", "placement": "floating"}},
            "pi": {"naming": {"session": false}, "reporting": {"frequency": 4}}
        })
    );
    assert_eq!(f.ok(&["config", "unset", "herdr.viewer.mode"]), Value::Null);
    assert_eq!(f.ok(&["config", "get", "herdr.viewer.mode"]), "editor");
}

#[test]
fn a_save_without_a_usable_frontend_context_notifies_nothing_and_stays_quiet() {
    let f = Fixture::new();
    f.init();
    for environment in [
        &[
            ("CMUX_WORKSPACE_ID", "w"),
            ("HERDR_ENV", "1"),
            ("HERDR_PANE_ID", "p"),
        ][..],
        &[("HERDR_ENV", "1")][..],
    ] {
        let output = f
            .command()
            .envs(environment.iter().copied())
            .args(["create", "Quiet", "--json"])
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{environment:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "",
            "{environment:?}"
        );
    }
}
