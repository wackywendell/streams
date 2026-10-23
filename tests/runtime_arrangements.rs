use serde_json::{Value, json};
use streams_temper::frontend::herdr::HerdrTopology;
use streams_temper::model::{NamingMode, Session, Stream};
use streams_temper::runtime::arrangements::{
    ArrangedSession, Arrangement, ArrangementTab, capture, verified_inventory,
};
use streams_temper::runtime::conversations::Harness;

fn plan() -> Value {
    json!({"stream_id":"stream", "workspace_id":"w", "tabs":[{
        "tab_id":"t", "label":"agent title", "label_mode":"manual", "sessions":[{
            "session_id":"session", "harness":"future harness", "native_id":"native",
            "locator":"relative path accepted by Python", "pane_id":"p", "terminal_id":"term"
        }]
    }]})
}

fn snapshot() -> Value {
    // Tabs and panes deliberately differ from lexical order and layout order.
    json!({
        "workspaces":[{"workspace_id":"w", "label":"stream"}, {"workspace_id":"other", "label":"other"}],
        "tabs":[
            {"workspace_id":"w", "tab_id":"z", "label":"agent title", "number":1},
            {"workspace_id":"w", "tab_id":"a", "label":"2", "number":2},
            {"workspace_id":"w", "tab_id":"empty", "label":"", "number":3},
            {"workspace_id":"other", "tab_id":"foreign", "label":"foreign", "number":1}
        ],
        "panes":[
            {"workspace_id":"w", "tab_id":"z", "pane_id":"p1", "terminal_id":"term1"},
            {"workspace_id":"w", "tab_id":"z", "pane_id":"p2", "terminal_id":"term2"},
            {"workspace_id":"w", "tab_id":"a", "pane_id":"p3", "terminal_id":"term3"},
            {"workspace_id":"w", "tab_id":"empty", "pane_id":"p4", "terminal_id":"term4"},
            {"workspace_id":"other", "tab_id":"foreign", "pane_id":"p5", "terminal_id":"term5"}
        ],
        "layouts":[
            {"workspace_id":"w", "tab_id":"z", "panes":[{"pane_id":"p2"},{"pane_id":"p1"}]},
            {"workspace_id":"w", "tab_id":"a", "panes":[{"pane_id":"p3"}]},
            {"workspace_id":"w", "tab_id":"empty", "panes":[{"pane_id":"p4"}]},
            {"workspace_id":"other", "tab_id":"foreign", "panes":[{"pane_id":"p5"}]}
        ],
        "agents":[
            agent("w", "z", "p1", "term1", "n1"),
            agent("w", "z", "p2", "term2", "n2"),
            agent("w", "a", "p3", "term3", "n3"),
            agent("w", "empty", "p4", "term4", "n4"),
            agent("other", "foreign", "p5", "term5", "n5")
        ]
    })
}

fn agent(workspace: &str, tab: &str, pane: &str, terminal: &str, id: &str) -> Value {
    json!({"workspace_id":workspace, "tab_id":tab, "pane_id":pane, "terminal_id":terminal,
        "agent":"pi", "agent_session":{"agent":"pi", "kind":"id", "value":id},
        "cwd":"/same/checkout", "title":"agent title"})
}

fn stream() -> Stream {
    Stream::new("stream", "stream", None, "host").unwrap()
}

fn session(id: &str, native: &str, host: &str, stream: &str) -> Session {
    Session::new(id, host, stream, "pi", native, "agent title").unwrap()
}

fn sessions() -> Vec<Session> {
    (1..=4)
        .map(|i| session(&format!("s{i}"), &format!("n{i}"), "host", "stream"))
        .collect()
}

#[test]
fn plan_roundtrips_and_constructors_produce_validated_intent() {
    let value = plan();
    let saved: Arrangement = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(&saved).unwrap(), value);
    assert_eq!(saved.stream_id(), "stream");
    assert_eq!(saved.workspace_id(), "w");
    let entry = &saved.tabs()[0].sessions()[0];
    assert_eq!(entry.harness(), "future harness");
    assert_eq!(entry.native_id(), "native");
    assert_eq!(entry.locator(), Some("relative path accepted by Python"));
    assert_eq!(entry.pane_id(), Some("p"));
    assert_eq!(entry.terminal_id(), Some("term"));
    let entry =
        ArrangedSession::new("s".into(), "custom".into(), "n".into(), None, None, None).unwrap();
    let tab = ArrangementTab::new("t".into(), None, NamingMode::Automatic, vec![entry]).unwrap();
    let constructed = Arrangement::new("stream".into(), "w".into(), vec![tab]).unwrap();
    assert_eq!(
        serde_json::from_value::<Arrangement>(serde_json::to_value(&constructed).unwrap()).unwrap(),
        constructed
    );
}

#[test]
fn missing_optional_observations_and_nullable_labels_are_accepted() {
    let mut value = plan();
    let tab = &mut value["tabs"][0];
    tab.as_object_mut().unwrap().remove("label");
    for field in ["locator", "pane_id", "terminal_id"] {
        tab["sessions"][0].as_object_mut().unwrap().remove(field);
    }
    let saved: Arrangement = serde_json::from_value(value).unwrap();
    assert_eq!(saved.tabs()[0].label(), None);
    assert_eq!(saved.tabs()[0].sessions()[0].locator(), None);
    assert_eq!(saved.tabs()[0].sessions()[0].pane_id(), None);
    assert_eq!(saved.tabs()[0].sessions()[0].terminal_id(), None);
    for label in [json!(null), json!(""), json!("  ")] {
        let mut value = plan();
        value["tabs"][0]["label"] = label;
        assert!(serde_json::from_value::<Arrangement>(value).is_ok());
    }
}

#[test]
fn invalid_plan_identities_labels_locators_and_duplicate_session_ids_fail() {
    for field in ["stream_id", "workspace_id"] {
        for bad in [
            json!(null),
            json!(""),
            json!("  "),
            json!("bad\n"),
            json!(1),
        ] {
            let mut value = plan();
            value[field] = bad;
            assert!(serde_json::from_value::<Arrangement>(value).is_err());
        }
    }
    for field in ["session_id", "harness", "native_id", "locator"] {
        for bad in [json!(""), json!("  "), json!("bad\u{7f}"), json!(false)] {
            let mut value = plan();
            value["tabs"][0]["sessions"][0][field] = bad;
            assert!(
                serde_json::from_value::<Arrangement>(value).is_err(),
                "{field}"
            );
        }
    }
    for (field, bad) in [
        ("tab_id", json!("")),
        ("label", json!("bad\n")),
        ("label", json!(3)),
        ("label_mode", json!("generated")),
    ] {
        let mut value = plan();
        value["tabs"][0][field] = bad;
        assert!(serde_json::from_value::<Arrangement>(value).is_err());
    }
    let mut unknown = plan();
    unknown["typo"] = json!(true);
    assert!(serde_json::from_value::<Arrangement>(unknown).is_err());
    let mut value = plan();
    let duplicate = value["tabs"][0].clone();
    value["tabs"].as_array_mut().unwrap().push(duplicate);
    assert!(serde_json::from_value::<Arrangement>(value).is_err());
    assert!(serde_json::from_value::<Arrangement>(json!(["stream", "w", []])).is_err());
}

#[test]
fn capture_preserves_native_order_and_manual_label_provenance_without_id_discovery() {
    let topology = HerdrTopology::parse(&snapshot()).unwrap();
    let saved = capture(&topology, "w", &stream(), &sessions(), "host").unwrap();
    assert_eq!(
        saved.tabs().iter().map(|t| t.tab_id()).collect::<Vec<_>>(),
        ["z", "a", "empty"]
    );
    assert_eq!(saved.tabs()[0].label(), Some("agent title"));
    assert_eq!(saved.tabs()[0].label_mode(), NamingMode::Manual);
    assert_eq!(
        saved.tabs()[0]
            .sessions()
            .iter()
            .map(|s| s.session_id())
            .collect::<Vec<_>>(),
        ["s2", "s1"]
    );
    for tab in &saved.tabs()[1..] {
        assert_eq!(tab.label(), None);
        assert_eq!(tab.label_mode(), NamingMode::Automatic);
    }
    assert!(
        saved
            .tabs()
            .iter()
            .flat_map(|t| t.sessions())
            .all(|s| s.locator().is_none())
    );
    let verified = verified_inventory(&topology, None).unwrap();
    assert!(std::ptr::eq(verified[0].native, &topology.agents[0]));
    assert_eq!(verified[0].harness, Harness::Pi);
}

#[test]
fn capture_includes_only_known_local_host_stream_sessions() {
    let topology = HerdrTopology::parse(&snapshot()).unwrap();
    let sessions = vec![
        session("local", "n1", "host", "stream"),
        session("remote", "n2", "remote-host", "stream"),
        session("foreign", "n3", "host", "another-stream"),
    ];
    let saved = capture(&topology, "w", &stream(), &sessions, "host").unwrap();
    assert_eq!(saved.tabs().len(), 1);
    assert_eq!(saved.tabs()[0].sessions().len(), 1);
    assert_eq!(saved.tabs()[0].sessions()[0].session_id(), "local");
    assert!(capture(&topology, "absent", &stream(), &sessions, "host").is_err());
    assert!(
        capture(&topology, "w", &stream(), &[], "host")
            .unwrap()
            .tabs()
            .is_empty()
    );
}

#[test]
fn unresolved_foreign_workspace_is_not_read_for_capture_but_blocks_full_inventory() {
    let mut value = snapshot();
    let temp = tempfile::tempdir().unwrap();
    value["agents"][4]["agent_session"]["kind"] = json!("path");
    value["agents"][4]["agent_session"]["value"] = json!(temp.path().join("absent.jsonl"));
    let topology = HerdrTopology::parse(&value).unwrap();
    assert!(capture(&topology, "w", &stream(), &sessions(), "host").is_ok());
    assert_eq!(verified_inventory(&topology, Some("w")).unwrap().len(), 4);
    assert!(verified_inventory(&topology, None).is_err());
    assert!(verified_inventory(&topology, Some("other")).is_err());
}

#[test]
fn unresolved_sibling_identity_reports_its_pane_without_returning_partial_inventory() {
    for metadata in [
        None,
        Some(Value::Null),
        Some(json!({"agent":"codex", "kind":"id", "value":"n2"})),
    ] {
        let mut value = snapshot();
        let sibling = value["agents"][1].as_object_mut().unwrap();
        if let Some(metadata) = metadata {
            sibling.insert("agent_session".into(), metadata);
        } else {
            sibling.remove("agent_session");
        }
        let topology = HerdrTopology::parse(&value).unwrap();
        for result in [
            verified_inventory(&topology, Some("w")).map(|_| ()),
            capture(&topology, "w", &stream(), &sessions(), "host").map(|_| ()),
        ] {
            let message = result.unwrap_err().to_string();
            assert!(message.contains("pane p2"), "{message}");
            assert!(message.contains("workspace w"), "{message}");
            assert!(message.contains("tab z"), "{message}");
        }
    }
}

#[test]
fn duplicate_live_and_eligible_catalog_identities_fail_closed() {
    let mut value = snapshot();
    value["agents"][1]["agent_session"]["value"] = json!("n1");
    let topology = HerdrTopology::parse(&value).unwrap();
    assert!(verified_inventory(&topology, None).is_err());
    assert!(capture(&topology, "w", &stream(), &sessions(), "host").is_err());
    let topology = HerdrTopology::parse(&snapshot()).unwrap();
    let mut entries = sessions();
    entries.push(session("different-id", "n1", "host", "stream"));
    assert!(capture(&topology, "w", &stream(), &entries, "host").is_err());
}

#[test]
fn unsupported_named_harnesses_are_skipped_but_supported_bad_metadata_is_rejected() {
    let mut value = snapshot();
    value["agents"][0]["agent"] = json!("future-agent");
    value["agents"][0]["agent_session"] = json!(null);
    let topology = HerdrTopology::parse(&value).unwrap();
    assert_eq!(verified_inventory(&topology, None).unwrap().len(), 4);
    assert_eq!(
        capture(&topology, "w", &stream(), &sessions(), "host")
            .unwrap()
            .tabs()[0]
            .sessions()
            .len(),
        1
    );
    for metadata in [
        json!(null),
        json!({"agent":"codex", "kind":"id", "value":"n1"}),
        json!({"agent":"pi", "kind":"id", "value":"bad\n"}),
    ] {
        value["agents"][0]["agent"] = json!("pi");
        value["agents"][0]["agent_session"] = metadata;
        let topology = HerdrTopology::parse(&value).unwrap();
        assert!(verified_inventory(&topology, None).is_err());
        assert!(capture(&topology, "w", &stream(), &sessions(), "host").is_err());
    }
}

#[test]
fn path_metadata_is_verified_and_its_locator_is_captured() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("not-a-native-id.jsonl");
    std::fs::write(&path, "{\"type\":\"session\",\"id\":\"n1\"}\n").unwrap();
    let mut value = snapshot();
    value["agents"][0]["agent_session"] = json!({"agent":"pi", "kind":"path", "value":path});
    let topology = HerdrTopology::parse(&value).unwrap();
    let saved = capture(&topology, "w", &stream(), &sessions(), "host").unwrap();
    let entry = &saved.tabs()[0].sessions()[1];
    assert_eq!(entry.native_id(), "n1");
    let canonical = std::fs::canonicalize(path).unwrap();
    assert_eq!(entry.locator(), canonical.to_str());
}
