//! Pass-local selectors and exact owner identity, without native attestation or saved placement.
use serde_json::{Value, json};
use streams_temper::{
    frontend::herdr::HerdrNativeSnapshot,
    runtime::{
        native_cards::PiOwner,
        native_markers::{selected_stream, stream_tokens},
    },
};

fn snapshot(metadata: Value) -> Value {
    let owner = json!({"pane_id":"owner","terminal_id":"terminal","workspace_id":"w","tab_id":"t",
        "agent":"pi","agent_session":metadata});
    let neighbor = json!({"pane_id":"neighbor","terminal_id":"other-terminal","workspace_id":"w","tab_id":"t",
        "agent":"pi","agent_session":{"agent":"pi","kind":"unsupported","value":"must not inspect"}});
    json!({"workspaces":[{"workspace_id":"w","number":1,"label":"Work","tokens":{}}],
        "tabs":[{"workspace_id":"w","tab_id":"t","number":1,"label":"Work"}],
        "panes":[owner,neighbor],"agents":[owner,neighbor],
        "layouts":[{"workspace_id":"w","tab_id":"t","panes":[{"pane_id":"owner"},{"pane_id":"neighbor"}]}]})
}
#[test]
fn metadata_selection_uses_ordinary_markers_without_canonical_placement() {
    let mut value = snapshot(json!({"agent":"pi","kind":"id","value":"native"}));
    assert_eq!(
        selected_stream(
            &HerdrNativeSnapshot::parse(&value).unwrap(),
            "w",
            "catalog",
            "host"
        )
        .unwrap(),
        None
    );
    value["workspaces"][0]["tokens"] = json!(stream_tokens("catalog", "host", "effort"));
    assert_eq!(
        selected_stream(
            &HerdrNativeSnapshot::parse(&value).unwrap(),
            "w",
            "catalog",
            "host"
        )
        .unwrap()
        .as_deref(),
        Some("effort")
    );
    value["workspaces"][0]["tokens"]["strm_stream"] = json!("different");
    assert_eq!(
        selected_stream(
            &HerdrNativeSnapshot::parse(&value).unwrap(),
            "w",
            "catalog",
            "host"
        )
        .unwrap()
        .as_deref(),
        Some("different")
    );
    value["workspaces"][0]["tokens"]["strm_host"] = json!("foreign");
    assert!(
        selected_stream(
            &HerdrNativeSnapshot::parse(&value).unwrap(),
            "w",
            "catalog",
            "host"
        )
        .is_err()
    );
    value["workspaces"][0]["tokens"] = json!({"strm_host":"host"});
    assert!(
        selected_stream(
            &HerdrNativeSnapshot::parse(&value).unwrap(),
            "w",
            "catalog",
            "host"
        )
        .is_err()
    );
}
#[test]
fn terminal_identity_survives_a_move_but_not_conversation_replacement() {
    let mut value = snapshot(json!({"agent":"pi","kind":"id","value":"native"}));
    let owner = PiOwner::from_snapshot(
        &HerdrNativeSnapshot::parse(&value).unwrap(),
        "owner",
        "native",
    )
    .unwrap();
    for collection in ["panes", "agents"] {
        value[collection][0]["pane_id"] = json!("moved");
    }
    value["layouts"][0]["panes"][0]["pane_id"] = json!("moved");
    let observed = HerdrNativeSnapshot::parse(&value).unwrap();
    assert_eq!(
        owner.observe(&observed).unwrap().unwrap().0.pane.id,
        "moved"
    );
    for collection in ["panes", "agents"] {
        value[collection][0]["agent_session"]["value"] = json!("replacement");
    }
    assert!(
        owner
            .observe(&HerdrNativeSnapshot::parse(&value).unwrap())
            .unwrap()
            .is_none()
    );
}
#[test]
fn path_identity_checks_only_the_observed_exact_file_and_detects_replacement_at_that_path() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().canonicalize().unwrap().join("owner.jsonl");
    std::fs::write(&path, "{\"type\":\"session\",\"id\":\"native\"}\n").unwrap();
    let value = snapshot(json!({"agent":"pi","kind":"path","value":path}));
    let snapshot = HerdrNativeSnapshot::parse(&value).unwrap();
    let owner = PiOwner::from_snapshot(&snapshot, "owner", "native").unwrap();
    assert_eq!(
        owner.observe(&snapshot).unwrap().unwrap().1,
        Some(path.clone())
    );
    std::fs::write(&path, "{\"type\":\"session\",\"id\":\"replacement\"}\n").unwrap();
    assert!(owner.observe(&snapshot).unwrap().is_none());
    std::fs::remove_file(&path).unwrap();
    assert!(
        owner.observe(&snapshot).is_err(),
        "unavailable is unknown, not proof of exit"
    );
}
