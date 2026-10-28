use super::*;
use crate::tui::{CheckoutBinding, CheckoutSelection, DocumentLocation, ResolvedDocument};

struct DocumentSource(PathBuf);
impl DocumentSource {
    fn display(&self, owner: &HerdrInventoryPane) -> Result<CardSnapshot> {
        let original = card(owner);
        let mut stream = original.stream().clone();
        stream.repositories.insert(
            "repo".into(),
            crate::model::Repository::new("repository").unwrap(),
        );
        stream.documents.push(
            crate::model::Document::new("repo", "main", "design.md", "Design", None).unwrap(),
        );
        let selection = CheckoutSelection {
            checkout_id: "checkout".into(),
            root: self.0.clone(),
        };
        CardSnapshot::new(
            stream,
            original.session().clone(),
            DisplayContext {
                catalog_source: original.scope().catalog_source.clone(),
                owner: owner::location(owner),
                checkouts: vec![CheckoutBinding {
                    repository: "repo".into(),
                    selection: selection.clone(),
                }],
                documents: vec![DocumentLocation {
                    repository: "repo".into(),
                    path: "design.md".into(),
                    location: ResolvedDocument::File {
                        selection,
                        file: self.0.join("design.md"),
                    },
                }],
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
    let source = DocumentSource(root);
    let target = source
        .display(&engine.owner_pane)
        .unwrap()
        .document_target("repo", "design.md")
        .unwrap();
    let identity = target.editor_identity().unwrap();
    let key = json_digest(&(
        &identity.catalog_source,
        &identity.host_id,
        &identity.checkout_id,
        &identity.file,
    ))
    .unwrap();
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
