use super::*;
use serde_json::{Value, json};
use std::{cell::Cell, collections::BTreeMap};
use streams::{
    config::{Config, Strategy},
    model::TitleDefaults,
    storage::records::{RecordAddress, RecordChange, RecordStore},
};
mod apply;
mod publication;

#[test]
fn record_selector_discovers_host_qualified_identity_and_refuses_duplicate_uuid_ambiguity() {
    let id = "11234567-89ab-4def-8123-456789abcdef";
    let records: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|host| Record::Stream(Stream::new(id, "Shared", None, host).unwrap()))
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
    fn next_id(&self) -> usize {
        let next = self.sequence.get() + 1;
        self.sequence.set(next);
        next
    }
    fn create(&self, title: &str, goal: Option<&str>) -> Value {
        let stream = Stream::new(
            format!("stream-{}", self.next_id()),
            title,
            goal.map(str::to_owned),
            self.opened.host().as_str(),
        )
        .unwrap();
        let record = Record::Stream(stream);
        let saved = self
            .opened
            .records()
            .save::<_, application::ApplicationError>(
                &RecordAddress::for_record(&record),
                |_, current| {
                    assert!(current.is_none());
                    Ok(RecordChange {
                        record: record.clone(),
                        outcome: (),
                    })
                },
            )
            .unwrap();
        record_to_data(&saved.current)
    }
    fn attach_session(
        &self,
        stream_id: &str,
        native_id: &str,
        status: Status,
    ) -> streams::model::Session {
        let host = self.opened.host().as_str();
        let mut session = streams::model::Session::new(
            streams::model::Session::id_for(host, "pi", native_id),
            host,
            stream_id,
            "pi",
            native_id,
            native_id,
        )
        .unwrap();
        session.status = status;
        let saved =
            application::create_record(self.opened.records(), &Record::Session(session)).unwrap();
        let Record::Session(saved) = saved.current else {
            unreachable!()
        };
        saved
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
        closed: bool,
        all: bool,
    ) -> Result<Output> {
        list_records(&self.opened, query, status, closed, all, None, None)
    }
    fn update(&self, reference: &str, intent: StreamFields) -> Result<Output> {
        update_record(
            &self.opened,
            &self.select(reference),
            None,
            &intent,
            TitleDefaults::default(),
            |_| panic!("unexpected text read"),
        )
    }
    fn set_session_status(
        &self,
        session: &streams::model::Session,
        status: Status,
    ) -> streams::model::Session {
        let saved = application::save_session_edit(
            self.opened.records(),
            session,
            streams::operations::patches::SessionPatch {
                status: Some(status),
                ..Default::default()
            },
            TitleDefaults::default(),
        )
        .unwrap();
        let Record::Session(updated) = saved.current else {
            unreachable!()
        };
        updated
    }
    fn snapshot(&self) -> Value {
        json!(
            self.opened
                .inventory()
                .unwrap()
                .into_iter()
                .map(|read| record_to_data(&read))
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
    let stream = f.create("Straße", Some("Literal [plan]"));
    let id = stream["id"].as_str().unwrap();
    assert_eq!(ok(f.show("STRASSE"))["stream"], stream);
    let continuation = f.create("STRASSE continuation", None);
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
        ));
        assert_eq!(value["stream"]["goal"], "line one\nline two\n");
    }
    assert_eq!(reads, vec![Path::new("goal.txt"), Path::new("-")]);

    let session = f.attach_session(id, "conv-1", Status::Closed);
    assert_eq!(
        ok(f.list(None, &[], false, false))["streams"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let closed_list = ok(f.list(None, &[], true, false));
    assert_eq!(closed_list["streams"][0]["id"], id);
    assert_eq!(closed_list["streams"][0]["status"], "closed");
    assert_eq!(
        ok(f.list(None, &[], false, true))["streams"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let session = f.set_session_status(&session, Status::Done);
    assert_eq!(
        ok(f.list(None, &[Status::Done], false, false))["streams"][0]["id"],
        id
    );
    let _ = f.set_session_status(&session, Status::Active);
    assert_eq!(
        ok(check(&f.opened, Some(id))),
        json!({"valid":true,"streams":1,"sessions":1})
    );
    assert_eq!(
        ok(check(&f.opened, None)),
        json!({"valid":true,"streams":2,"sessions":1})
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
fn repositories_keep_identity_and_reject_invalid_intent_before_save() {
    let f = Fixture::new();
    let stream = f.create("Work", None);
    let id = stream["id"].as_str().unwrap();
    let changed = ok(f.update(
        id,
        StreamFields {
            add_repository: vec!["repo".into(), "https://example.test/repo".into()],
            ..Default::default()
        },
    ));
    assert_eq!(
        changed["stream"]["repositories"]["repo"]["url"],
        "https://example.test/repo"
    );
    let before = f.snapshot();
    let error = f
        .update(
            id,
            StreamFields {
                add_repository: vec!["bad".into(), "".into()],
                ..Default::default()
            },
        )
        .err()
        .unwrap()
        .data();
    assert!(error.get("error").is_some());
    assert_eq!(f.snapshot(), before);
    assert_eq!(ok(f.show(id))["stream"], changed["stream"]);
    let removed = ok(f.update(
        id,
        StreamFields {
            remove_repository: vec!["repo".into()],
            ..Default::default()
        },
    ));
    assert_eq!(removed["stream"]["references"], json!([]));
    assert_eq!(removed["stream"]["repositories"], json!({}));
}
