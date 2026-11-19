use super::*;
use crate::cli::Input;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
};
use streams::{
    config::{Config, Strategy},
    file_store::records::RecordIoEvent,
    model::Record,
    model::{RecordKind, TitleDefaults},
    operations::membership::MoveIntent,
    runtime::conversations::Harness,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordStore},
    },
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
    fn request(&self, stream: Option<&str>) -> EnrollmentRequest {
        let mut wire = json!({"version":2,"catalog_source":self.opened.source_id(),
            "harness":"pi","harness_session_id":NATIVE,"session":{"title":"Contribution"},
            "actor":"automatic","cwd":self.app.cwd});
        if let Some(stream) = stream {
            wire["stream_id"] = json!(stream)
        } else {
            wire["stream"] = json!({"title":"Effort","goal":"Goal"})
        }
        EnrollmentRequest::parse(wire.clone(), self.opened.source_id()).unwrap()
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
        self.opened
            .records()
            .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
                Ok(RecordChange {
                    record,
                    outcome: (),
                })
            })
            .unwrap();
    }
}

#[test]
fn record_only_enrollment_uses_the_admitted_root_without_native_or_settings_rereads_or_publication()
{
    let f = Fixture::new();
    let request = f.request(None);
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
fn enrollment_retried_after_a_move_refuses_without_writing_or_retargeting_directory_intent() {
    let f = Fixture::new();
    f.put(Stream::new("a", "A", None, f.opened.host().as_str()).unwrap());
    let request = f.request(Some("a"));
    let first = f
        .app
        .enroll_records(&f.opened, &request, &f.context())
        .unwrap();
    assert!(first.directory_selected);
    let session = first.session.unwrap();
    let binding = f.opened.locations().unwrap();
    let selected_path = binding.path("a", Some(&session.id));
    let selected = fs::read(&selected_path).unwrap();
    f.put(Stream::new("b", "B", None, f.opened.host().as_str()).unwrap());
    application::move_session_observed(
        f.opened.records(),
        &MoveIntent::new(f.opened.host().as_str(), "pi", NATIVE, "b").unwrap(),
    )
    .unwrap();
    let observed = f.context();
    f.events.lock().unwrap().clear();
    // The record shows a later membership, so the old enrollment cannot be told apart from a
    // new one that would move the conversation; it refuses rather than undo the move.
    let error = f
        .app
        .enroll_records(&f.opened, &request, &observed)
        .err()
        .unwrap();
    assert!(
        error
            .to_string()
            .contains("moving a Session to another Stream is not supported yet"),
        "{error}"
    );
    assert_eq!(fs::read(selected_path).unwrap(), selected);
    assert!(binding.read("b", None).unwrap().is_none());
    let events = f.events.lock().unwrap().clone();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, RecordIoEvent::Written(..))),
        "{events:?}"
    );
}

#[test]
fn directory_failure_preserves_accepted_records_without_claiming_local_completion() {
    let f = Fixture::new();
    let request = f.request(None);
    let effort = streams::agent::effort_id_for(f.opened.host().as_str(), Harness::Pi, NATIVE);
    let directory_path = f.opened.locations().unwrap().path(&effort, None);
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
fn a_failed_session_save_still_reports_the_stream_it_created() {
    let f = Fixture::new();
    let request = f.request(None);
    let host = f.opened.host().as_str();
    let session = request.session(host, "unused").unwrap();
    let address = RecordAddress::new(host, RecordKind::Session, &session.id).unwrap();
    let conversations = f
        .opened
        .root()
        .join(streams::file_store::records::relative_record_path(&address).unwrap())
        .parent()
        .unwrap()
        .to_owned();
    // The Session save fails after the Stream save landed.
    fs::create_dir_all(&conversations).unwrap();
    fs::set_permissions(&conversations, fs::Permissions::from_mode(0o555)).unwrap();
    let completed = f
        .app
        .enroll_records(&f.opened, &request, &f.context())
        .unwrap();
    fs::set_permissions(&conversations, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(completed.output.code, 2, "{}", completed.output.value);
    assert!(completed.output.accepted_save);
    assert!(completed.session.is_none());
    let effort = streams::agent::effort_id_for(host, Harness::Pi, NATIVE);
    assert_eq!(completed.stream.as_ref().unwrap().id, effort);
    assert_eq!(completed.output.value["stream"]["id"], json!(effort));
    assert_eq!(completed.output.value["saves"].as_array().unwrap().len(), 1);
}
