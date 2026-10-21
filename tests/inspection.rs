use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

use serde_json::json;
use streams_temper::{
    application,
    config::Config,
    model::{Record, Stream},
    operations::{Actor, Operation},
    storage::{StorageError, Transaction},
    sync::{git::GitCatalog, local::LocalCatalog, policy::Policy},
    yaml_store::{YamlStore, relative_record_path},
};

use crate::support;

type Files = BTreeMap<PathBuf, (Option<Vec<u8>>, SystemTime, u32)>;
fn files(root: &Path) -> Files {
    fn visit(path: &Path, result: &mut Files) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_symlink() {
            Some(
                fs::read_link(path)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            )
        } else {
            metadata.is_file().then(|| fs::read(path).unwrap())
        };
        result.insert(
            path.to_owned(),
            (bytes, metadata.modified().unwrap(), metadata.mode()),
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(&entry.unwrap().path(), result);
            }
        }
    }
    let mut result = Files::new();
    visit(root, &mut result);
    result
}

fn create() -> Operation {
    Operation::new(
        "create",
        None,
        Record::Stream(Stream::new("work", "Work", None, "host").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}

#[test]
fn local_inspection_reads_consistent_authority_and_has_no_effects() {
    let temp = tempfile::tempdir().unwrap();
    let store = YamlStore::new(temp.path());
    let backend = LocalCatalog::new(store.clone(), "host").unwrap();
    let before = files(temp.path());
    assert!(backend.inspect().is_err());
    assert_eq!(files(temp.path()), before);
    assert!(backend.save(&create(), None).unwrap().accepted());
    let before = files(temp.path());
    let inspected = backend.inspect().unwrap();
    assert_eq!(
        inspected.snapshot.catalog.stream("work").unwrap().title,
        "Work"
    );
    assert_eq!(inspected.authorities["work"].host_id, "host");
    assert_eq!(inspected.authorities["work"].token, "create");
    assert_eq!(files(temp.path()), before);
    store
        .with_inspection::<_, StorageError>(|tx| {
            assert!(tx.save(create().after(), &tx.read()?).is_err());
            assert!(
                tx.journal()
                    .write(
                        "forbidden.json",
                        json!({"version":1}).as_object().unwrap().clone()
                    )
                    .is_err()
            );
            assert!(tx.journal().clear("local.json").is_err());
            Ok(())
        })
        .unwrap();
    assert_eq!(files(temp.path()), before);
}

#[test]
fn missing_stream_creation_evidence_blocks_inspection_without_repair() {
    let temp = tempfile::tempdir().unwrap();
    let store = YamlStore::new(temp.path());
    let backend = LocalCatalog::new(store.clone(), "host").unwrap();
    assert!(backend.save(&create(), None).unwrap().accepted());
    fs::remove_file(temp.path().join(relative_record_path(create().after()))).unwrap();
    assert_eq!(backend.authority("work").unwrap(), None);
    let before = files(temp.path());
    assert!(
        backend
            .inspect()
            .unwrap_err()
            .to_string()
            .contains("missing stream")
    );
    assert_eq!(files(temp.path()), before);
}

#[test]
fn inspection_does_not_create_missing_journal_or_repair_interrupted_saves() {
    let temp = tempfile::tempdir().unwrap();
    let store = YamlStore::new(temp.path());
    store
        .with_transaction::<_, StorageError>(|_| Ok(()))
        .unwrap();
    let backend = LocalCatalog::new(store.clone(), "host").unwrap();
    let before = files(temp.path());
    assert!(
        backend
            .inspect()
            .unwrap()
            .snapshot
            .catalog
            .records()
            .is_empty()
    );
    assert_eq!(files(temp.path()), before);
    store
        .with_transaction::<_, StorageError>(|tx| {
            tx.journal().write(
                "save.json",
                json!({"version":1,"interrupted":true})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
        })
        .unwrap();
    let before = files(temp.path());
    assert!(
        backend
            .inspect()
            .unwrap_err()
            .to_string()
            .contains("interrupted save")
    );
    assert_eq!(files(temp.path()), before);
}

#[test]
fn git_inspection_neither_remembers_history_nor_refreshes_index() {
    let temp = tempfile::tempdir().unwrap();
    let mut backend = GitCatalog::new(temp.path(), "host", Policy::default()).unwrap();
    support::git::install_hook(&mut backend, |_, _, _, _| None);
    let backend = backend.initialize(None, "main").unwrap();
    let operation = create();
    assert!(backend.save(&operation, None).unwrap().accepted());
    fs::remove_file(temp.path().join(".catalog-sync/receipts.json")).unwrap();
    let record = temp.path().join(relative_record_path(operation.after()));
    fs::File::options()
        .write(true)
        .open(record)
        .unwrap()
        .set_times(
            fs::FileTimes::new().set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
        )
        .unwrap();
    let before = files(temp.path());
    let inspected = backend.inspect().unwrap();
    assert!(inspected.snapshot.catalog.stream("work").is_some());
    assert_eq!(inspected.authorities["work"].token, "create");
    assert_eq!(files(temp.path()), before);
    assert!(!temp.path().join(".catalog-sync/receipts.json").exists());
}

#[test]
fn uninitialized_application_inspection_never_creates_identity_or_catalog() {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::new(
        &BTreeMap::from([("HOME".into(), temp.path().to_str().unwrap().into())]),
        temp.path(),
    )
    .unwrap();
    let before = files(temp.path());
    assert!(application::inspect(&config).is_err());
    assert!(!config.host_path().exists());
    assert_eq!(files(temp.path()), before);
}
