use serde_json::{Value, json};
use streams_temper::frontend::herdr::HerdrTopology;
use streams_temper::runtime::{conversations::Harness, native_inventory::verified_inventory};

fn snapshot() -> Value {
    json!({
        "workspaces":[{"workspace_id":"w", "label":"work"}, {"workspace_id":"other", "label":"other"}],
        "tabs":[{"workspace_id":"w", "tab_id":"t", "label":"work"}, {"workspace_id":"other", "tab_id":"foreign", "label":"other"}],
        "panes":[
            {"workspace_id":"w", "tab_id":"t", "pane_id":"p1", "terminal_id":"term1"},
            {"workspace_id":"w", "tab_id":"t", "pane_id":"p2", "terminal_id":"term2"},
            {"workspace_id":"other", "tab_id":"foreign", "pane_id":"p3", "terminal_id":"term3"}
        ],
        "layouts":[
            {"workspace_id":"w", "tab_id":"t", "panes":[{"pane_id":"p1"},{"pane_id":"p2"}]},
            {"workspace_id":"other", "tab_id":"foreign", "panes":[{"pane_id":"p3"}]}
        ],
        "agents":[
            agent("w", "t", "p1", "term1", "n1"),
            agent("w", "t", "p2", "term2", "n2"),
            agent("other", "foreign", "p3", "term3", "n3")
        ]
    })
}
fn agent(workspace: &str, tab: &str, pane: &str, terminal: &str, id: &str) -> Value {
    json!({"workspace_id":workspace,"tab_id":tab,"pane_id":pane,"terminal_id":terminal,
        "agent":"pi","agent_session":{"agent":"pi","kind":"id","value":id}})
}

#[test]
fn current_inventory_borrows_actual_snapshot_rows_and_has_no_saved_state() {
    let topology = HerdrTopology::parse(&snapshot()).unwrap();
    let verified = verified_inventory(&topology, None).unwrap();
    assert_eq!(verified.len(), 3);
    assert!(std::ptr::eq(verified[0].native, &topology.agents[0]));
    assert_eq!(verified[0].harness, Harness::Pi);
}

#[test]
fn unresolved_outside_scope_is_not_opened_but_prevents_a_complete_inventory() {
    let mut value = snapshot();
    value["agents"][2]["agent_session"]["kind"] = json!("path");
    value["agents"][2]["agent_session"]["value"] = json!("/not-a-discovery-input.jsonl");
    let topology = HerdrTopology::parse(&value).unwrap();
    assert_eq!(verified_inventory(&topology, Some("w")).unwrap().len(), 2);
    assert!(verified_inventory(&topology, None).is_err());
    assert!(verified_inventory(&topology, Some("other")).is_err());
}

#[test]
fn malformed_scope_identity_reports_the_pane_and_never_returns_partial_inventory() {
    for metadata in [
        None,
        Some(Value::Null),
        Some(json!({"agent":"codex","kind":"id","value":"n2"})),
    ] {
        let mut value = snapshot();
        let sibling = value["agents"][1].as_object_mut().unwrap();
        if let Some(metadata) = metadata {
            sibling.insert("agent_session".into(), metadata);
        } else {
            sibling.remove("agent_session");
        }
        let topology = HerdrTopology::parse(&value).unwrap();
        let message = verified_inventory(&topology, Some("w"))
            .unwrap_err()
            .to_string();
        assert!(message.contains("pane p2"), "{message}");
        assert!(message.contains("workspace w"), "{message}");
        assert!(message.contains("tab t"), "{message}");
    }
}

#[test]
fn duplicate_live_identities_refuse_and_unsupported_harnesses_are_ignored() {
    let mut value = snapshot();
    value["agents"][1]["agent_session"]["value"] = json!("n1");
    assert!(verified_inventory(&HerdrTopology::parse(&value).unwrap(), None).is_err());
    value["agents"][1]["agent"] = json!("future-agent");
    value["agents"][1]["agent_session"] = Value::Null;
    assert_eq!(
        verified_inventory(&HerdrTopology::parse(&value).unwrap(), None)
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn path_metadata_verifies_native_identity_and_returns_only_the_exact_locator() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("not-a-native-id.jsonl");
    std::fs::write(&path, "{\"type\":\"session\",\"id\":\"n1\"}\n").unwrap();
    let mut value = snapshot();
    value["agents"][0]["agent_session"] = json!({"agent":"pi","kind":"path","value":path});
    let topology = HerdrTopology::parse(&value).unwrap();
    let verified = verified_inventory(&topology, Some("w")).unwrap();
    assert_eq!(verified[0].identity.id, "n1");
    assert_eq!(
        verified[0].identity.locator,
        Some(path.canonicalize().unwrap())
    );
}
