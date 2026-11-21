use super::*;
use crate::model::{ReferenceEntry, ReferenceKind, ReferenceRole, ReferenceStatus};
use crate::tui::{DocumentLocation, ResolvedDocument};

struct DocumentSource(PathBuf, Option<&'static str>);
impl DocumentSource {
    fn display(&self, owner: &HerdrInventoryPane) -> Result<CardSnapshot> {
        let original = card(owner);
        let mut session = original.session().clone();
        let mut entry = ReferenceEntry {
            kind: ReferenceKind::Document,
            name: "Design".into(),
            purpose: "Guide the work".into(),
            role: ReferenceRole::Brief,
            status: ReferenceStatus::Active,
            manual: false,
        };
        if let Some(change) = self.1 {
            match change {
                "name" => entry.name = "New name".into(),
                "purpose" => entry.purpose = "New purpose".into(),
                "role" => entry.role = ReferenceRole::Dependency,
                "dropped" => entry.status = ReferenceStatus::Dropped,
                _ => unreachable!(),
            }
        }
        let visible = entry.status != ReferenceStatus::Dropped;
        session.references.insert("design.md".into(), entry);
        CardSnapshot::new(
            original.stream().clone(),
            session,
            DisplayContext {
                catalog_source: original.scope().catalog_source.clone(),
                owner: owner::location(owner),
                documents: if visible {
                    vec![DocumentLocation {
                        path: "design.md".into(),
                        location: ResolvedDocument::File {
                            root: self.0.clone(),
                            file: self.0.join("design.md"),
                        },
                    }]
                } else {
                    vec![]
                },
            },
        )
        .map_err(|error| invalid(error.to_string()))
    }
}
impl NativeCardSource for DocumentSource {
    fn read_records(&mut self, owner: &HerdrInventoryPane) -> Result<CardRecords> {
        Ok(self.display(owner)?.into_records())
    }
    fn read(&mut self, owner: &HerdrInventoryPane) -> Result<CardSnapshot> {
        self.display(owner)
    }
    fn resolve_document(&mut self, target: &DocumentTarget) -> Result<ResolvedEditor> {
        Ok(ResolvedEditor {
            root: self.0.clone(),
            identity: target.editor_identity().unwrap(),
        })
    }
    fn launch_context(&self) -> SourceLaunch {
        SourceLaunch {
            environment: BTreeMap::new(),
            cwd: self.0.clone(),
        }
    }
}

#[test]
fn document_open_reuses_the_exact_editor_and_accepts_a_tab_info_focus_reply() {
    let f = Fixture::new();
    let root = f.directory.path().canonicalize().unwrap();
    std::fs::write(root.join("design.md"), "Design").unwrap();
    let mut engine = f.engine().unwrap();
    let source = DocumentSource(root, None);
    let target = source
        .display(&engine.owner_pane)
        .unwrap()
        .document_target("design.md")
        .unwrap();
    let identity = target.editor_identity().unwrap();
    let key = json_digest(&(&identity.catalog_source, &identity.host_id, &identity.file)).unwrap();
    {
        let mut world = f.world.lock().unwrap();
        let mut editor = pane("editor", "editor-terminal", "w", "t", "document editor");
        editor["tokens"] = json!({"strm_role":"editor","strm_doc":key});
        world.snapshot["panes"].as_array_mut().unwrap().push(editor);
        world.refresh();
        engine.snapshot = world.observe();
    }
    engine.source = Box::new(source);
    f.server.clear();
    engine.open_document(&target).unwrap();
    assert_eq!(f.server.methods(), vec!["tab.focus"]);
}

#[test]
fn document_open_refuses_fresh_target_relationship_and_dropped_changes() {
    for change in ["name", "purpose", "role", "dropped"] {
        let f = Fixture::new();
        let root = f.directory.path().canonicalize().unwrap();
        std::fs::write(root.join("design.md"), "Design").unwrap();
        let mut engine = f.engine().unwrap();
        let painted = DocumentSource(root.clone(), None)
            .display(&engine.owner_pane)
            .unwrap();
        let target = painted.document_target("design.md").unwrap();
        engine.source = Box::new(DocumentSource(root, Some(change)));
        f.server.clear();
        assert!(
            engine
                .open_document(&target)
                .unwrap_err()
                .to_string()
                .contains("document viewing context changed"),
            "{change}"
        );
        assert!(f.server.methods().is_empty(), "{change}");
    }
}
