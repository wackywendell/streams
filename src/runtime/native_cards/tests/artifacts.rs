use super::*;
use crate::artifacts::{ArtifactEntry, ArtifactSummary};

struct ArtifactSource(Source);
impl NativeCardSource for ArtifactSource {
    fn read_records(&mut self, owner: &HerdrInventoryPane) -> Result<CardRecords> {
        let original = self.0.read_records(owner)?;
        let mut session = original.session().clone();
        session.artifacts.insert(
            "effort".into(),
            vec![ArtifactEntry {
                name: "assessment.md".into(),
                description: "Findings".into(),
            }],
        );
        CardRecords::new(
            original.stream().clone(),
            session,
            "catalog".into(),
            owner::location(owner),
        )
        .map_err(|e| invalid(e.to_string()))
    }
    fn read(&mut self, owner: &HerdrInventoryPane) -> Result<CardSnapshot> {
        CardSnapshot::from_records(self.read_records(owner)?, vec![], vec![])
            .map_err(|e| invalid(e.to_string()))
    }
    fn resolve_document(&mut self, target: &DocumentTarget) -> Result<ResolvedEditor> {
        self.0.resolve_document(target)
    }
    fn launch_context(&self) -> SourceLaunch {
        self.0.launch_context()
    }
}

fn engine(f: &Fixture, client: HerdrClient) -> NativeCardEngine {
    let snapshot = f.world.lock().unwrap().observe();
    NativeCardEngine::new(
        client,
        Box::new(ArtifactSource(Source {
            world: f.world.clone(),
            cwd: f.directory.path().canonicalize().unwrap(),
        })),
        snapshot,
        f.owner.clone(),
        f.config.clone(),
        f.environment.clone(),
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}
fn target(engine: &NativeCardEngine) -> ArtifactSummary {
    crate::artifacts::summaries(engine.card.session(), "effort").remove(0)
}

#[test]
fn artifact_open_is_read_only_fresh_and_never_retried_after_lost_reply() {
    for lost_reply in [false, true] {
        let f = Fixture::new();
        let world = f.world.clone();
        let plugin = f.directory.path().canonicalize().unwrap();
        let server = FakeHerdr::start(
            &f.directory.path().join("artifact.sock"),
            move |method, params| {
                let mut world = world.lock().unwrap();
                match method {
                    "plugin.pane.open" => {
                        assert_eq!(params["entrypoint"], "artifact");
                        assert_eq!(params["placement"], "tab");
                        assert_eq!(params["focus"], false);
                        assert_eq!(
                            params["env"]["STREAMS_ARTIFACT_REFERENCE"]
                                .as_str()
                                .unwrap()
                                .rsplit('/')
                                .next(),
                            Some("assessment.md")
                        );
                        assert_eq!(params["env"]["STREAMS_ARTIFACT_CATALOG_SOURCE"], "catalog");
                        assert!(params["env"].get("STREAMS_ARTIFACT_PATH").is_none());
                        world.created += 1;
                        let number = world.created;
                        let row = pane(
                            &format!("viewer-{number}"),
                            &format!("viewer-terminal-{number}"),
                            "w",
                            &format!("viewer-tab-{number}"),
                            "artifact viewer",
                        );
                        world.snapshot["panes"]
                            .as_array_mut()
                            .unwrap()
                            .push(row.clone());
                        world.snapshot["tabs"].as_array_mut().unwrap().push(json!({"tab_id":format!("viewer-tab-{number}"),"workspace_id":"w","number":number + 1,"label":"Output","resource":"viewer tab"}));
                        world.refresh();
                        if lost_reply {
                            Reply::Raw(vec![])
                        } else {
                            Reply::Result(
                                json!({"type":"plugin_pane_opened","plugin_pane":{"plugin_id":PLUGIN_ID,"entrypoint":"artifact","pane":row}}),
                            )
                        }
                    }
                    "tab.focus" => {
                        assert_eq!(params["tab_id"], format!("viewer-tab-{}", world.created));
                        Reply::Result(json!({"type":"ok"}))
                    }
                    _ => world.reply(method, params, &plugin),
                }
            },
        );
        let mut env = f.environment.clone();
        env.insert(
            "HERDR_SOCKET_PATH".into(),
            server.socket.to_str().unwrap().into(),
        );
        let client = HerdrClient::connect(env, 1.0).unwrap();
        let mut first = engine(&f, client.clone());
        let output = target(&first);
        assert_eq!(first.open_artifact(&output).is_ok(), !lost_reply);
        assert_eq!(f.world.lock().unwrap().created, 1);
        if !lost_reply {
            let mut second = engine(&f, client);
            second.open_artifact(&output).unwrap();
            assert_eq!(f.world.lock().unwrap().created, 2);
            assert_eq!(f.world.lock().unwrap().display_reads, 0);
            let mut stale = output.clone();
            stale.description = "Changed".into();
            assert!(second.open_artifact(&stale).is_err());
        }
        assert!(
            !f.world
                .lock()
                .unwrap()
                .changes
                .iter()
                .any(|resource| resource == "unrelated terminal")
        );
    }
}
