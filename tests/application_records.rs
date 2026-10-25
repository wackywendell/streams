use crate::support::record_io;
use std::{
    collections::BTreeMap,
    fs,
    sync::{Arc, Mutex},
};

use serde_json::json;
use streams_temper::{
    application,
    config::Config,
    model::{Record, RecordKind, Stream},
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
    },
    yaml_store::records::{RecordIoEvent, YamlRecordStore},
};

const HOST: &str = "01234567-89ab-4def-8123-456789abcdef";
const OTHER: &str = "11234567-89ab-4def-8123-456789abcdef";

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
    let store = YamlRecordStore::new(&root, HOST).unwrap();
    store.initialize().unwrap();
    // Neither unrelated bad records nor old history can enter a point-read dependency set.
    fs::create_dir_all(root.join("hosts/unrelated/streams")).unwrap();
    fs::write(
        root.join("hosts/unrelated/streams/bad.yaml"),
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
    let record = Record::Stream(Stream::new("work", "Work", None, HOST).unwrap());
    let address = RecordAddress::new(HOST, RecordKind::Stream, "work").unwrap();
    let request =
        RecordRequest::new(address.clone(), "create", &json!({"record": record})).unwrap();
    opened
        .records()
        .save::<StorageError>(&request, |_, _| {
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
    YamlRecordStore::new(&second_root, OTHER)
        .unwrap()
        .initialize()
        .unwrap();
    assert!(application::open_records(&second_config).is_err());
    let marker = fs::read_to_string(second_root.join(".catalog-local/records.json")).unwrap();
    assert!(marker.contains(OTHER));
    assert!(!second_root.join(".catalog-sync").exists());
}
