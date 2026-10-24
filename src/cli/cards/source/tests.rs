use super::*;
use crate::cli::{Input, Operation};
use serde_json::json;
use std::{collections::BTreeMap, fs};
use streams_temper::{
    agent,
    config::{Config, Strategy},
    model::{Record, Session, Stream},
    operations::Actor,
};

#[test]
fn native_observation_reuses_its_proof_and_card_source_resolves_each_new_read() {
    use streams_temper::{
        frontend::herdr::{HerdrClient, HerdrNativeSnapshot},
        runtime::{
            conversations::Harness, native::PiCallerObservation, native_cards::NativeCardSource,
            state::Association,
        },
    };

    let temp = tempfile::Builder::new()
        .prefix("proof-wiring-")
        .tempdir_in("/tmp")
        .unwrap();
    let cwd = fs::canonicalize(temp.path()).unwrap();
    let server = crate::fake_herdr::FakeHerdr::start(&cwd.join("herdr.sock"), |method, _| {
        panic!("unexpected native request: {method}");
    });
    let environment = BTreeMap::from([
        ("HOME".into(), cwd.to_str().unwrap().into()),
        ("HERDR_ENV".into(), "1".into()),
        ("HERDR_PANE_ID".into(), "w:p1".into()),
        ("HERDR_SESSION".into(), "test".into()),
        (
            "HERDR_SOCKET_PATH".into(),
            server.socket.to_str().unwrap().into(),
        ),
    ]);
    let app = App {
        config: Config::new(&environment, &cwd).unwrap(),
        environment,
        cwd: cwd.clone(),
        input: Input::Stdin,
        herdr: None,
    };
    let opened = application::initialize(&app.config, Strategy::None, None, "main", false).unwrap();
    let stream = Stream::new("effort", "Effort", None, opened.host().as_str()).unwrap();
    let session = Session::new(
        agent::session_id_for(opened.host().as_str(), Harness::Pi, "native"),
        opened.host().as_str(),
        "effort",
        "pi",
        "native",
        "Conversation",
    )
    .unwrap();
    let create = Operation::new(
        "create",
        None,
        Record::Stream(stream),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(opened.backend().save(&create, None).unwrap().accepted());
    let authority = opened.backend().authority("effort").unwrap().unwrap();
    let attach = Operation::new(
        "attach",
        None,
        Record::Session(session.clone()),
        Actor::User,
        Some(authority.token),
        vec![],
    )
    .unwrap();
    assert!(opened.backend().save(&attach, None).unwrap().accepted());
    let client = HerdrClient::connect(app.environment.clone(), 1.0).unwrap();
    let transcript = cwd.join("owner.jsonl");
    let original = "{\"type\":\"session\",\"id\":\"native\"}\n";
    fs::write(&transcript, original).unwrap();
    let pane = json!({"pane_id":"w:p1","terminal_id":"term1","workspace_id":"w","tab_id":"w:t1","agent":"pi",
        "agent_session":{"agent":"pi","kind":"path","value":transcript}});
    let snapshot = HerdrNativeSnapshot::parse(&json!({
        "workspaces":[{"workspace_id":"w","label":"Working","number":1}],
        "tabs":[{"workspace_id":"w","tab_id":"w:t1","label":"Working","number":1}],
        "panes":[pane],"agents":[pane],
        "layouts":[{"workspace_id":"w","tab_id":"w:t1","panes":[{"pane_id":"w:p1"}]}]
    }))
    .unwrap();
    let caller = PiCallerObservation::from_snapshot(&client, snapshot.clone(), "native").unwrap();
    let workspace = caller.calling_workspace();
    app.state(&opened)
        .unwrap()
        .stream_lock(Some(workspace.scope()), "effort")
        .unwrap()
        .write_association(&Association::new("effort", "w", None, true).unwrap())
        .unwrap();
    fs::write(&transcript, "{\"type\":\"session\",\"id\":\"changed\"}\n").unwrap();
    for inventory in [false, true] {
        let observed = app
            .agent_observation_from_native(
                Harness::Pi,
                "native",
                None,
                true,
                inventory,
                Some(NativeObservation::Pi(&caller)),
            )
            .unwrap();
        assert_eq!(observed.context.session.unwrap().id, session.id);
        assert_eq!(observed.native.unwrap().native_session_id(), "native");
        assert_eq!(observed.native_inventory.is_some(), inventory);
        if let Some(inventory) = observed.native_inventory {
            assert_eq!(inventory.panes()[0].native_session_id(), Some("native"));
        }
    }
    assert!(
        app.agent_observation_from_native(
            Harness::Pi,
            "native",
            None,
            true,
            false,
            Some(NativeObservation::Workspace(workspace)),
        )
        .is_err()
    );
    let mut source = ApplicationSource::new(app, "native".into(), None, None);
    assert!(source.read(&client, &snapshot, false).is_err());
    fs::write(&transcript, original).unwrap();
    assert_eq!(
        source
            .read(&client, &snapshot, false)
            .unwrap()
            .card
            .session()
            .id,
        session.id
    );
    fs::write(&transcript, "{\"type\":\"session\",\"id\":\"changed\"}\n").unwrap();
    assert!(source.read(&client, &snapshot, false).is_err());
    assert_eq!(server.methods(), ["ping"]);
}
