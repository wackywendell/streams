use super::*;
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap};
use streams_temper::{
    config::{Config, Strategy},
    model::TitleDefaults,
    storage::records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
};
mod apply;
mod publication;

#[test]
fn record_selector_discovers_host_qualified_identity_and_refuses_duplicate_uuid_ambiguity() {
    let id = "11234567-89ab-4def-8123-456789abcdef";
    let records: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|host| streams_temper::storage::records::RecordRead {
            record: Record::Stream(Stream::new(id, "Shared", None, host).unwrap()),
            revision: "observed".into(),
        })
        .collect();
    assert!(select_stream(&records, id).is_err());
    assert!(select_stream(&records, "Shared").is_err());
    assert_eq!(
        select_stream(&records, &format!("a/{id}"))
            .unwrap()
            .owner_host_id,
        "a"
    );
    assert_eq!(
        select_stream(&records, &format!("b/{id}"))
            .unwrap()
            .owner_host_id,
        "b"
    );
}

struct Fixture {
    _directory: tempfile::TempDir,
    opened: OpenedRecords,
    sequence: Cell<usize>,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let env = BTreeMap::from([("HOME".into(), root.to_str().unwrap().to_owned())]);
        let config = Config::new(&env, &root).unwrap();
        let opened =
            application::initialize_records(&config, Strategy::None, None, "main").unwrap();
        Self {
            _directory: directory,
            opened,
            sequence: Cell::new(0),
        }
    }
    fn operation_id(&self) -> String {
        let next = self.sequence.get() + 1;
        self.sequence.set(next);
        format!("operation-{next}")
    }
    fn create(&self, title: &str, goal: Option<&str>, status: Status) -> Value {
        let mut stream = Stream::new(
            format!("stream-{}", self.sequence.get()),
            title,
            goal.map(str::to_owned),
            self.opened.host().as_str(),
        )
        .unwrap();
        stream.status = status;
        let record = Record::Stream(stream);
        let request = RecordRequest::new(
            RecordAddress::for_record(&record),
            self.operation_id(),
            &json!({"create":record_to_data(&record)}),
        )
        .unwrap();
        let saved = self
            .opened
            .records()
            .save::<application::ApplicationError>(&request, |_, current| {
                assert!(current.is_none());
                Ok(RecordChange {
                    record: record.clone(),
                    outcome: json!({"outcome":"changes"}).as_object().unwrap().clone(),
                })
            })
            .unwrap();
        record_to_data(&saved.current.record)
    }
    fn select(&self, reference: &str) -> Stream {
        select_stream(&self.opened.inventory().unwrap(), reference)
            .unwrap()
            .clone()
    }
    fn show(&self, reference: &str) -> Result<Output> {
        let records = self.opened.inventory()?;
        Ok(Output::value(
            json!({"stream":select_stream(&records,reference)?}),
        ))
    }
    fn list(
        &self,
        query: Option<&str>,
        status: &[Status],
        archived: bool,
        all: bool,
    ) -> Result<Output> {
        list_records(&self.opened, query, status, archived, all, None)
    }
    fn update(&self, reference: &str, intent: StreamFields) -> Result<Output> {
        update_record(
            &self.opened,
            &self.select(reference),
            None,
            &intent,
            TitleDefaults::default(),
            |_| panic!("unexpected text read"),
            || self.operation_id(),
        )
    }
    fn archive(&self, reference: &str, archived: bool) -> Result<Output> {
        save_stream_edit(
            &self.opened,
            &self.select(reference),
            json!({"archived":archived}).as_object().unwrap().clone(),
            TitleDefaults::default(),
            &self.operation_id(),
        )
    }
    fn snapshot(&self) -> Value {
        json!(
            self.opened
                .inventory()
                .unwrap()
                .into_iter()
                .map(|read| (read.revision, record_to_data(&read.record)))
                .collect::<Vec<_>>()
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
        ok(f.list(Some("literal ["), &[], false, false))["streams"]
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
        let value = ok(update_record(
            &f.opened,
            &f.select(id),
            None,
            &fields,
            TitleDefaults::default(),
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
        ok(f.list(None, &[], false, false))["streams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(ok(f.list(None, &[], true, false))["streams"][0]["id"], id);
    assert_eq!(
        ok(f.list(None, &[], false, true))["streams"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let restored = ok(f.archive(id, false));
    assert_eq!(restored["stream"]["archived"], false);
    assert_eq!(restored["stream"]["status"], "waiting");
    assert_eq!(
        ok(check(&f.opened, Some(id))),
        json!({"valid":true,"streams":1,"sessions":0})
    );
    assert_eq!(
        ok(check(&f.opened, None)),
        json!({"valid":true,"streams":2,"sessions":0})
    );
    assert_eq!(ok(f.show("continu"))["stream"], continuation);
    let error = f.show("STRAS").err().unwrap().data();
    assert_eq!(error["error"]["kind"], "ambiguous");
    let mut candidates: Vec<_> = error["error"]["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    candidates.sort();
    let host = f.opened.host().as_str();
    let mut expected = vec![
        format!("{host}/{id}"),
        format!("{host}/{}", continuation["id"].as_str().unwrap()),
    ];
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
    let before = f.snapshot();
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
    assert_eq!(f.snapshot(), before);
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
