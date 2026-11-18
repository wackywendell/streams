use super::*;
use crate::cli::Input;
use serde_json::json;
use std::{collections::BTreeMap, fs, sync::Mutex};
use streams::{
    agent::{self, ContextRequest},
    config::Config,
    file_store::records::{FileRecordStore, RecordIoEvent, relative_record_path},
    frontend::herdr::HerdrInventoryPane,
    model::{Record, Reference, Session, Stream},
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordStore},
    },
};
const NATIVE: &str = "11234567-89ab-4def-8123-456789abcdef";

fn inspect_fixture_checkout(
    path: &std::path::Path,
) -> std::result::Result<
    Option<streams::runtime::checkouts::RepositoryInspection>,
    streams::runtime::RuntimeError,
> {
    Ok(path
        .ancestors()
        .find(|root| root.join(".fixture-checkout").exists())
        .map(|root| streams::runtime::checkouts::RepositoryInspection {
            root: root.to_path_buf(),
            relative_cwd: path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(".")
                .into(),
        }))
}

struct Hints {
    _temp: tempfile::TempDir,
    root: PathBuf,
    registry: streams::runtime::CheckoutRegistry,
    locations: streams::runtime::DirectoryBindings,
    stream: Stream,
    session: Session,
}
impl Hints {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let registry = streams::runtime::CheckoutRegistry::new(root.join("registry"), "host")
            .unwrap()
            .with_inspector(inspect_fixture_checkout);
        let locations =
            streams::runtime::DirectoryBindings::new(root.join("locations"), registry.clone());
        Self {
            _temp: temp,
            root,
            registry,
            locations,
            stream: Stream::new("effort", "Effort", None, "host").unwrap(),
            session: Session::new("session", "host", "effort", "pi", NATIVE, "Session").unwrap(),
        }
    }
    fn checkout(&self, name: &str, url: &str) -> streams::runtime::checkouts::CheckoutLocation {
        let path = self.root.join(name);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join(".fixture-checkout"), "").unwrap();
        fs::write(path.join("plan.md"), "Plan").unwrap();
        self.registry.register(&path, Some(url)).unwrap()
    }
    fn document(&mut self, url: &str, path: &str) {
        self.stream.references.push(
            serde_json::from_value(json!({
                "name":path,"purpose":"Guide work","role":"brief","state":"active",
                "target":{"kind":"document","repository_url":url,"path":path}
            }))
            .unwrap(),
        );
    }
    fn context(&self) -> serde_json::Value {
        documents::context(&self.registry, &self.locations, &self.stream, &self.session)
    }
}

#[test]
fn direct_urls_resolve_explicit_defaults_without_selection_or_portable_checkout_id() {
    for plain_directory in [false, true] {
        for portable_url in [false, true] {
            let mut f = Hints::new();
            let checkout = f.checkout("default", "repository");
            f.registry.set_default("repository", &checkout.id).unwrap();
            f.document("repository", "plan.md");
            if portable_url {
                let mut portable =
                    streams::model::Checkout::new("not-an-alias", "main", ".").unwrap();
                portable.repository_url = Some("repository".into());
                f.session.checkout = Some(portable);
            }
            if plain_directory {
                let plain = f.root.join("plain");
                fs::create_dir(&plain).unwrap();
                f.locations
                    .remember("effort", &plain, Some("session"), false, None, None)
                    .unwrap();
            }
            let context = f.context();
            assert!(f.stream.repositories.is_empty());
            assert_eq!(context["checkouts"][0]["repository_url"], "repository");
            assert_eq!(context["document_locations"][0]["checkout_id"], checkout.id);
            assert_eq!(context["document_locations"][0]["kind"], "local");
            assert_eq!(
                context["document_locations"][0]["error"],
                serde_json::Value::Null
            );
        }
    }
}

#[test]
fn selected_checkout_contributes_alias_free_hints_before_any_document_and_wins_over_defaults() {
    let mut f = Hints::new();
    let selected = f.checkout("selected", "repository");
    f.registry
        .register(&selected.directory, Some("second-url"))
        .unwrap();
    let default = f.checkout("default", "repository");
    f.registry.set_default("repository", &default.id).unwrap();
    f.locations
        .remember(
            "effort",
            &selected.directory,
            Some("session"),
            false,
            None,
            Some(&selected.id),
        )
        .unwrap();
    let context = f.context();
    assert_eq!(context["checkouts"].as_array().unwrap().len(), 2);
    assert!(
        context["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["checkout_id"] == selected.id)
    );
    assert_eq!(context["document_locations"], json!([]));
    f.document("repository", "plan.md");
    assert_eq!(
        f.context()["document_locations"][0]["checkout_id"],
        selected.id
    );
    let foreign = f.checkout("foreign", "foreign-repository");
    f.registry
        .set_default("foreign-repository", &foreign.id)
        .unwrap();
    f.document("foreign-repository", "plan.md");
    assert_eq!(
        f.context()["document_locations"][1]["checkout_id"],
        foreign.id
    );
    let mut portable = streams::model::Checkout::new("alias", "main", ".").unwrap();
    portable.repository_url = Some("foreign-repository".into());
    f.session.checkout = Some(portable);
    assert!(
        f.context()["document_locations"][1]["error"]
            .as_str()
            .unwrap()
            .contains("lacks an explicit repository URL binding")
    );
}

#[test]
fn missing_defaults_and_files_preserve_unavailable_reference_locations() {
    let mut f = Hints::new();
    let checkout = f.checkout("checkout", "repository");
    f.document("repository", "plan.md");
    assert!(
        f.context()["document_locations"][0]["error"]
            .as_str()
            .unwrap()
            .contains("no explicit checkout default")
    );
    f.registry.set_default("repository", &checkout.id).unwrap();
    fs::remove_file(checkout.directory.join("plan.md")).unwrap();
    let context = f.context();
    assert_eq!(context["document_locations"][0]["checkout_id"], checkout.id);
    assert!(context["document_locations"][0]["kind"].is_null());
    assert!(
        context["document_locations"][0]["error"]
            .as_str()
            .unwrap()
            .contains("Document not present")
    );
    assert_eq!(f.stream.references.len(), 1);
}

#[test]
fn broken_explicit_selection_and_unregistered_vcs_never_fall_back_to_a_valid_default() {
    for broken in [
        "removed",
        "corrupt-binding",
        "unregistered-vcs",
        "registry-error",
    ] {
        let mut f = Hints::new();
        let checkout = f.checkout("default", "repository");
        f.registry.set_default("repository", &checkout.id).unwrap();
        f.document("repository", "plan.md");
        let plain = f.root.join("plain");
        fs::create_dir(&plain).unwrap();
        f.locations
            .remember("effort", &plain, Some("session"), false, None, None)
            .unwrap();
        match broken {
            "removed" => fs::remove_dir(&plain).unwrap(),
            "corrupt-binding" => {
                fs::write(f.locations.path("effort", Some("session")), "invalid").unwrap()
            }
            "unregistered-vcs" => fs::create_dir(plain.join(".git")).unwrap(),
            _ => fs::write(f.registry.path(), "invalid").unwrap(),
        }
        let context = f.context();
        assert_eq!(context["checkouts"], json!([]), "{broken}");
        let location = &context["document_locations"][0];
        assert!(
            location["kind"].is_null() && location["checkout_id"].is_null(),
            "{broken}"
        );
        assert!(!location["error"].as_str().unwrap().is_empty());
    }
}

fn save(records: &application::OpenedRecords, record: Record) {
    records
        .records()
        .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
            Ok(RecordChange {
                record,
                outcome: (),
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
        FileRecordStore::new(&settings.catalog_root, host.as_str())
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
            stream.references.push(serde_json::from_value::<Reference>(json!({
                "name":"Missing document","purpose":"Guide the work","role":"brief","state":"active",
                "target":{"kind":"document","repository_url":format!("unavailable-{index}"),"path":"not-present.md","ref":"main"}
            })).unwrap());
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
        save(&records, Record::Stream(stream.clone()));
        save(&records, Record::Session(session.clone()));
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
        assert_eq!(frame.stream().references.len(), 4);
        let display = source.display(frame.clone()).unwrap();
        for reference in &display.stream().references {
            let streams::model::ReferenceTarget::Document(document) = &reference.target else {
                panic!()
            };
            let target = display
                .document_target(&document.repository_url, &document.path)
                .unwrap();
            assert!(matches!(
                target.location,
                ResolvedDocument::Unavailable { .. }
            ));
            assert_eq!(&target.reference, reference);
        }
        assert!(
            events.lock().unwrap().is_empty(),
            "provided context must not reopen or reread catalog records"
        );
        session.title = "New title".into();
        save(&records, Record::Session(session.clone()));
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
        save(&records, Record::Stream(destination));
        let intent = streams::operations::membership::MoveIntent::new(
            host.as_str(),
            "pi",
            NATIVE,
            "destination",
        )
        .unwrap();
        application::move_session_observed(records.records(), &intent).unwrap();
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
