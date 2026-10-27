use super::*;
use crate::{
    fake_herdr::{FakeHerdr, Reply},
    runtime::checkouts::CheckoutRegistry,
};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    sync::{Arc, atomic::AtomicUsize},
};

struct Fixture {
    temp: tempfile::TempDir,
    runtime_root: PathBuf,
    locations: DirectoryBindings,
    frontend: BuiltinFrontend,
    server: FakeHerdr,
    environment: BTreeMap<String, String>,
    roots: ConversationRoots,
    cancelled: AtomicBool,
    stream: Stream,
}
impl Fixture {
    fn new(handler: impl Fn(&str, &Value) -> Reply + Send + Sync + 'static) -> Self {
        let temp = tempfile::Builder::new()
            .prefix("reopen-")
            .tempdir_in("/tmp")
            .unwrap();
        let server = FakeHerdr::start(&temp.path().join("herdr.sock"), handler);
        let client = crate::frontend::herdr::HerdrClient::connect(
            BTreeMap::from([
                ("HERDR_ENV".into(), "1".into()),
                ("HERDR_PANE_ID".into(), "owner".into()),
                (
                    "HERDR_SOCKET_PATH".into(),
                    server.socket.to_str().unwrap().into(),
                ),
            ]),
            1.0,
        )
        .unwrap();
        let runtime_root = temp.path().join("runtime");
        let locations = DirectoryBindings::new(
            &runtime_root,
            CheckoutRegistry::new(temp.path().join("checkouts"), "host").unwrap(),
        );
        let environment = BTreeMap::from([("HOME".into(), temp.path().to_str().unwrap().into())]);
        Self {
            temp,
            runtime_root,
            locations,
            frontend: BuiltinFrontend::Herdr(client),
            server,
            environment,
            roots: ConversationRoots::default(),
            cancelled: AtomicBool::new(false),
            stream: Stream::new("stream", "Work", None, "host").unwrap(),
        }
    }
    fn runtime(&self) -> WorkspaceRuntime<'_> {
        WorkspaceRuntime::new(
            "host",
            &self.locations,
            Some(&self.frontend),
            "catalog",
            &self.roots,
            &self.environment,
        )
    }
    fn session(&self, id: &str) -> (Session, PathBuf) {
        let path = self.temp.path().join(format!("{id}.jsonl"));
        fs::write(&path, json!({"type":"session","id":id}).to_string() + "\n").unwrap();
        let session = Session::new(
            Session::id_for("host", "pi", id),
            "host",
            "stream",
            "pi",
            id,
            id,
        )
        .unwrap();
        self.locations
            .remember(
                "stream",
                self.temp.path(),
                Some(&session.id),
                false,
                None,
                None,
            )
            .unwrap();
        (session, path)
    }
    fn assert_no_saved_native_addresses(&self) {
        fn check(value: &Value) {
            match value {
                Value::Object(object) => {
                    for (key, value) in object {
                        assert!(
                            !["workspace_id", "tab_id", "pane_id", "nonce", "pending"]
                                .contains(&key.as_str()),
                            "{key}"
                        );
                        check(value);
                    }
                }
                Value::Array(values) => {
                    for value in values {
                        check(value);
                    }
                }
                _ => {}
            }
        }
        let mut directories = vec![self.runtime_root.clone()];
        while let Some(path) = directories.pop() {
            if !path.exists() {
                continue;
            }
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                if entry.file_type().unwrap().is_dir() {
                    directories.push(entry.path());
                } else if entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "lock")
                {
                    assert_eq!(entry.metadata().unwrap().len(), 0);
                } else {
                    check(
                        &serde_json::from_slice::<Value>(&fs::read(entry.path()).unwrap()).unwrap(),
                    );
                }
            }
        }
    }
}
fn empty_snapshot() -> Value {
    json!({"type":"session_snapshot","snapshot":{"workspaces":[],"tabs":[],"panes":[],"agents":[],"layouts":[]}})
}
fn bound_snapshot() -> Value {
    json!({"workspaces":[{"workspace_id":"workspace","label":"Work","number":1,
        "tokens":stream_tokens("catalog", "host", "stream")}],
        "tabs":[],"panes":[],"agents":[],"layouts":[]})
}
fn created() -> Value {
    json!({"type":"workspace_created","workspace":{"workspace_id":"created","label":"Work","number":1},
        "tab":{"tab_id":"created-tab","workspace_id":"created"},
        "root_pane":{"pane_id":"created-pane","tab_id":"created-tab","workspace_id":"created"}})
}

#[test]
fn foreign_or_cancelled_open_never_observes_or_saves_native_state() {
    let f = Fixture::new(|method, _| panic!("unexpected {method}"));
    let foreign = Stream::new("stream", "Work", None, "foreign").unwrap();
    assert!(
        f.runtime()
            .open(&f.cancelled, &foreign, &[], None, None)
            .is_err()
    );
    f.cancelled.store(true, Ordering::Release);
    assert!(
        f.runtime()
            .open(&f.cancelled, &f.stream, &[], None, None)
            .is_err()
    );
    assert!(!f.runtime_root.exists());
    assert_eq!(f.server.methods(), ["ping"]);
}

#[test]
fn lost_launch_or_metadata_reply_stops_once_without_a_journal_or_automatic_retry() {
    for lose_metadata in [false, true] {
        let actual_creations = Arc::new(AtomicUsize::new(0));
        let actual = actual_creations.clone();
        let f = Fixture::new(move |method, params| match method {
            "events.subscribe" => Reply::Result(json!({"type":"subscription_started"})),
            "session.snapshot" => Reply::Result(empty_snapshot()),
            "workspace.create" => {
                actual.fetch_add(1, Ordering::Relaxed);
                if lose_metadata {
                    Reply::Result(created())
                } else {
                    Reply::Raw(vec![])
                }
            }
            "workspace.report_metadata" => {
                assert_eq!(params["source"], "streams");
                assert_eq!(
                    params["tokens"],
                    json!(stream_tokens("catalog", "host", "stream"))
                );
                Reply::Raw(vec![])
            }
            _ => panic!("unexpected {method}"),
        });
        assert!(
            f.runtime()
                .open(&f.cancelled, &f.stream, &[], Some(f.temp.path()), None)
                .is_err()
        );
        assert_eq!(actual_creations.load(Ordering::Relaxed), 1);
        assert_eq!(
            f.server
                .methods()
                .iter()
                .filter(|method| *method == "session.snapshot")
                .count(),
            1
        );
        f.assert_no_saved_native_addresses();
        // A later explicit request is allowed to observe and try again.
        assert!(
            f.runtime()
                .open(&f.cancelled, &f.stream, &[], Some(f.temp.path()), None)
                .is_err()
        );
        assert_eq!(actual_creations.load(Ordering::Relaxed), 2);
        f.assert_no_saved_native_addresses();
    }
}

#[test]
fn current_selector_snapshot_is_reused_and_running_elsewhere_is_only_reported() {
    let f = Fixture::new(|method, _| match method {
        "workspace.focus" => Reply::Result(json!({"type":"ok"})),
        _ => panic!("unexpected {method}"),
    });
    let (session, _) = f.session("running");
    let mut value = bound_snapshot();
    value["workspaces"].as_array_mut().unwrap().push(json!({
        "workspace_id":"elsewhere","label":"Elsewhere","number":2
    }));
    value["tabs"] =
        json!([{"tab_id":"other-tab","workspace_id":"elsewhere","label":"Other","number":1}]);
    value["panes"] = json!([{"pane_id":"other-pane","terminal_id":"other-terminal",
        "workspace_id":"elsewhere","tab_id":"other-tab","agent":"pi",
        "agent_session":{"agent":"pi","kind":"id","value":"running"}}]);
    value["agents"] = value["panes"].clone();
    value["layouts"] = json!([{"workspace_id":"elsewhere","tab_id":"other-tab","panes":[{"pane_id":"other-pane"}]}]);
    let snapshot = HerdrNativeSnapshot::parse(&value).unwrap();
    let result = f
        .runtime()
        .open(
            &f.cancelled,
            &f.stream,
            &[SelectedConversation {
                session: &session,
                locator: None,
            }],
            None,
            Some(&snapshot),
        )
        .unwrap();
    assert!(result.restored.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert!(result.skipped[0].reason.contains("elsewhere"));
    assert_eq!(f.server.methods(), ["ping", "workspace.focus"]);
    f.assert_no_saved_native_addresses();
}

#[test]
fn reopen_launches_one_tab_per_requested_session_without_saved_addresses() {
    let tabs = Arc::new(AtomicUsize::new(0));
    let count = tabs.clone();
    let f = Fixture::new(move |method, params| {
        Reply::Result(match method {
            "tab.create" => {
                let n = count.fetch_add(1, Ordering::Relaxed);
                json!({"type":"tab_created","tab":{"tab_id":format!("tab-{n}"),"workspace_id":"workspace"},
                "root_pane":{"pane_id":format!("pane-{n}"),"tab_id":format!("tab-{n}"),"workspace_id":"workspace"}})
            }
            "agent.start" => {
                let pane = params["pane_id"].as_str().unwrap();
                let n = pane.strip_prefix("pane-").unwrap();
                json!({"type":"agent_started","agent":{"pane_id":pane,"tab_id":format!("tab-{n}"),"workspace_id":"workspace",
                "name":params["name"],"agent":params["kind"],"interactive_ready":true,"terminal_id":format!("terminal-{n}")}})
            }
            "workspace.focus" => json!({"type":"ok"}),
            _ => panic!("unexpected {method}"),
        })
    });
    let snapshot = HerdrNativeSnapshot::parse(&bound_snapshot()).unwrap();
    let (one, first) = f.session("one");
    let (two, second) = f.session("two");
    let bindings: Vec<_> = [&one, &two]
        .into_iter()
        .map(|session| {
            let path = f.locations.path(&f.stream.id, Some(&session.id));
            let before = fs::metadata(&path).unwrap().ino();
            (path, before)
        })
        .collect();
    let result = f
        .runtime()
        .open(
            &f.cancelled,
            &f.stream,
            &[
                SelectedConversation {
                    session: &one,
                    locator: Some(&first),
                },
                SelectedConversation {
                    session: &two,
                    locator: Some(&second),
                },
            ],
            None,
            Some(&snapshot),
        )
        .unwrap();
    assert_eq!(result.restored, [one.id, two.id]);
    assert_eq!(tabs.load(Ordering::Relaxed), 2);
    for (path, before) in bindings {
        assert_eq!(
            fs::metadata(path).unwrap().ino(),
            before,
            "opening with saved intent must not replace a binding"
        );
    }
    assert!(f.server.methods().iter().all(|method| matches!(
        method.as_str(),
        "ping" | "tab.create" | "agent.start" | "workspace.focus"
    )));
    f.assert_no_saved_native_addresses();
}

#[test]
fn remembered_directory_is_preserved_unless_the_caller_supplies_a_replacement() {
    let f = Fixture::new(|method, _| panic!("unexpected {method}"));
    let (session, _) = f.session("directory");
    let original = f
        .locations
        .read(&f.stream.id, Some(&session.id))
        .unwrap()
        .unwrap();
    let new = f.temp.path().join("replacement");
    fs::create_dir(&new).unwrap();
    let new = new.canonicalize().unwrap();
    f.runtime()
        .remember(&f.stream, Some(&session), &new, true)
        .unwrap();
    assert_eq!(
        f.locations
            .read(&f.stream.id, Some(&session.id))
            .unwrap()
            .unwrap(),
        original
    );
    f.runtime()
        .remember(&f.stream, Some(&session), &new, false)
        .unwrap();
    assert_eq!(
        f.locations
            .read(&f.stream.id, Some(&session.id))
            .unwrap()
            .unwrap()
            .cwd,
        new
    );
}

#[test]
fn resume_prepares_only_the_exact_locator_without_a_native_guard() {
    let f = Fixture::new(|method, _| panic!("unexpected {method}"));
    let (session, file) = f.session("resume");
    let runtime = WorkspaceRuntime::new(
        "host",
        &f.locations,
        None,
        "catalog",
        &f.roots,
        &f.environment,
    );
    assert!(
        runtime
            .resume(
                &f.stream,
                SelectedConversation {
                    session: &session,
                    locator: None
                },
                None,
                None
            )
            .is_err()
    );
    let command = runtime
        .resume(
            &f.stream,
            SelectedConversation {
                session: &session,
                locator: Some(&file),
            },
            None,
            None,
        )
        .unwrap();
    assert_eq!(command.command().locator(), file.canonicalize().unwrap());
    assert_eq!(command.cwd(), f.temp.path().canonicalize().unwrap());
    assert_eq!(f.server.methods(), ["ping"]);
    f.assert_no_saved_native_addresses();
}
