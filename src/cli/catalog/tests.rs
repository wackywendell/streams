use std::cell::Cell;

use serde_json::{Value, json};
use streams_temper::{model::Status, operations::Operation, sync::local::LocalCatalog};

use super::*;

#[path = "../../../tests/support/memory.rs"]
mod memory;

mod apply;
mod publication;

struct Fixture {
    backend: LocalCatalog<memory::MemoryStore>,
    store: memory::MemoryStore,
    sequence: Cell<usize>,
}

impl Fixture {
    fn new() -> Self {
        let store = memory::MemoryStore::default();
        Self {
            backend: LocalCatalog::new(store.clone(), "test-host").unwrap(),
            store,
            sequence: Cell::new(0),
        }
    }

    fn operation_id(&self) -> String {
        let sequence = self.sequence.get() + 1;
        self.sequence.set(sequence);
        format!("operation-{sequence}")
    }

    fn create(&self, title: &str, goal: Option<&str>, status: Status) -> Value {
        let mut stream = Stream::new(
            format!("stream-{}", self.sequence.get()),
            title,
            goal.map(str::to_owned),
            "test-host",
        )
        .unwrap();
        stream.status = status;
        let operation = Operation::new(
            self.operation_id(),
            None,
            Record::Stream(stream),
            Actor::User,
            None,
            vec![],
        )
        .unwrap();
        assert!(self.backend.save(&operation, None).unwrap().accepted());
        record_to_data(operation.after())
    }

    fn select<'a>(&self, catalog: &'a Catalog, reference: &str) -> Result<&'a Stream> {
        resolve(catalog.streams(), reference, |s| (&s.id, &s.title, None))
    }

    fn show(&self, reference: &str) -> Result<Output> {
        show(&self.backend, false, |catalog| {
            self.select(catalog, reference)
        })
    }

    fn update(&self, reference: &str, fields: StreamFields) -> Result<Output> {
        update(
            &self.backend,
            |catalog| self.select(catalog, reference),
            None,
            &fields,
            |_| panic!("unexpected text read"),
            || self.operation_id(),
        )
    }

    fn archive(&self, reference: &str, archived: bool) -> Result<Output> {
        archive(
            &self.backend,
            |catalog| self.select(catalog, reference),
            archived,
            || self.operation_id(),
        )
    }
}

fn ok(result: Result<Output>) -> Value {
    let output = result.unwrap();
    assert_eq!(output.code, 0, "{}", output.value);
    output.value
}

#[test]
fn local_catalog_manages_fields_visibility_and_full_unicode_literal_selection() {
    let f = Fixture::new();
    let stream = f.create("Straße", Some("Literal [plan]"), Status::Waiting);
    let id = stream["id"].as_str().unwrap();
    assert_eq!(ok(f.show("STRASSE"))["stream"], stream);
    let continuation = f.create("STRASSE continuation", None, Status::Waiting);
    assert_eq!(ok(f.show("STRASSE"))["stream"]["id"], id);
    assert_eq!(
        ok(list(
            &f.backend,
            Some("literal ["),
            &[],
            false,
            false,
            false
        ))["streams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let mut reads = Vec::new();
    for path in ["goal.txt", "-"] {
        let fields = StreamFields {
            goal_file: Some(path.into()),
            ..Default::default()
        };
        let value = ok(update(
            &f.backend,
            |catalog| f.select(catalog, id),
            None,
            &fields,
            |path| {
                reads.push(path.to_path_buf());
                Ok("line one\nline two\n".into())
            },
            || f.operation_id(),
        ));
        assert_eq!(value["stream"]["goal"], "line one\nline two\n");
    }
    assert_eq!(reads, vec![Path::new("goal.txt"), Path::new("-")]);

    let archived = ok(f.archive(id, true));
    assert_eq!(archived["stream"]["archived"], true);
    assert_eq!(archived["stream"]["status"], "waiting");
    assert_eq!(
        ok(list(&f.backend, None, &[], false, false, false))["streams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        ok(list(&f.backend, None, &[], true, false, false))["streams"][0]["id"],
        id
    );
    assert_eq!(
        ok(list(&f.backend, None, &[], false, true, false))["streams"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let restored = ok(f.archive(id, false));
    assert_eq!(restored["stream"]["archived"], false);
    assert_eq!(restored["stream"]["status"], "waiting");
    assert_eq!(
        ok(check(&f.backend, Some(id))),
        json!({"valid":true,"streams":1,"sessions":0})
    );
    assert_eq!(
        ok(check(&f.backend, None)),
        json!({"valid":true,"streams":2,"sessions":0})
    );
    assert_eq!(ok(f.show("continu"))["stream"], continuation);
    let error = f.show("STRAS").err().unwrap().data();
    assert_eq!(error["error"]["kind"], "ambiguous");
    let mut candidates: Vec<_> = error["error"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    candidates.sort();
    let mut expected = vec![id, continuation["id"].as_str().unwrap()];
    expected.sort();
    assert_eq!(candidates, expected);
}

#[test]
fn collections_keep_identity_and_reject_invalid_intent_before_save() {
    let f = Fixture::new();
    let stream = f.create("Work", None, Status::Waiting);
    let id = stream["id"].as_str().unwrap();
    let changed = ok(f.update(
        id,
        StreamFields {
            add_repository: vec!["repo".into(), "https://example.test/repo".into()],
            add_document: vec![
                "repo".into(),
                "main".into(),
                "docs/design.md".into(),
                "Design".into(),
            ],
            document_commit: vec!["repo".into(), "docs/design.md".into(), "abc123".into()],
            add_issue: vec!["https://example.test/issue".into(), "Issue".into()],
            ..Default::default()
        },
    ));
    assert_eq!(changed["stream"]["documents"][0]["commit"], "abc123");
    let before = f.backend.snapshot().unwrap();
    let sequence = f.sequence.get();
    let error = f
        .update(
            id,
            StreamFields {
                document_commit: vec!["repo".into(), "missing.md".into(), "abc".into()],
                ..Default::default()
            },
        )
        .err()
        .unwrap()
        .data();
    assert!(error.get("error").is_some());
    assert_eq!(
        f.sequence.get(),
        sequence,
        "validation precedes operation ID generation"
    );
    let after = f.backend.snapshot().unwrap();
    assert_eq!(after.catalog, before.catalog);
    assert_eq!(after.revisions, before.revisions);
    assert_eq!(ok(f.show(id))["stream"], changed["stream"]);
    let removed = ok(f.update(
        id,
        StreamFields {
            remove_document: vec!["repo".into(), "docs/design.md".into()],
            remove_repository: vec!["repo".into()],
            ..Default::default()
        },
    ));
    assert_eq!(removed["stream"]["documents"], json!([]));
    assert_eq!(removed["stream"]["repositories"], json!({}));
}
