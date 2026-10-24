use super::*;
use crate::file_io;
use serde_json::json;
use std::os::unix::fs::DirBuilderExt;

fn snapshot() -> Value {
    let markers = |role: &str| {
        json!({"strm_agent":"old-pair", "strm_nonce":"card-nonce",
        "strm_pi":"owner-terminal", "strm_card":"card-terminal", "strm_role":role, "strm_native":"native"})
    };
    json!({
        "workspaces":[{"workspace_id":"w","label":"Work","number":1,"tokens":{}}],
        "tabs":[{"workspace_id":"w","tab_id":"t","label":"1","number":1}],
        "panes":[
            {"pane_id":"owner","terminal_id":"owner-terminal","workspace_id":"w","tab_id":"t","tokens":markers("agent")},
            {"pane_id":"card","terminal_id":"card-terminal","workspace_id":"w","tab_id":"t","tokens":markers("card")},
            {"pane_id":"editor","terminal_id":"editor-terminal","workspace_id":"w","tab_id":"t","tokens":{"strm_editor":"editor-nonce","strm_doc":"doc"}},
            {"pane_id":"unmarked","terminal_id":"unmarked-terminal","workspace_id":"w","tab_id":"t","tokens":{}}
        ],
        "layouts":[{"workspace_id":"w","tab_id":"t","panes":[{"pane_id":"owner"},{"pane_id":"card"},{"pane_id":"editor"},{"pane_id":"unmarked"}]}],
        "agents":[]
    })
}

struct Fixture {
    _temp: tempfile::TempDir,
    state: NativeState,
    legacy: NativeState,
    server: fake_herdr::FakeHerdr,
    client: HerdrClient,
    keys: Vec<String>,
}
impl Fixture {
    fn new(failure: &'static str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let state = NativeState::new(temp.path().join("state")).unwrap();
        let socket = temp.path().join("herdr.sock");
        let legacy = NativeState::new(temp.path().join("v1")).unwrap();
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(legacy.directory())
            .unwrap();
        let root = legacy.directory().to_owned();
        let completed = state.directory().join("format.json");
        let mut keys = Vec::new();
        for (prefix, nonce, pane, terminal) in [
            ("ab", "card-nonce", "card", "card-terminal"),
            ("cd", "editor-nonce", "editor", "editor-terminal"),
            ("ef", "unmarked-nonce", "unmarked", "unmarked-terminal"),
            ("12", "card-nonce", "card", "card-terminal"),
        ] {
            let key = prefix.repeat(32);
            file_io::write_json(&root.join(format!("{key}.json")), &json!({"version":1,
                "key":key,"nonce":nonce,"phase":"pending","paneId":pane,"terminalId":terminal,"tabId":"t"})).unwrap();
            keys.push(key);
        }
        let key = "34".repeat(32);
        file_io::write_json(
            &root.join(format!("{key}.json")),
            &json!({"version":1,"key":key,"nonce":"unknown","phase":"pending"}),
        )
        .unwrap();
        keys.push(key);
        let key = "56".repeat(32);
        file_io::write_json(
            &root.join(format!("{key}.json")),
            &json!({"broken":"generated evidence"}),
        )
        .unwrap();
        keys.push(key);
        file_io::write_json(
            &root.join("workspace-open.json"),
            &json!({"pending":"keep"}),
        )
        .unwrap();
        let server = fake_herdr::FakeHerdr::start(&socket, move |method, params| {
            if method != "session.snapshot" {
                assert!(!completed.exists(), "completion preceded native reply");
                assert!(
                    root.join(format!("{}.json", "ab".repeat(32))).exists(),
                    "intent deleted before native reply"
                );
            }
            fake_herdr::Reply::Result(match method {
                "session.snapshot" => json!({"type":"session_snapshot","snapshot":snapshot()}),
                "plugin.pane.close" => {
                    assert_eq!(
                        params["pane_id"], "card",
                        "never close editors or unknown panes"
                    );
                    json!({"type":"plugin_pane_closed","pane_id":if failure == "close" {"wrong"} else {"card"}})
                }
                "pane.report_metadata" => {
                    assert_eq!(
                        params["pane_id"], "owner",
                        "closed card needs no metadata clear"
                    );
                    assert_eq!(params["source"], PLUGIN_ID);
                    for key in PAIR_KEYS {
                        assert!(params["tokens"][key].is_null());
                    }
                    json!({"type":if failure == "clear" {"wrong"} else {"ok"}})
                }
                _ => panic!("unplanned reset request: {method} {params}"),
            })
        });
        let client = HerdrClient::connect(
            BTreeMap::from([("HERDR_SOCKET_PATH".into(), socket.to_string_lossy().into())]),
            1.0,
        )
        .unwrap();
        Self {
            _temp: temp,
            state,
            legacy,
            server,
            client,
            keys,
        }
    }
    fn reset(&self) -> Result<EvidenceResetReport> {
        reset_evidence(
            &self.client,
            &self.state,
            &self.legacy,
            0.0,
            &AtomicBool::new(false),
        )
    }
}

#[test]
fn reset_is_one_snapshot_one_pass_and_never_closes_editors_or_unknown_panes() {
    let f = Fixture::new("");
    let workspace_open = f.legacy.directory().join("workspace-open.json");
    let pending = std::fs::read(&workspace_open).unwrap();
    let report = f.reset().unwrap();
    assert!(!report.already_completed);
    assert_eq!(report.closed_cards, ["card"]);
    assert_eq!(report.cleared_pair_markers, ["owner"]);
    assert_eq!(report.deleted_evidence, 6);
    assert_eq!(
        f.server.methods(),
        [
            "ping",
            "session.snapshot",
            "plugin.pane.close",
            "pane.report_metadata"
        ]
    );
    for key in &f.keys {
        assert!(!f.legacy.directory().join(format!("{key}.json")).exists());
    }
    assert!(f.state.format_completed().unwrap());
    assert!(!f.legacy.format_completed().unwrap());
    assert_eq!(std::fs::read(workspace_open).unwrap(), pending);
    assert!(f.state.read(&"ab".repeat(32)).unwrap().is_none());
    let before = f.server.methods();
    assert!(f.reset().unwrap().already_completed);
    assert_eq!(
        f.server.methods(),
        before,
        "completed reset never restarts cards"
    );
}

#[test]
fn cancelled_reset_does_not_observe_delete_or_install() {
    let f = Fixture::new("");
    assert!(reset_evidence(&f.client, &f.state, &f.legacy, 0.0, &AtomicBool::new(true)).is_err());
    assert_eq!(f.server.methods(), ["ping"]);
    assert!(!f.state.directory().exists());
    for key in &f.keys {
        assert!(f.legacy.directory().join(format!("{key}.json")).exists());
    }
}

#[test]
fn mismatched_close_or_clear_reply_retains_all_evidence_and_no_completion() {
    for failure in ["close", "clear"] {
        let f = Fixture::new(failure);
        assert!(f.reset().is_err(), "{failure}");
        for key in &f.keys {
            assert!(f.legacy.directory().join(format!("{key}.json")).exists());
        }
        assert!(!f.state.directory().join("format.json").exists());
        assert_eq!(
            f.server
                .methods()
                .iter()
                .filter(|m| *m == "session.snapshot")
                .count(),
            1
        );
    }
}

#[test]
fn pure_plan_requires_exact_nonce_resources_and_card_markers() {
    let mut s = snapshot();
    let record = LegacyEvidence {
        key: "ab".repeat(32),
        observed: Some((
            "card-nonce".into(),
            Resources::new("card", "card-terminal", "t").unwrap(),
        )),
    };
    for (field, value) in [
        ("strm_nonce", "foreign"),
        ("strm_card", "other-terminal"),
        ("strm_role", "editor"),
    ] {
        s["panes"][1]["tokens"][field] = value.into();
        let parsed = HerdrNativeSnapshot::parse(&s).unwrap();
        assert!(
            plan(&parsed, std::slice::from_ref(&record))
                .iter()
                .all(|action| !matches!(action, ResetAction::CloseCard(_)))
        );
        s = snapshot();
    }
    s["panes"][1]["terminal_id"] = "recycled".into();
    let parsed = HerdrNativeSnapshot::parse(&s).unwrap();
    assert!(
        plan(&parsed, &[record])
            .iter()
            .all(|action| !matches!(action, ResetAction::CloseCard(_)))
    );
}
