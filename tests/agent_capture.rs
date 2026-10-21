use std::{collections::BTreeMap, fs, path::PathBuf};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use streams_temper::{
    agent::{CaptureTarget, capture_inputs},
    application::InspectedCatalog,
    config::HostIdentity,
    frontend::herdr::HerdrTopology,
    model::{Catalog, Session, Stream},
    operations::Authority,
    runtime::{conversations::ConversationRoots, state::RuntimeState},
    storage::Snapshot,
    sync::Inspection,
};

const HOST: &str = "01234567-89ab-4def-8123-456789abcdef";
const FOREIGN: &str = "11234567-89ab-4def-8123-456789abcdef";
const SCOPE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const OTHER_SCOPE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct Fixture {
    temp: tempfile::TempDir,
    inspected: InspectedCatalog,
    state: RuntimeState,
    roots: ConversationRoots,
}

impl Fixture {
    fn new(sessions: Vec<Session>) -> Self {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let state = RuntimeState::new(temp.path().join("runtime"), HOST).unwrap();
        let roots = ConversationRoots {
            pi: vec![temp.path().join("pi")],
            codex: vec![temp.path().join("codex")],
            claude: vec![temp.path().join("claude")],
        };
        let catalog = Catalog::new(
            vec![
                Stream::new("effort", "Effort title", None, HOST).unwrap(),
                Stream::new("other", "Other title", None, HOST).unwrap(),
            ],
            sessions,
            vec![],
        )
        .unwrap();
        let inspected = InspectedCatalog {
            root: temp.path().join("catalog-never-opened"),
            host: HostIdentity::parse(HOST).unwrap(),
            source: "frozen-physical-source".into(),
            inspection: Inspection {
                snapshot: Snapshot {
                    catalog,
                    revisions: BTreeMap::new(),
                },
                authorities: BTreeMap::from([(
                    "effort".into(),
                    Authority::new(HOST, "live-secret-must-not-leak").unwrap(),
                )]),
            },
        };
        Self {
            temp,
            inspected,
            state,
            roots,
        }
    }

    fn capture(&self, topology: &HerdrTopology, target: CaptureTarget<'_>) -> Value {
        capture_inputs(
            &self.inspected,
            &self.state,
            Ok((topology, SCOPE)),
            target,
            &self.roots,
        )
        .unwrap()
    }

    fn transcript(&self, filename: &str, identity: &str) -> PathBuf {
        fs::create_dir_all(&self.roots.pi[0]).unwrap();
        let path = self.roots.pi[0].join(filename);
        fs::write(
            &path,
            format!("{}\n", json!({"type": "session", "id": identity})),
        )
        .unwrap();
        path.canonicalize().unwrap()
    }

    fn association(&self, scope: &str, workspace: &str, stream: &str, ready: bool) -> PathBuf {
        let key = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&[workspace]).unwrap())
        );
        let path = self
            .state
            .root()
            .join("associations")
            .join(scope)
            .join(format!("{key}.json"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            json!({
                "version": 1, "workspace_id": workspace, "stream_id": stream,
                "session_id": null, "command": null, "ready": ready,
            })
            .to_string(),
        )
        .unwrap();
        path
    }
}

fn session(host: &str, id: &str, harness: &str, native: &str) -> Session {
    Session::new(
        id,
        host,
        "effort",
        harness,
        native,
        format!("Contribution {id}"),
    )
    .unwrap()
}

fn native(harness: &str, identity: &str) -> Value {
    json!({"agent": harness, "agent_session": {"agent": harness, "kind": "id", "value": identity}})
}

fn topology(workspaces: &[&str], agents: Vec<(&str, Value)>) -> HerdrTopology {
    let agents: Vec<_> = agents
        .into_iter()
        .enumerate()
        .map(|(index, (workspace, mut native))| {
            native["workspace_id"] = json!(workspace);
            native["tab_id"] = json!(format!("tab-{workspace}"));
            native["pane_id"] = json!(format!("pane-{index}"));
            native["terminal_id"] = json!(format!("terminal-{index}"));
            native
        })
        .collect();
    let snapshot = json!({
        "workspaces": workspaces.iter().map(|id| json!({"workspace_id": id, "label": format!("Label {id}")})).collect::<Vec<_>>(),
        "tabs": workspaces.iter().map(|id| json!({"workspace_id": id, "tab_id": format!("tab-{id}"), "label": "Tab"})).collect::<Vec<_>>(),
        "panes": agents.iter().map(|agent| json!({
            "workspace_id": agent["workspace_id"], "tab_id": agent["tab_id"],
            "pane_id": agent["pane_id"], "terminal_id": agent["terminal_id"],
        })).collect::<Vec<_>>(),
        "layouts": workspaces.iter().map(|id| json!({
            "workspace_id": id, "tab_id": format!("tab-{id}"),
            "panes": agents.iter().filter(|agent| agent["workspace_id"].as_str() == Some(*id))
                .map(|agent| json!({"pane_id": agent["pane_id"]})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "agents": agents,
    });
    HerdrTopology::parse(&snapshot).unwrap()
}

fn sample<'a>(capture: &'a Value, host: &str, native: &str) -> &'a Value {
    capture["samples"]
        .as_array()
        .unwrap()
        .iter()
        .find(|sample| sample["host_id"] == host && sample["native_session_id"] == native)
        .unwrap()
}

#[test]
fn failed_native_inventory_creates_no_runtime_or_catalog_state() {
    let f = Fixture::new(vec![session(HOST, "historical", "pi", "historical")]);
    assert!(!f.state.root().exists());
    assert!(!f.inspected.root.exists());
    let result = capture_inputs(
        &f.inspected,
        &f.state,
        Err("native topology unavailable".into()),
        CaptureTarget::Inventory,
        &f.roots,
    )
    .unwrap();
    assert_eq!(result["version"], 1);
    assert_eq!(result["host_id"], HOST);
    assert_eq!(result["streams"].as_array().unwrap().len(), 2);
    assert_eq!(result["workspaces"], json!([]));
    assert_eq!(result["topology_error"], "native topology unavailable");
    assert!(result.get("samples").is_none());
    assert!(!f.state.root().exists());
    assert!(!f.inspected.root.exists());
    let selected = capture_inputs(
        &f.inspected,
        &f.state,
        Err("native topology unavailable".into()),
        CaptureTarget::Stream("effort"),
        &f.roots,
    )
    .unwrap();
    assert_eq!(sample(&selected, HOST, "historical")["open"], Value::Null);
    assert!(!f.state.root().exists());
    assert!(!f.inspected.root.exists());
}

#[test]
fn empty_positive_topology_proves_closed_but_redacts_all_live_authority() {
    let f = Fixture::new(vec![
        session(HOST, "z-local", "pi", "z-native"),
        session(HOST, "a-local", "pi", "a-native"),
        session(FOREIGN, "foreign", "pi", "foreign-native"),
        session(HOST, "codex", "codex", "codex-native"),
    ]);
    let path = f.transcript("a-native.jsonl", "a-native");
    let result = f.capture(&topology(&[], vec![]), CaptureTarget::Stream("effort"));
    assert_eq!(
        result["target"],
        json!({"kind": "stream", "id": "effort", "title": "Effort title"})
    );
    assert_eq!(result["topology_error"], Value::Null);
    let local = sample(&result, HOST, "a-native");
    assert_eq!(local["open"], false);
    assert_eq!(local["transcript_file"], json!(path));
    assert_eq!(local["unavailable"], Value::Null);
    assert_eq!(local["catalog"]["catalog_source"], "frozen-physical-source");
    assert_eq!(
        local["catalog"]["authority"],
        json!({"host_id": HOST, "token": "[redacted]"})
    );
    assert_eq!(local["catalog"]["sessions"].as_array().unwrap().len(), 4);
    assert_eq!(sample(&result, HOST, "z-native")["open"], false);
    assert!(sample(&result, HOST, "z-native")["unavailable"].is_string());
    assert_eq!(
        sample(&result, FOREIGN, "foreign-native")["open"],
        Value::Null
    );
    assert_eq!(
        result["unsupported"],
        json!([{"host_id": HOST, "harness": "codex", "native_session_id": "codex-native"}])
    );
    assert!(!result.to_string().contains("live-secret-must-not-leak"));
    let identities: Vec<_> = result["samples"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            (
                s["host_id"].as_str().unwrap(),
                s["native_session_id"].as_str().unwrap(),
            )
        })
        .collect();
    let mut sorted = identities.clone();
    sorted.sort();
    assert_eq!(identities, sorted);
    assert!(!f.state.root().exists());
    assert!(!f.inspected.root.exists());
}

#[test]
fn workspace_capture_includes_uncarded_pi_without_attaching_or_resolving_cwd() {
    let f = Fixture::new(vec![]);
    let topology = topology(
        &["workspace"],
        vec![("workspace", native("pi", "uncarded"))],
    );
    let result = f.capture(&topology, CaptureTarget::Workspace("workspace"));
    assert_eq!(
        result["target"],
        json!({"kind": "workspace", "id": "workspace", "title": "Label workspace"})
    );
    assert_eq!(result["workspaces"][0]["stream_id"], Value::Null);
    let captured = sample(&result, HOST, "uncarded");
    assert_eq!(captured["open"], true);
    assert_eq!(captured["catalog"]["stream"], Value::Null);
    assert_eq!(captured["catalog"]["session"], Value::Null);
    assert_eq!(captured["catalog"]["sessions"], json!([]));
    assert_eq!(captured["catalog"]["authority"], Value::Null);
    assert_eq!(captured["transcript_file"], Value::Null);
    assert!(captured["unavailable"].is_string());
    assert_eq!(
        captured["locations"],
        json!([{
            "workspace_id": "workspace", "tab_id": "tab-workspace",
            "pane_id": "pane-0", "terminal_id": "terminal-0",
        }])
    );
    assert!(!f.state.root().exists());
    assert_eq!(f.inspected.inspection.snapshot.catalog.sessions().len(), 0);
}

#[test]
fn scoped_ready_associations_supply_fallback_not_membership() {
    let f = Fixture::new(vec![]);
    f.association(SCOPE, "workspace", "effort", true);
    f.association(SCOPE, "pending", "missing-is-ignored-until-ready", false);
    f.association(
        OTHER_SCOPE,
        "workspace",
        "missing-in-unselected-scope",
        true,
    );
    let topology = topology(
        &["workspace", "pending"],
        vec![
            ("workspace", native("pi", "new-native")),
            ("pending", native("pi", "pending-native")),
        ],
    );
    let result = f.capture(&topology, CaptureTarget::Workspace("workspace"));
    assert_eq!(
        sample(&result, HOST, "new-native")["catalog"]["stream"]["id"],
        "effort"
    );
    assert_eq!(
        sample(&result, HOST, "new-native")["catalog"]["session"],
        Value::Null
    );
    assert_eq!(result["workspaces"][1]["stream_id"], Value::Null);
    let pending = f.capture(&topology, CaptureTarget::Workspace("pending"));
    assert_eq!(
        sample(&pending, HOST, "pending-native")["catalog"]["stream"],
        Value::Null
    );
}

#[test]
fn foreign_host_never_uses_local_live_locations_or_transcripts_for_the_same_native_id() {
    let f = Fixture::new(vec![session(FOREIGN, "foreign", "pi", "shared-native")]);
    f.association(SCOPE, "workspace", "effort", true);
    let path = f.transcript("shared-native.jsonl", "shared-native");
    let topology = topology(
        &["workspace"],
        vec![("workspace", native("pi", "shared-native"))],
    );
    let result = f.capture(&topology, CaptureTarget::Stream("effort"));
    let foreign = sample(&result, FOREIGN, "shared-native");
    assert_eq!(foreign["open"], Value::Null);
    assert_eq!(foreign["locations"], json!([]));
    assert_eq!(foreign["transcript_file"], Value::Null);
    assert_eq!(foreign["unavailable"], "foreign host");
    let local = sample(&result, HOST, "shared-native");
    assert_eq!(local["open"], true);
    assert_eq!(local["transcript_file"], json!(path));
    assert_eq!(local["catalog"]["session"], Value::Null);
    assert_eq!(local["catalog"]["sessions"].as_array().unwrap().len(), 1);
}

#[test]
fn observed_conversation_uses_its_actual_catalog_stream_not_the_workspace_fallback() {
    let mut enrolled = session(HOST, "elsewhere", "pi", "known-native");
    enrolled.stream_id = "other".into();
    let mut sibling = session(FOREIGN, "sibling", "pi", "sibling-native");
    sibling.stream_id = "other".into();
    let f = Fixture::new(vec![enrolled, sibling]);
    f.association(SCOPE, "workspace", "effort", true);
    let topology = topology(
        &["workspace"],
        vec![("workspace", native("pi", "known-native"))],
    );
    let result = f.capture(&topology, CaptureTarget::Stream("effort"));
    let captured = sample(&result, HOST, "known-native");
    assert_eq!(captured["catalog"]["stream"]["id"], "other");
    assert_eq!(captured["catalog"]["session"]["id"], "elsewhere");
    assert_eq!(captured["catalog"]["sessions"].as_array().unwrap().len(), 2);
    assert_eq!(captured["catalog"]["authority"], Value::Null);
    assert_eq!(
        f.inspected
            .inspection
            .snapshot
            .catalog
            .session(HOST, "elsewhere")
            .unwrap()
            .stream_id,
        "other"
    );
}

#[test]
fn redaction_preserves_foreign_effective_authority_presence_and_host() {
    let mut f = Fixture::new(vec![session(HOST, "local", "pi", "native")]);
    f.inspected.inspection.authorities.insert(
        "effort".into(),
        Authority::new(FOREIGN, "foreign-secret").unwrap(),
    );
    let result = f.capture(&topology(&[], vec![]), CaptureTarget::Stream("effort"));
    assert_eq!(
        sample(&result, HOST, "native")["catalog"]["authority"],
        json!({"host_id": FOREIGN, "token": "[redacted]"})
    );
    assert!(!result.to_string().contains("foreign-secret"));
}

#[test]
fn each_workspace_identity_check_is_all_or_nothing() {
    let f = Fixture::new(vec![session(HOST, "good", "pi", "good-native")]);
    f.association(SCOPE, "workspace", "effort", true);
    for malformed in [
        Value::Null,
        json!(["codex", "id", "bad-native"]),
        json!({"agent": "codex", "id": "old-structural-only-shape"}),
        json!({"agent": "pi", "kind": "id", "value": "mismatch"}),
        json!({"agent": {"codex": null}, "kind": "id", "value": "bad"}),
        json!({"agent": "codex", "kind": {"id": null}, "value": "bad"}),
        json!({"agent": "codex", "kind": "id", "value": " "}),
        json!({"agent": "codex", "kind": "path", "value": f.temp.path().join("missing-capture-fixture")}),
    ] {
        let mut bad = native("codex", "bad-native");
        bad["agent_session"] = malformed;
        let topology = topology(
            &["workspace"],
            vec![
                ("workspace", native("pi", "good-native")),
                ("workspace", bad),
            ],
        );
        let result = f.capture(&topology, CaptureTarget::Stream("effort"));
        let captured = sample(&result, HOST, "good-native");
        assert_eq!(captured["open"], Value::Null, "{result}");
        assert_eq!(captured["locations"], json!([]));
        assert_eq!(result["unsupported"], json!([]));
        assert!(
            result["topology_error"]
                .as_str()
                .unwrap()
                .contains("workspace")
        );
    }
}

#[test]
fn duplicate_live_native_identity_rejects_whole_workspace_not_just_the_duplicate() {
    let f = Fixture::new(vec![session(HOST, "old", "pi", "same-native")]);
    f.association(SCOPE, "workspace", "effort", true);
    let topology = topology(
        &["workspace"],
        vec![
            ("workspace", native("pi", "same-native")),
            ("workspace", native("pi", "same-native")),
            ("workspace", native("pi", "another-native")),
        ],
    );
    let result = f.capture(&topology, CaptureTarget::Stream("effort"));
    assert_eq!(result["samples"].as_array().unwrap().len(), 1);
    assert_eq!(sample(&result, HOST, "same-native")["open"], Value::Null);
    assert!(
        result["topology_error"]
            .as_str()
            .unwrap()
            .contains("ambiguous live native conversation identity")
    );
}

#[test]
fn unrelated_pi_is_not_resolved_and_makes_historical_liveness_unknown() {
    let f = Fixture::new(vec![session(HOST, "old", "pi", "historical")]);
    f.association(SCOPE, "selected", "effort", true);
    let mut unrelated = native("pi", "unverified");
    unrelated["agent_session"] = json!({"agent": "pi", "kind": "path", "value": f.temp.path().join("not-read-for-liveness")});
    let topology = topology(&["selected", "unrelated"], vec![("unrelated", unrelated)]);
    let result = f.capture(&topology, CaptureTarget::Stream("effort"));
    assert_eq!(sample(&result, HOST, "historical")["open"], Value::Null);
    assert_eq!(
        result["topology_error"],
        "Pi identities outside selected workspaces were not verified; historical open state unknown"
    );
    assert_eq!(result["samples"].as_array().unwrap().len(), 1);
}

#[test]
fn unrelated_non_pi_does_not_prevent_positive_closed_evidence() {
    let f = Fixture::new(vec![session(HOST, "old", "pi", "historical")]);
    f.association(SCOPE, "selected", "effort", true);
    let mut unrelated = native("codex", "unverified");
    unrelated["agent_session"] = Value::Null;
    let topology = topology(&["selected", "unrelated"], vec![("unrelated", unrelated)]);
    let result = f.capture(&topology, CaptureTarget::Stream("effort"));
    assert_eq!(sample(&result, HOST, "historical")["open"], false);
    assert_eq!(result["topology_error"], Value::Null);
}

#[test]
fn dangling_or_corrupt_ready_associations_invalidate_all_native_inventory() {
    let f = Fixture::new(vec![session(HOST, "old", "pi", "historical")]);
    f.association(SCOPE, "workspace", "effort", true);
    let dangling = f.association(SCOPE, "dangling", "not-local", true);
    let topology = topology(
        &["workspace", "dangling"],
        vec![("workspace", native("pi", "historical"))],
    );
    let result = f.capture(&topology, CaptureTarget::Inventory);
    assert_eq!(result["workspaces"], json!([]));
    assert!(
        result["topology_error"]
            .as_str()
            .unwrap()
            .contains("dangling ready workspace association")
    );
    let selected = f.capture(&topology, CaptureTarget::Stream("effort"));
    assert_eq!(sample(&selected, HOST, "historical")["open"], Value::Null);
    assert!(
        capture_inputs(
            &f.inspected,
            &f.state,
            Ok((&topology, SCOPE)),
            CaptureTarget::Workspace("workspace"),
            &f.roots
        )
        .is_err()
    );
    fs::write(&dangling, "[]").unwrap();
    let corrupt = f.capture(&topology, CaptureTarget::Inventory);
    assert_eq!(corrupt["workspaces"], json!([]));
    assert!(corrupt["topology_error"].is_string());
}

#[test]
fn targets_require_exact_local_stream_and_observed_workspace_identities() {
    let f = Fixture::new(vec![]);
    let topology = topology(&["observed"], vec![]);
    for target in [
        CaptureTarget::Stream("Effort title"),
        CaptureTarget::Stream("eff"),
        CaptureTarget::Stream("remote-only"),
        CaptureTarget::Workspace("unobserved"),
    ] {
        assert!(
            capture_inputs(
                &f.inspected,
                &f.state,
                Ok((&topology, SCOPE)),
                target,
                &f.roots
            )
            .is_err()
        );
    }
    assert!(
        capture_inputs(
            &f.inspected,
            &f.state,
            Err("offline".into()),
            CaptureTarget::Workspace("observed"),
            &f.roots
        )
        .is_err()
    );
    let exact = f.capture(&topology, CaptureTarget::Stream("effort"));
    assert_eq!(exact["target"]["id"], "effort");
    assert_eq!(exact["samples"], json!([]));
    assert!(!f.state.root().exists());
}

#[test]
fn supported_non_pi_conversations_are_reported_as_unsupported_replay_inputs() {
    let f = Fixture::new(vec![session(HOST, "codex", "codex", "saved-codex")]);
    f.association(SCOPE, "workspace", "effort", true);
    let topology = topology(
        &["workspace"],
        vec![
            ("workspace", native("codex", "live-codex")),
            ("workspace", native("claude", "live-claude")),
            ("workspace", native("pi", "live-pi")),
        ],
    );
    let result = f.capture(&topology, CaptureTarget::Stream("effort"));
    assert_eq!(
        result["unsupported"],
        json!([
            {"harness": "codex", "native_session_id": "live-codex", "workspace_id": "workspace"},
            {"harness": "claude", "native_session_id": "live-claude", "workspace_id": "workspace"},
            {"host_id": HOST, "harness": "codex", "native_session_id": "saved-codex"},
        ])
    );
    assert_eq!(result["samples"].as_array().unwrap().len(), 1);
    assert_eq!(
        sample(&result, HOST, "live-pi")["catalog"]["sessions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn verified_path_locators_are_used_but_conflicting_locators_remain_unavailable() {
    let f = Fixture::new(vec![]);
    f.association(SCOPE, "one", "effort", true);
    f.association(SCOPE, "two", "effort", true);
    let one = f.transcript("one.jsonl", "native");
    let two = f.transcript("two.jsonl", "native");
    let by_path = |path: &PathBuf| json!({"agent": "pi", "agent_session": {"agent": "pi", "kind": "path", "value": path}});
    let single = topology(&["one"], vec![("one", by_path(&one))]);
    let result = f.capture(&single, CaptureTarget::Stream("effort"));
    assert_eq!(
        sample(&result, HOST, "native")["transcript_file"],
        json!(one)
    );
    assert_eq!(sample(&result, HOST, "native")["open"], true);
    let conflicting = topology(
        &["one", "two"],
        vec![("one", by_path(&one)), ("two", by_path(&two))],
    );
    let result = f.capture(&conflicting, CaptureTarget::Stream("effort"));
    let captured = sample(&result, HOST, "native");
    assert_eq!(captured["open"], true);
    assert_eq!(captured["locations"].as_array().unwrap().len(), 2);
    assert_eq!(captured["transcript_file"], Value::Null);
    assert_eq!(
        captured["unavailable"],
        "ambiguous saved conversation locators"
    );
    assert_eq!(result["topology_error"], Value::Null);
    assert!(f.temp.path().is_dir());
}
