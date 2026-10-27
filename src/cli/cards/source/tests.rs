use super::*;
use crate::cli::Input;
use serde_json::json;
use std::{collections::BTreeMap, fs, sync::Mutex};
use streams_temper::{
    agent::{self, ContextRequest},
    config::Config,
    frontend::herdr::HerdrInventoryPane,
    model::{Document, Record, Repository, Session, Stream},
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
    },
    yaml_store::records::{RecordIoEvent, YamlRecordStore, relative_record_path},
};
const NATIVE: &str = "11234567-89ab-4def-8123-456789abcdef";
fn save(records: &application::OpenedRecords, operation: &str, record: Record) {
    let request = RecordRequest::new(
        RecordAddress::for_record(&record),
        operation,
        &json!({"record":record}),
    )
    .unwrap();
    records
        .records()
        .save::<StorageError>(&request, |_, _| {
            Ok(RecordChange {
                record,
                outcome: json!({"outcome":"changes"}).as_object().unwrap().clone(),
            })
        })
        .unwrap();
}
#[test]
fn source_reuses_supplied_records_and_each_later_read_addresses_only_its_session_and_stream() {
    for unrelated in [0, 64] {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().canonicalize().unwrap();
        let environment = BTreeMap::from([("HOME".into(), cwd.to_str().unwrap().into())]);
        let app = App {
            config: Config::new(&environment, &cwd).unwrap(),
            environment,
            cwd,
            input: Input::Stdin,
            herdr: None,
        };
        let settings = app.config.load().unwrap();
        fs::create_dir_all(app.config.host_path().parent().unwrap()).unwrap();
        fs::write(
            app.config.host_path(),
            json!({"version":1,"id":"01234567-89ab-4def-8123-456789abcdef"}).to_string(),
        )
        .unwrap();
        let host = app.config.host_identity().unwrap();
        YamlRecordStore::new(&settings.catalog_root, host.as_str())
            .unwrap()
            .initialize()
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let records = Arc::new(
            application::open_records_observed(&app.config, move |event| {
                observed.lock().unwrap().push(event)
            })
            .unwrap(),
        );
        let mut stream = Stream::new("effort", "Effort", None, host.as_str()).unwrap();
        for index in 0..4 {
            let alias = format!("repository-{index}");
            stream.repositories.insert(
                alias.clone(),
                Repository::new(format!("unavailable-{index}")).unwrap(),
            );
            stream
                .documents
                .push(Document::new(alias, "main", "not-present.md", "", None).unwrap());
        }
        let mut session = Session::new(
            agent::session_id_for(host.as_str(), Harness::Pi, NATIVE),
            host.as_str(),
            "effort",
            "pi",
            NATIVE,
            "Conversation",
        )
        .unwrap();
        save(&records, "stream", Record::Stream(stream.clone()));
        save(&records, "session", Record::Session(session.clone()));
        let neighbor = records.root().join("hosts/unrelated/streams");
        fs::create_dir_all(&neighbor).unwrap();
        for index in 0..unrelated {
            fs::write(neighbor.join(format!("bad-{index}.yaml")), "not a record").unwrap();
        }
        let owner=HerdrInventoryPane::parse(&json!({"pane_id":"pi","tab_id":"tab","workspace_id":"workspace","terminal_id":"terminal"})).unwrap();
        let context = app
            .context_from_records(
                &records,
                ContextRequest {
                    harness: Harness::Pi,
                    native_id: NATIVE,
                    stream_id: None,
                    environment_stream_id: None,
                    workspace_stream_id: None,
                },
                settings.naming,
            )
            .unwrap();
        events.lock().unwrap().clear();
        let mut source = ApplicationSource::new(
            app.clone(),
            records.clone(),
            NATIVE.into(),
            None,
            Some(context),
        );
        let frame = source.read_records(&owner).unwrap();
        assert_eq!(frame.session().id, session.id);
        assert_eq!(frame.stream().documents.len(), 4);
        assert!(
            events.lock().unwrap().is_empty(),
            "provided context must not reopen or reread catalog records"
        );
        session.title = "New title".into();
        save(&records, "rename", Record::Session(session.clone()));
        events.lock().unwrap().clear();
        assert_eq!(
            source.read_records(&owner).unwrap().session().title,
            "New title"
        );
        let reads: Vec<_> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                RecordIoEvent::Read(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            reads,
            vec![
                records.root().join(".catalog-local/records.json"),
                records.root().join(
                    relative_record_path(&RecordAddress::for_record(&Record::Session(
                        session.clone()
                    )))
                    .unwrap()
                ),
                records.root().join(
                    relative_record_path(&RecordAddress::for_record(&Record::Stream(stream)))
                        .unwrap()
                )
            ]
        );
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, RecordIoEvent::List(_)))
        );
        let destination = Stream::new("destination", "Destination", None, host.as_str()).unwrap();
        save(&records, "destination", Record::Stream(destination));
        let intent = streams_temper::operations::membership::MoveIntent::new(
            host.as_str(),
            "pi",
            NATIVE,
            "destination",
        )
        .unwrap();
        application::move_session_observed(records.records(), "move", &intent).unwrap();
        let mut source = ApplicationSource::new(
            app,
            records,
            NATIVE.into(),
            Some(frame.scope().clone()),
            None,
        );
        let moved = source.read_records(&owner).unwrap();
        assert_eq!(moved.scope().stream_id, "destination");
        assert!(frame.scope().same_conversation(moved.scope()));
    }
}
