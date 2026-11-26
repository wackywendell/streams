use crate::support::record_io;
use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex},
};

use serde_json::json;
use streams::{
    application,
    config::Config,
    file_store::records::{FileRecordStore, RecordIoEvent, relative_record_path},
    model::{Record, Session, Stream, Title},
    operations::edits::EditRequest,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordStore},
    },
};

const HOST: &str = "01234567-89ab-4def-8123-456789abcdef";
const OTHER: &str = "11234567-89ab-4def-8123-456789abcdef";

#[test]
fn one_record_creation_reads_only_its_target_and_membership_and_a_retry_writes_nothing() {
    for unrelated in [0, 64] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let store = FileRecordStore::new(root, HOST)
            .unwrap()
            .with_io_observer(move |event| observed.lock().unwrap().push(event));
        store.initialize().unwrap();
        for index in 0..unrelated {
            let path = root.join(format!("hosts/unrelated-{index}/conversations/bad.json"));
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "not a record").unwrap();
        }
        let stream =
            Record::Stream(Stream::new(HOST, "work", Title::new("Work", false), None).unwrap());
        application::create_record(&store, &stream).unwrap();
        let session = Record::Session(
            Session::new(HOST, "pi", "native", "work", Title::new("Lane", false)).unwrap(),
        );
        let target = root.join(relative_record_path(&RecordAddress::for_record(&session)).unwrap());
        let dependency =
            root.join(relative_record_path(&RecordAddress::for_record(&stream)).unwrap());
        let marker = root.join(".catalog-local/records.json");
        events.lock().unwrap().clear();
        let created = application::create_record(&store, &session).unwrap();
        let trace = events.lock().unwrap().clone();
        assert_eq!(
            record_io::accesses(&trace),
            vec![
                RecordIoEvent::Lock(root.join(".catalog.lock")),
                RecordIoEvent::Read(marker.clone()),
                RecordIoEvent::Read(target.clone()),
                RecordIoEvent::Read(dependency.clone()),
            ]
        );
        assert_eq!(
            record_io::bytes_read(&trace),
            (fs::metadata(&marker).unwrap().len() + fs::metadata(&dependency).unwrap().len())
                as usize
        );
        assert_eq!(
            record_io::writes(&trace),
            vec![(
                target.clone(),
                fs::metadata(&target).unwrap().len() as usize
            )]
        );
        let bytes = fs::read(&target).unwrap();
        events.lock().unwrap().clear();
        let retry = application::create_record(&store, &session).unwrap();
        assert!(!retry.written);
        assert_eq!(retry.current, created.current);
        assert_eq!(fs::read(&target).unwrap(), bytes);
        let trace = events.lock().unwrap().clone();
        assert_eq!(
            record_io::accesses(&trace),
            vec![
                RecordIoEvent::Lock(root.join(".catalog.lock")),
                RecordIoEvent::Read(marker.clone()),
                RecordIoEvent::Read(target.clone())
            ]
        );
        assert_eq!(
            record_io::bytes_read(&trace),
            fs::metadata(marker).unwrap().len() as usize + bytes.len()
        );
        assert!(record_io::writes(&trace).is_empty());
        let Record::Session(created) = &created.current else {
            panic!("created a Session")
        };
        let req = EditRequest::parse(&json!({
            "changes": {"next": {"text": "Later progress", "short": null}}
        }))
        .unwrap();
        application::edit_session(&store, &RecordAddress::for_session(created), &req).unwrap();
        let bytes = fs::read(&target).unwrap();
        // Creating again for the SAME Stream changes nothing.
        let retry_same_stream = application::create_record(&store, &session).unwrap();
        assert!(!retry_same_stream.written);
        assert_eq!(fs::read(&target).unwrap(), bytes);
        // Creating again for a DIFFERENT Stream is refused.
        let other_stream =
            Record::Stream(Stream::new(HOST, "other", Title::new("Other", false), None).unwrap());
        application::create_record(&store, &other_stream).unwrap();
        let different = Record::Session(
            Session::new(HOST, "pi", "native", "other", Title::new("Lane", false)).unwrap(),
        );
        assert!(application::create_record(&store, &different).is_err());
        assert_eq!(fs::read(target).unwrap(), bytes);
    }
}

#[test]
fn session_creation_refuses_a_missing_same_host_stream_without_installing_a_target() {
    let directory = tempfile::tempdir().unwrap();
    let store = FileRecordStore::new(directory.path(), HOST).unwrap();
    store.initialize().unwrap();
    let session = Record::Session(
        Session::new(HOST, "pi", "native", "missing", Title::new("Lane", false)).unwrap(),
    );
    assert!(application::create_record(&store, &session).is_err());
    assert!(
        !directory
            .path()
            .join(relative_record_path(&RecordAddress::for_record(&session)).unwrap())
            .exists()
    );
}

fn fixture_config(directory: &std::path::Path) -> Config {
    Config::new(
        &BTreeMap::from([
            ("HOME".into(), directory.to_str().unwrap().into()),
            (
                "XDG_CONFIG_HOME".into(),
                directory.join("config").to_str().unwrap().into(),
            ),
            (
                "XDG_DATA_HOME".into(),
                directory.join("data").to_str().unwrap().into(),
            ),
        ]),
        directory,
    )
    .unwrap()
}

fn host(config: &Config, id: &str) {
    fs::create_dir_all(config.host_path().parent().unwrap()).unwrap();
    fs::write(config.host_path(), json!({"version":1,"id":id}).to_string()).unwrap();
}

#[test]
fn record_factory_opens_only_fixed_metadata_and_retains_source_identity() {
    let directory = tempfile::tempdir().unwrap();
    let config = fixture_config(directory.path());
    host(&config, HOST);
    let root = config.load().unwrap().catalog_root;
    let store = FileRecordStore::new(&root, HOST).unwrap();
    store.initialize().unwrap();
    // Neither unrelated bad records nor old history can enter a point-read dependency set.
    fs::create_dir_all(root.join("hosts/unrelated/streams")).unwrap();
    fs::write(
        root.join("hosts/unrelated/streams/bad.json"),
        "not a record",
    )
    .unwrap();
    fs::create_dir_all(root.join(".catalog-sync")).unwrap();
    fs::write(
        root.join(".catalog-sync/authority.json"),
        "invalid old history",
    )
    .unwrap();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = events.clone();
    let opened = application::open_records_observed(&config, move |event| {
        observed.lock().unwrap().push(event)
    })
    .unwrap();
    assert_eq!(opened.root(), root);
    assert_eq!(opened.host().as_str(), HOST);
    let trace = events.lock().unwrap().clone();
    assert_eq!(
        record_io::bytes_read(&trace),
        fs::metadata(root.join(".catalog-local/records.json"))
            .unwrap()
            .len() as usize
    );
    assert!(record_io::writes(&trace).is_empty());
    assert_eq!(
        record_io::accesses(&trace),
        vec![
            RecordIoEvent::Lock(root.join(".catalog.lock")),
            RecordIoEvent::Read(root.join(".catalog-local/records.json")),
        ]
    );
    let source = opened.source_id().to_owned();
    let record =
        Record::Stream(Stream::new(HOST, "work", Title::new("Work", false), None).unwrap());
    let address = RecordAddress::stream(HOST, "work").unwrap();

    opened
        .records()
        .save::<_, StorageError>(&address.clone(), |_, _| {
            Ok(RecordChange {
                record,
                outcome: serde_json::Map::new(),
            })
        })
        .unwrap();
    let reopened = application::open_records(&config).unwrap();
    assert_eq!(reopened.source_id(), source);
    assert!(
        reopened
            .records()
            .with_reader::<_, StorageError>(|reader| reader.read(&address))
            .unwrap()
            .is_some()
    );
}

#[test]
fn record_factory_rejects_missing_legacy_incomplete_and_foreign_producer_roots_without_repair() {
    let directory = tempfile::tempdir().unwrap();
    let config = fixture_config(directory.path());
    let root = config.load().unwrap().catalog_root;
    assert!(application::open_records(&config).is_err());
    assert!(!root.exists());
    assert!(!config.host_path().exists());
    host(&config, HOST);
    assert!(application::open_records(&config).is_err());
    assert!(!root.exists());
    fs::create_dir_all(root.join("streams")).unwrap();
    fs::write(root.join("streams/old.yaml"), "old catalog").unwrap();
    assert!(application::open_records(&config).is_err());
    assert_eq!(
        fs::read_to_string(root.join("streams/old.yaml")).unwrap(),
        "old catalog"
    );
    assert!(!root.join(".catalog.lock").exists());
    assert!(!root.join(".catalog-local").exists());

    let second = tempfile::tempdir().unwrap();
    let second_config = fixture_config(second.path());
    host(&second_config, HOST);
    let second_root = second_config.load().unwrap().catalog_root;
    FileRecordStore::new(&second_root, OTHER)
        .unwrap()
        .initialize()
        .unwrap();
    assert!(application::open_records(&second_config).is_err());
    let marker = fs::read_to_string(second_root.join(".catalog-local/records.json")).unwrap();
    assert!(marker.contains(OTHER));
    assert!(!second_root.join(".catalog-sync").exists());
}

#[test]
fn fresh_initialization_creates_the_saved_producer_and_selected_transport_without_domain_records() {
    for strategy in [
        streams::config::Strategy::None,
        streams::config::Strategy::Git,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let config = fixture_config(directory.path());
        let opened = application::initialize_records(&config, strategy, None, "main").unwrap();
        assert_eq!(opened.host(), &config.host_identity().unwrap());
        let inventory = opened.inventory().unwrap();
        assert_eq!(inventory.len(), 1);
        assert_eq!(inventory[0].kind(), streams::model::RecordKind::Host);
        assert!(
            application::read_host_label(&opened, opened.host().as_str())
                .unwrap()
                .is_some()
        );
        let sync = streams::sync::open_backend(opened.root(), opened.host().as_str()).unwrap();
        assert_eq!(sync.strategy(), strategy);
        assert_eq!(
            opened.root().join(".git").exists(),
            strategy == streams::config::Strategy::Git
        );
        assert_eq!(
            application::open_records(&config).unwrap().source_id(),
            opened.source_id()
        );
        assert!(!opened.root().join(".catalog-sync").exists());
    }
}

#[test]
fn fresh_initialization_does_not_create_a_producer_for_an_existing_nonempty_root() {
    let directory = tempfile::tempdir().unwrap();
    let config = fixture_config(directory.path());
    let root = config.load().unwrap().catalog_root;
    fs::create_dir_all(&root).unwrap();
    let path = root.join("unrecognized-data");
    fs::write(&path, "leave intact").unwrap();
    assert!(
        application::initialize_records(&config, streams::config::Strategy::None, None, "main")
            .is_err()
    );
    assert!(!config.host_path().exists());
    assert!(!root.join(".catalog-local").exists());
    assert_eq!(fs::read_to_string(path).unwrap(), "leave intact");
}

#[test]
fn opened_records_keeps_the_settings_it_was_opened_with() {
    let directory = tempfile::tempdir().unwrap();
    let config = fixture_config(directory.path());
    config
        .set(streams::config::ConfigKey::StreamNaming, "manual")
        .unwrap();
    config
        .set(streams::config::ConfigKey::LifecycleDecayDays, "7")
        .unwrap();
    config
        .set(streams::config::ConfigKey::FrontendMode, "none")
        .unwrap();

    let initialized =
        application::initialize_records(&config, streams::config::Strategy::None, None, "main")
            .unwrap();
    let expected = config.load().unwrap();
    assert_eq!(initialized.settings(), &expected);
    assert_eq!(initialized.settings().catalog_root, initialized.root());

    let opened = application::open_records(&config).unwrap();
    assert_eq!(opened.settings(), &expected);
    assert!(opened.runtime_state().is_ok());
}
