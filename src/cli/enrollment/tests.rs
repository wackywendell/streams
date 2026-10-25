use super::*;
use crate::cli::Input;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex},
};
use streams_temper::{
    config::{Config, Strategy},
    model::TitleDefaults,
    operations::membership::MoveIntent,
    runtime::conversations::Harness,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
    },
    yaml_store::records::{RecordIoEvent, relative_record_path},
};

const NATIVE: &str = "11234567-89ab-4def-8123-456789abcdef";
struct Fixture {
    _directory: tempfile::TempDir,
    app: App,
    opened: application::OpenedRecords,
    events: Arc<Mutex<Vec<RecordIoEvent>>>,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let cwd = fs::canonicalize(directory.path()).unwrap();
        let environment = BTreeMap::from([
            ("HOME".into(), cwd.to_str().unwrap().into()),
            ("HERDR_ENV".into(), "1".into()),
            (
                "HERDR_SOCKET_PATH".into(),
                cwd.join("must-not-connect.sock").to_str().unwrap().into(),
            ),
        ]);
        let app = App {
            config: Config::new(&environment, &cwd).unwrap(),
            environment,
            cwd,
            input: Input::Stdin,
            herdr: None,
        };
        application::initialize_records(&app.config, Strategy::None, None, "main").unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let opened = application::open_records_observed(&app.config, move |event| {
            observed.lock().unwrap().push(event)
        })
        .unwrap();
        Self {
            _directory: directory,
            app,
            opened,
            events,
        }
    }
    fn request(&self, id: &str, stream: Option<&str>) -> EnrollmentRequest {
        let mut wire = json!({"version":2,"catalog_source":self.opened.source_id(),"operation_id":id,
            "harness":"pi","harness_session_id":NATIVE,"session":{"title":"Contribution"},
            "actor":"automatic","cwd":self.app.cwd});
        if let Some(stream) = stream {
            wire["stream_id"] = json!(stream)
        } else {
            wire["stream"] = json!({"title":"Effort","goal":"Goal"})
        }
        EnrollmentRequest::parse(&wire.to_string(), self.opened.source_id()).unwrap()
    }
    fn context(&self) -> RecordContext {
        self.app
            .context_from_records(
                &self.opened,
                ContextRequest {
                    harness: Harness::Pi,
                    native_id: NATIVE,
                    stream_id: None,
                    environment_stream_id: None,
                    workspace_stream_id: None,
                },
                TitleDefaults::default(),
            )
            .unwrap()
    }
    fn put(&self, stream: Stream) {
        let record = Record::Stream(stream);
        let request =
            RecordRequest::new(RecordAddress::for_record(&record), "fixture", &json!({})).unwrap();
        self.opened
            .records()
            .save::<StorageError>(&request, |_, _| {
                Ok(RecordChange {
                    record,
                    outcome: Default::default(),
                })
            })
            .unwrap();
    }
}

#[test]
fn record_only_enrollment_uses_the_admitted_root_without_native_or_settings_rereads_or_publication()
{
    let f = Fixture::new();
    let request = f.request("initial", None);
    let observed = f.context();
    // Runtime path derivation uses the admitted root and immutable Config paths, not settings.
    fs::write(f.app.config.config_path(), "not JSON").unwrap();
    let completed = f
        .app
        .enroll_records(&f.opened, &request, &observed)
        .unwrap();
    assert_eq!(completed.output.code, 0, "{}", completed.output.value);
    assert!(completed.output.accepted_save);
    assert!(completed.directory_selected);
    assert_eq!(
        completed.session.as_ref().unwrap().stream_id,
        completed.stream.as_ref().unwrap().id
    );
    assert_eq!(completed.output.value["saves"].as_array().unwrap().len(), 2);
    assert!(completed.output.value.get("authority").is_none());
    assert!(completed.output.value.get("publication").is_none());
    assert!(
        completed.output.value["saves"][0]["save"]
            .get("sharing")
            .is_none()
    );
}

#[test]
fn retained_enrollment_keeps_its_original_decision_and_current_records_without_retargeting_directory_intent()
 {
    let f = Fixture::new();
    f.put(Stream::new("a", "A", None, f.opened.host().as_str()).unwrap());
    let request = f.request("initial", Some("a"));
    let first = f
        .app
        .enroll_records(&f.opened, &request, &f.context())
        .unwrap();
    assert!(first.directory_selected);
    let session = first.session.unwrap();
    let binding = f
        .app
        .locations(f.opened.root(), f.opened.host().as_str())
        .unwrap();
    let selected_path = binding.path("a", Some(&session.id));
    let selected = fs::read(&selected_path).unwrap();
    f.put(Stream::new("b", "B", None, f.opened.host().as_str()).unwrap());
    application::move_session(
        f.opened.records(),
        "move",
        &MoveIntent::new(f.opened.host().as_str(), "pi", NATIVE, "b").unwrap(),
    )
    .unwrap();
    let observed = f.context();
    f.events.lock().unwrap().clear();
    let retry = f
        .app
        .enroll_records(&f.opened, &request, &observed)
        .unwrap();
    assert!(retry.output.accepted_save);
    assert_eq!(retry.output.code, 2);
    assert!(!retry.directory_selected);
    assert_eq!(retry.session.as_ref().unwrap().stream_id, "b");
    assert_eq!(retry.stream.as_ref().unwrap().id, "b");
    assert_eq!(
        retry.output.value["saves"][0]["save"]["domain"]["stream_id"],
        "a"
    );
    assert_eq!(fs::read(selected_path).unwrap(), selected);
    assert!(binding.read("b", None).unwrap().is_none());
    let reads = f
        .events
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            RecordIoEvent::Read(path) => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reads,
        [
            f.opened.root().join(".catalog-local/records.json"),
            f.opened.root().join(
                relative_record_path(&RecordAddress::for_record(&Record::Session(session)))
                    .unwrap()
            )
        ]
    );
}

#[test]
fn directory_failure_preserves_accepted_records_without_claiming_local_completion() {
    let f = Fixture::new();
    let request = f.request("initial", None);
    let effort =
        streams_temper::agent::effort_id_for(f.opened.host().as_str(), Harness::Pi, NATIVE);
    let directory_path = f
        .app
        .locations(f.opened.root(), f.opened.host().as_str())
        .unwrap()
        .path(&effort, None);
    let blocked = directory_path.parent().unwrap();
    fs::create_dir_all(blocked.parent().unwrap()).unwrap();
    fs::write(blocked, "not a directory").unwrap();
    let completed = f
        .app
        .enroll_records(&f.opened, &request, &f.context())
        .unwrap();
    assert!(completed.output.accepted_save);
    assert_eq!(completed.output.code, 2);
    assert!(!completed.directory_selected);
    assert!(completed.session.is_some());
    assert!(completed.stream.is_some());
    assert!(completed.output.value.get("completion_error").is_some());
    assert_eq!(completed.output.value["saves"].as_array().unwrap().len(), 2);
    assert_eq!(f.context().session, completed.session);
}

#[test]
fn uncertain_enrollment_keeps_its_receipt_without_claiming_definite_acceptance() {
    let mut receipt = Receipt::default();
    assert!(
        receipt
            .persist(
                "initial:session".into(),
                Err(StorageError::uncertain("replacement flush failed").into())
            )
            .is_err()
    );
    assert!(!receipt.accepted);
    let value = serde_json::to_value(receipt).unwrap();
    assert_eq!(value["saves"][0]["save"]["local"], "uncertain");
    assert!(value["saves"][0]["save"]["domain"].is_null());
}
