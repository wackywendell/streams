use super::*;
use crate::{
    fake_herdr as herdr,
    runtime::{conversations::IDENTITY_FILE_OPENS, state::Association},
};
use serde_json::json;

struct Fixture {
    directory: tempfile::TempDir,
    server: herdr::FakeHerdr,
    client: HerdrClient,
    snapshot: HerdrNativeSnapshot,
    transcript: PathBuf,
}

impl Fixture {
    fn new(neighbors: usize) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("pi-caller-")
            .tempdir_in("/tmp")
            .unwrap();
        let server = herdr::FakeHerdr::start(&directory.path().join("herdr.sock"), |method, _| {
            panic!("unexpected native request: {method}");
        });
        let environment = BTreeMap::from([
            ("HERDR_ENV".into(), "1".into()),
            ("HERDR_PANE_ID".into(), "w:p0".into()),
            ("HERDR_SESSION".into(), "test".into()),
            (
                "HERDR_SOCKET_PATH".into(),
                server.socket.to_str().unwrap().into(),
            ),
        ]);
        let client = HerdrClient::connect(environment, 1.0).unwrap();
        let transcript = directory.path().join("owner.jsonl");
        std::fs::write(&transcript, "{\"type\":\"session\",\"id\":\"native\"}\n").unwrap();
        let mut panes = Vec::new();
        for index in 0..=neighbors {
            let path = if index == 0 {
                transcript.clone()
            } else {
                let path = directory.path().join(format!("neighbor-{index}.jsonl"));
                if index != neighbors {
                    std::fs::write(
                        &path,
                        format!("{{\"type\":\"session\",\"id\":\"other-{index}\"}}\n"),
                    )
                    .unwrap();
                }
                path
            };
            panes.push(json!({
                "pane_id":format!("w:p{index}"),"terminal_id":format!("term-{index}"),
                "workspace_id":"w","tab_id":"w:t1","agent":"pi",
                "agent_session":{"agent":"pi","kind":"path","value":path}
            }));
        }
        let snapshot = HerdrNativeSnapshot::parse(&json!({
            "workspaces":[{"workspace_id":"w","label":"Working","number":1}],
            "tabs":[{"workspace_id":"w","tab_id":"w:t1","label":"Working","number":1}],
            "panes":panes,"agents":panes,
            "layouts":[{"workspace_id":"w","tab_id":"w:t1","panes":panes.iter().map(|row| json!({"pane_id":row["pane_id"]})).collect::<Vec<_>>()}]
        })).unwrap();
        Self {
            directory,
            server,
            client,
            snapshot,
            transcript,
        }
    }

    fn context(&self) -> (RuntimeState, Stream, Session, Authority) {
        let state = RuntimeState::new(
            self.directory
                .path()
                .canonicalize()
                .unwrap()
                .join("runtime"),
            "host",
        )
        .unwrap();
        let endpoint = self.client.endpoint();
        let scope = herdr_scope(&endpoint.socket, endpoint.session.as_deref());
        state
            .stream_lock(Some(&scope), "effort")
            .unwrap()
            .write_association(&Association::new("effort", "w", None, true).unwrap())
            .unwrap();
        (
            state,
            Stream::new("effort", "Effort", None, "host").unwrap(),
            Session::new("card", "host", "effort", "pi", "native", "Conversation").unwrap(),
            Authority {
                host_id: "host".into(),
                token: "creation".into(),
            },
        )
    }
}

fn opened() -> Vec<PathBuf> {
    IDENTITY_FILE_OPENS.with(|files| std::mem::take(&mut *files.borrow_mut()))
}

#[test]
fn owner_observation_reads_one_transcript_not_unreadable_neighbors() {
    let fixture = Fixture::new(64);
    let (state, stream, session, authority) = fixture.context();
    opened();
    let observed =
        PiCallerObservation::from_snapshot(&fixture.client, fixture.snapshot.clone(), "native")
            .unwrap();
    let caller = observed.caller();
    assert_eq!(caller.pane_id(), "w:p0");
    assert_eq!(caller.terminal_id(), "term-0");
    assert_eq!(caller.endpoint(), fixture.client.endpoint());
    assert_eq!(
        caller.locator(),
        Some(fixture.transcript.canonicalize().unwrap().as_path())
    );
    assert_eq!(observed.snapshot(), &fixture.snapshot);
    let workspace = observed.calling_workspace();
    assert_eq!(workspace.workspace_id(), caller.workspace_id());
    assert_eq!(
        workspace.scope(),
        herdr_scope(
            &caller.endpoint().socket,
            caller.endpoint().session.as_deref()
        )
    );
    assert_eq!(
        workspace
            .context_stream_id(&state, None)
            .unwrap()
            .as_deref(),
        Some("effort")
    );
    for _ in 0..3 {
        let attested =
            attest_context_from_caller(&observed, &state, &stream, &session, Some(&authority))
                .unwrap();
        assert_eq!(attested.terminal_id(), "term-0");
    }
    assert_eq!(opened(), vec![fixture.transcript.canonicalize().unwrap()]);
    assert_eq!(fixture.server.methods(), ["ping"]);
}

#[test]
fn unchanged_snapshot_paths_are_resolved_again_for_each_observation() {
    let fixture = Fixture::new(0);
    opened();
    let first =
        PiCallerObservation::from_snapshot(&fixture.client, fixture.snapshot.clone(), "native")
            .unwrap();
    std::fs::write(
        &fixture.transcript,
        "{\"type\":\"session\",\"id\":\"different\"}\n",
    )
    .unwrap();
    assert!(
        PiCallerObservation::from_snapshot(&fixture.client, fixture.snapshot.clone(), "native")
            .is_err()
    );
    assert_eq!(first.caller().native_session_id(), "native");
    assert_eq!(
        opened(),
        vec![fixture.transcript.canonicalize().unwrap(); 2]
    );
}

#[test]
fn attesting_the_resolved_caller_preserves_record_and_endpoint_fences() {
    let fixture = Fixture::new(0);
    let (state, stream, mut session, authority) = fixture.context();
    opened();
    let observed =
        PiCallerObservation::from_snapshot(&fixture.client, fixture.snapshot.clone(), "native")
            .unwrap();
    assert!(attest_context_from_caller(&observed, &state, &stream, &session, None).is_err());
    session.harness_session_id = "helper".into();
    assert!(
        attest_context_from_caller(&observed, &state, &stream, &session, Some(&authority)).is_err()
    );
    session.harness_session_id = "native".into();
    session.host_id = "other-host".into();
    assert!(
        attest_context_from_caller(&observed, &state, &stream, &session, Some(&authority)).is_err()
    );
    assert_eq!(opened(), vec![fixture.transcript.canonicalize().unwrap()]);
}

#[test]
fn the_same_workspace_id_in_another_endpoint_namespace_is_not_attested() {
    let fixture = Fixture::new(0);
    let (state, stream, session, authority) = fixture.context();
    let other = HerdrClient::connect(
        BTreeMap::from([
            ("HERDR_ENV".into(), "1".into()),
            ("HERDR_PANE_ID".into(), "w:p0".into()),
            ("HERDR_SESSION".into(), "other".into()),
            (
                "HERDR_SOCKET_PATH".into(),
                fixture.server.socket.to_str().unwrap().into(),
            ),
        ]),
        1.0,
    )
    .unwrap();
    opened();
    let observed =
        PiCallerObservation::from_snapshot(&other, fixture.snapshot.clone(), "native").unwrap();
    assert_eq!(
        observed.caller().endpoint().session.as_deref(),
        Some("other")
    );
    assert_eq!(observed.calling_workspace().workspace_id(), "w");
    assert!(
        attest_context_from_caller(&observed, &state, &stream, &session, Some(&authority)).is_err()
    );
    assert_eq!(opened(), vec![fixture.transcript.canonicalize().unwrap()]);
}

#[test]
fn a_neighbor_identity_never_substitutes_for_the_exact_caller() {
    let fixture = Fixture::new(2);
    opened();
    assert!(
        PiCallerObservation::from_snapshot(&fixture.client, fixture.snapshot.clone(), "other-1")
            .is_err()
    );
    assert_eq!(opened(), vec![fixture.transcript.canonicalize().unwrap()]);
    let mut contradictory = fixture.snapshot.raw().clone();
    contradictory["panes"][0]["agent_session"]["value"] = json!("/unreadable-owner");
    let contradictory = HerdrNativeSnapshot::parse(&contradictory).unwrap();
    assert!(PiCallerObservation::from_snapshot(&fixture.client, contradictory, "native").is_err());
    assert!(opened().is_empty());
}
