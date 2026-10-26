use super::*;
use crate::{
    fake_herdr,
    frontend::herdr::{HerdrClient, HerdrNativeSnapshot},
    model::{NamingMode, Stream},
    runtime::{
        conversations::IDENTITY_FILE_OPENS,
        digest,
        placement::HerdrPlacementObservation,
        state::{NativePlacement, PlacementBinding, herdr_scope},
    },
};
use serde_json::json;
use std::path::PathBuf;

const CODEX: &str = "12345678-1234-5678-9abc-1234567890ab";

struct Fixture {
    directory: tempfile::TempDir,
    _server: fake_herdr::FakeHerdr,
    client: HerdrClient,
    snapshot: HerdrNativeSnapshot,
    scope: String,
    placement: NativePlacement,
    stream: Stream,
    owner: PathBuf,
    neighbor: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::Builder::new()
            .prefix("capture-inputs-")
            .tempdir_in("/tmp")
            .unwrap();
        let server =
            fake_herdr::FakeHerdr::start(&directory.path().join("herdr.sock"), |method, _| {
                panic!("unexpected native request: {method}")
            });
        let client = HerdrClient::connect(
            BTreeMap::from([
                ("HERDR_ENV".into(), "1".into()),
                ("HERDR_PANE_ID".into(), "owner".into()),
                ("HERDR_SESSION".into(), "test".into()),
                (
                    "HERDR_SOCKET_PATH".into(),
                    server.socket.to_str().unwrap().into(),
                ),
            ]),
            1.0,
        )
        .unwrap();
        let scope = herdr_scope(&server.socket, Some("test"));
        let owner = directory.path().join("owner.jsonl");
        let neighbor = directory.path().join("neighbor.jsonl");
        for (path, id) in [(&owner, "native-owner"), (&neighbor, "native-neighbor")] {
            std::fs::write(path, json!({"type":"session","id":id}).to_string() + "\n").unwrap();
        }
        let panes = json!([
            {"pane_id":"neighbor","terminal_id":"neighbor-terminal","workspace_id":"workspace","tab_id":"second",
             "agent":"pi","agent_session":{"agent":"pi","kind":"path","value":neighbor}},
            {"pane_id":"codex","terminal_id":"codex-terminal","workspace_id":"workspace","tab_id":"second",
             "agent":"codex","agent_session":{"agent":"codex","kind":"id","value":CODEX}},
            {"pane_id":"outside","terminal_id":"outside-terminal","workspace_id":"other","tab_id":"elsewhere",
             "agent":"pi","agent_session":{"agent":"pi","kind":"path","value":directory.path().join("unreadable-outside")}},
            {"pane_id":"owner","terminal_id":"owner-terminal","workspace_id":"workspace","tab_id":"first",
             "agent":"pi","agent_session":{"agent":"pi","kind":"path","value":owner}},
            {"pane_id":"card","terminal_id":"card-terminal","workspace_id":"workspace","tab_id":"first"}
        ]);
        let agents: Vec<_> = panes
            .as_array()
            .unwrap()
            .iter()
            .filter(|pane| pane.get("agent").is_some())
            .cloned()
            .collect();
        let snapshot = HerdrNativeSnapshot::parse(&json!({
            "workspaces":[
                {"workspace_id":"workspace","label":"Work","number":1,"tokens":{
                    "strm_catalog":digest(&serde_json::to_vec("catalog").unwrap()),
                    "strm_host":"host","strm_stream":"stream","strm_binding":"lifetime"}},
                {"workspace_id":"other","label":"Other","number":2}
            ],
            "tabs":[
                {"workspace_id":"workspace","tab_id":"first","label":"Owner work","number":1},
                {"workspace_id":"workspace","tab_id":"second","label":"2","number":2},
                {"workspace_id":"other","tab_id":"elsewhere","label":"Other","number":1}
            ],
            "panes":panes,"agents":agents,
            "layouts":[
                {"workspace_id":"workspace","tab_id":"first","panes":[{"pane_id":"owner"},{"pane_id":"card"}]},
                {"workspace_id":"workspace","tab_id":"second","panes":[{"pane_id":"codex"},{"pane_id":"neighbor"}]},
                {"workspace_id":"other","tab_id":"elsewhere","panes":[{"pane_id":"outside"}]}
            ]
        })).unwrap();
        let placement = NativePlacement::new(
            "stream",
            &scope,
            PlacementBinding::herdr("workspace", "lifetime").unwrap(),
        )
        .unwrap();
        let stream = Stream::new("stream", "Work", None, "host").unwrap();
        Self {
            directory,
            _server: server,
            client,
            snapshot,
            scope,
            placement,
            stream,
            owner,
            neighbor,
        }
    }
    fn caller(&self) -> PiCallerObservation {
        PiCallerObservation::from_snapshot(&self.client, self.snapshot.clone(), "native-owner")
            .unwrap()
    }
    fn ready<'a>(&'a self, snapshot: &'a HerdrNativeSnapshot) -> ReadyHerdrPlacement<'a> {
        HerdrPlacementObservation::new("catalog", "host", &self.scope, &self.placement, snapshot)
            .unwrap()
            .require_ready()
            .unwrap()
    }
    fn sessions(&self) -> BTreeMap<String, Option<Session>> {
        [
            ("pi", "native-owner"),
            ("pi", "native-neighbor"),
            ("codex", CODEX),
        ]
        .into_iter()
        .map(|(harness, native)| {
            let id = Session::id_for("host", harness, native);
            (
                id.clone(),
                Some(Session::new(id, "host", "stream", harness, native, "Work").unwrap()),
            )
        })
        .collect()
    }
}

fn opens() -> Vec<PathBuf> {
    IDENTITY_FILE_OPENS.with(|files| files.borrow().clone())
}
fn clear_opens() {
    IDENTITY_FILE_OPENS.with(|files| files.borrow_mut().clear());
}

#[test]
fn caller_capture_reuses_owner_identity_and_plans_without_transcript_or_native_rereads() {
    let f = Fixture::new();
    clear_opens();
    let caller = f.caller();
    let owner_path = f.owner.canonicalize().unwrap();
    let neighbor_path = f.neighbor.canonicalize().unwrap();
    assert_eq!(opens().as_slice(), std::slice::from_ref(&owner_path));
    let observed =
        WorkspaceCaptureObservation::from_caller(&f.stream, f.ready(caller.snapshot()), &caller)
            .unwrap();
    assert_eq!(opens(), [owner_path.clone(), neighbor_path]);
    let sessions = f.sessions();
    assert_eq!(observed.session_keys(), &sessions.keys().cloned().collect());
    std::fs::rename(&f.owner, f.directory.path().join("old-owner")).unwrap();
    std::fs::rename(&f.neighbor, f.directory.path().join("old-neighbor")).unwrap();
    let before = opens();
    for _ in 0..2 {
        let plan = observed.plan(&sessions).unwrap();
        assert_eq!(plan.workspace_id(), "workspace");
        assert_eq!(plan.stream_id(), "stream");
        assert_eq!(plan.tabs().len(), 2);
        assert_eq!(plan.tabs()[0].label(), Some("Owner work"));
        assert_eq!(plan.tabs()[0].label_mode(), NamingMode::Manual);
        assert_eq!(plan.tabs()[0].sessions()[0].locator(), owner_path.to_str());
        assert_eq!(plan.tabs()[0].sessions()[0].pane_id(), Some("owner"));
        assert_eq!(plan.tabs()[1].label(), None);
        assert_eq!(plan.tabs()[1].label_mode(), NamingMode::Automatic);
        assert_eq!(
            plan.tabs()[1]
                .sessions()
                .iter()
                .map(|s| s.harness())
                .collect::<Vec<_>>(),
            ["codex", "pi"]
        );
    }
    assert_eq!(opens(), before);
}

#[test]
fn explicit_inventory_names_only_the_observed_workspaces_supported_conversations() {
    let f = Fixture::new();
    clear_opens();
    let observed = WorkspaceCaptureObservation::observe(&f.stream, f.ready(&f.snapshot)).unwrap();
    let opened = opens();
    assert_eq!(opened.len(), 2);
    assert!(opened.contains(&f.owner.canonicalize().unwrap()));
    assert!(opened.contains(&f.neighbor.canonicalize().unwrap()));
    assert_eq!(observed.session_keys().len(), 3);
    assert!(
        !observed
            .session_keys()
            .contains(&Session::id_for("host", "pi", "outside"))
    );
    assert_eq!(observed.plan(&f.sessions()).unwrap().tabs().len(), 2);
}

#[test]
fn missing_extra_or_wrong_named_record_inputs_refuse_instead_of_faking_a_roster() {
    let f = Fixture::new();
    let caller = f.caller();
    let observed =
        WorkspaceCaptureObservation::from_caller(&f.stream, f.ready(caller.snapshot()), &caller)
            .unwrap();
    let key = Session::id_for("host", "pi", "native-owner");
    let mut sessions = f.sessions();
    let actual = sessions.remove(&key).unwrap();
    assert!(observed.plan(&sessions).is_err());
    sessions.insert(key.clone(), actual.clone());
    sessions.insert("unrequested".into(), None);
    assert!(observed.plan(&sessions).is_err());
    sessions.remove("unrequested");
    for field in ["id", "host", "harness", "native"] {
        let mut wrong = actual.clone().unwrap();
        match field {
            "id" => wrong.id = "wrong".into(),
            "host" => wrong.host_id = "foreign".into(),
            "harness" => wrong.harness = "claude".into(),
            "native" => wrong.harness_session_id = "different".into(),
            _ => unreachable!(),
        }
        sessions.insert(key.clone(), Some(wrong));
        assert!(observed.plan(&sessions).is_err(), "accepted wrong {field}");
    }
}

#[test]
fn actual_moved_membership_and_unenrolled_agents_are_omitted_not_retargeted() {
    let f = Fixture::new();
    let observed = WorkspaceCaptureObservation::observe(&f.stream, f.ready(&f.snapshot)).unwrap();
    let mut sessions = f.sessions();
    sessions
        .get_mut(&Session::id_for("host", "pi", "native-neighbor"))
        .unwrap()
        .as_mut()
        .unwrap()
        .stream_id = "different".into();
    sessions.insert(Session::id_for("host", "codex", CODEX), None);
    let plan = observed.plan(&sessions).unwrap();
    assert_eq!(plan.tabs().len(), 1);
    assert_eq!(plan.tabs()[0].sessions().len(), 1);
    assert_eq!(plan.tabs()[0].sessions()[0].native_id(), "native-owner");
    let absent = observed
        .session_keys()
        .iter()
        .map(|key| (key.clone(), None))
        .collect();
    assert!(observed.plan(&absent).unwrap().tabs().is_empty());
}

#[test]
fn equal_json_from_another_snapshot_cannot_borrow_the_callers_identity() {
    let f = Fixture::new();
    let caller = f.caller();
    let detached = caller.snapshot().clone();
    clear_opens();
    assert!(
        WorkspaceCaptureObservation::from_caller(&f.stream, f.ready(&detached), &caller).is_err()
    );
    assert!(opens().is_empty());
}

#[test]
fn actual_stream_and_caller_endpoint_admission_precede_identity_opens() {
    let f = Fixture::new();
    for stream in [
        Stream::new("stream", "Work", None, "foreign").unwrap(),
        Stream::new("other", "Work", None, "host").unwrap(),
    ] {
        clear_opens();
        assert!(WorkspaceCaptureObservation::observe(&stream, f.ready(&f.snapshot)).is_err());
        assert!(opens().is_empty());
    }
    let caller = f.caller();
    let wrong_scope = "a".repeat(64);
    let other = NativePlacement::new(
        "stream",
        &wrong_scope,
        PlacementBinding::herdr("workspace", "lifetime").unwrap(),
    )
    .unwrap();
    let ready =
        HerdrPlacementObservation::new("catalog", "host", &wrong_scope, &other, caller.snapshot())
            .unwrap()
            .require_ready()
            .unwrap();
    clear_opens();
    assert!(WorkspaceCaptureObservation::from_caller(&f.stream, ready, &caller).is_err());
    assert!(opens().is_empty());
}

#[test]
fn incomplete_metadata_refuses_before_inventory_and_concerned_bad_identity_fails_closed() {
    let f = Fixture::new();
    let mut raw = f.snapshot.raw().clone();
    raw["workspaces"][0]["tokens"]
        .as_object_mut()
        .unwrap()
        .remove("strm_binding");
    let partial = HerdrNativeSnapshot::parse(&raw).unwrap();
    clear_opens();
    assert!(
        HerdrPlacementObservation::new("catalog", "host", &f.scope, &f.placement, &partial)
            .unwrap()
            .require_ready()
            .is_err()
    );
    assert!(opens().is_empty());
    let caller = f.caller();
    std::fs::write(&f.neighbor, "corrupt identity\n").unwrap();
    assert!(
        WorkspaceCaptureObservation::from_caller(&f.stream, f.ready(caller.snapshot()), &caller)
            .is_err()
    );
}
