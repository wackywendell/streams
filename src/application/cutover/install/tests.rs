use super::*;
use crate::{
    model::{Record, RecordKind, Session},
    storage::records::RecordInventory,
};
use std::collections::BTreeMap;

fn old_root(path: &Path) {
    fs::create_dir_all(path.join(".catalog-sync")).unwrap();
    fs::create_dir_all(path.join(".catalog-local/native")).unwrap();
    fs::write(path.join(".catalog.lock"), "").unwrap();
    fs::write(path.join(".catalog-sync/operations"), "whole old history").unwrap();
    fs::write(
        path.join(".catalog-local/native/evidence"),
        "whole old native state",
    )
    .unwrap();
    fs::write(path.join("human-file"), "uncommitted human data").unwrap();
}
#[test]
fn installation_archives_the_whole_catalog_and_exposes_only_completed_producer_records() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("catalog");
    old_root(&root);
    let prepared = prepare(&super::super::tests::old(), "host").unwrap();
    let archive = directory.path().join("archive");
    let installed = install(
        &root,
        &archive,
        &InstallInputs {
            catalog: &prepared,
            runtime: None,
        },
        Strategy::None,
        None,
        "records-v2",
        Policy::default(),
    )
    .unwrap();
    assert_eq!(installed.migrated_records, 3);
    assert_eq!(installed.dropped_foreign_records, 3);
    for (name, content) in [
        (".catalog-sync/operations", "whole old history"),
        (".catalog-local/native/evidence", "whole old native state"),
        ("human-file", "uncommitted human data"),
    ] {
        assert_eq!(
            fs::read_to_string(installed.archived_catalog.join(name)).unwrap(),
            content
        );
        assert!(!root.join(name).exists());
    }
    let records = YamlRecordStore::new(&root, "host").unwrap();
    assert_eq!(records.inventory().unwrap().len(), 3);
    let address = RecordAddress::new(
        "host",
        RecordKind::Session,
        Session::id_for("host", "pi", "native"),
    )
    .unwrap();
    let Record::Session(session) = records
        .with_reader::<_, StorageError>(|reader| reader.read(&address))
        .unwrap()
        .unwrap()
        .record
    else {
        unreachable!()
    };
    assert_eq!(session.stream_id, "local");
    assert!(session.checkout.unwrap().repository_url.is_some());
    assert!(
        records
            .with_reader::<_, StorageError>(|reader| reader
                .read(&RecordAddress::new("host", RecordKind::Session, "old-id").unwrap()))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        RecordSync::open(&root, "host", Policy::default())
            .unwrap()
            .strategy(),
        Strategy::None
    );
    assert!(!root.join("hosts/other").exists());
}
#[test]
fn invalid_fresh_baseline_leaves_old_root_and_archive_destination_untouched() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("catalog");
    old_root(&root);
    let prepared = prepare(&super::super::tests::old(), "host").unwrap();
    let archive = directory.path().join("archive");
    assert!(
        install(
            &root,
            &archive,
            &InstallInputs {
                catalog: &prepared,
                runtime: None
            },
            Strategy::Git,
            None,
            "bad..branch",
            Policy::default()
        )
        .is_err()
    );
    assert_eq!(
        fs::read_to_string(root.join("human-file")).unwrap(),
        "uncommitted human data"
    );
    assert!(!archive.exists());
    assert!(!root.join(".catalog-local/records.json").exists());
    assert!(!root.join(".git").exists());
}
#[test]
fn existing_archive_or_archive_inside_old_root_is_never_replaced() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("catalog");
    old_root(&root);
    let prepared = prepare(&super::super::tests::old(), "host").unwrap();
    let archive = directory.path().join("archive");
    fs::create_dir(&archive).unwrap();
    for destination in [&archive, &root.join("archive")] {
        assert!(
            install(
                &root,
                destination,
                &InstallInputs {
                    catalog: &prepared,
                    runtime: None
                },
                Strategy::None,
                None,
                "records-v2",
                Policy::default()
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.join("human-file")).unwrap(),
            "uncommitted human data"
        );
    }
    assert_eq!(fs::read_dir(archive).unwrap().count(), 0);
}
#[test]
fn offline_command_retains_existing_producer_and_takes_native_writer_lock_without_creating_evidence()
 {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path();
    let config = Config::new(
        &BTreeMap::from([("HOME".into(), home.to_str().unwrap().into())]),
        home,
    )
    .unwrap();
    let previous =
        crate::application::initialize(&config, Strategy::None, None, "main", false).unwrap();
    let host = previous.host().clone();
    let native = NativeState::new(home.join("native")).unwrap();
    let result = run(
        &config,
        &native,
        &home.join("archive"),
        Strategy::Git,
        None,
        "records-v2",
    )
    .unwrap();
    assert!(result.archived_catalog.join(".catalog-sync").exists());
    let opened = crate::application::open_records(&config).unwrap();
    assert_eq!(opened.host(), &host);
    assert_eq!(opened.inventory().unwrap().len(), 0);
    let sync = RecordSync::open(opened.root(), host.as_str(), Policy::default()).unwrap();
    assert_eq!(
        sync.sync().sharing,
        crate::sync::outcomes::SharingStatus::LocalOnly
    );
    assert!(!opened.root().join(".catalog-local/native").exists());
    assert_eq!(
        fs::read_dir(native.directory())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>(),
        [".native.lock"]
    );
    let _released = native
        .lock(0.0, &std::sync::atomic::AtomicBool::new(false))
        .unwrap();
}

#[test]
fn offline_cutover_refuses_an_active_native_writer_before_preparing_or_archiving_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path();
    let config = Config::new(
        &BTreeMap::from([("HOME".into(), home.to_str().unwrap().into())]),
        home,
    )
    .unwrap();
    let previous =
        crate::application::initialize(&config, Strategy::None, None, "main", false).unwrap();
    let native = NativeState::new(home.join("native")).unwrap();
    let _held = native
        .lock(0.0, &std::sync::atomic::AtomicBool::new(false))
        .unwrap();
    let archive = home.join("archive");
    assert!(
        run(
            &config,
            &native,
            &archive,
            Strategy::None,
            None,
            "records-v2"
        )
        .is_err()
    );
    assert!(!archive.exists());
    assert!(previous.root().join(".catalog-sync").exists());
    assert!(!previous.root().join(".catalog-local/records.json").exists());
}

#[test]
fn runtime_choices_are_translated_before_new_catalog_admission_and_originals_are_archived() {
    use crate::runtime::state::{Association, LaunchReference, herdr_scope};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("catalog");
    old_root(&root);
    let prepared = prepare(&super::super::tests::old(), "host").unwrap();
    let state = RuntimeState::new(directory.path().join("runtime"), "host").unwrap();
    let bindings = DirectoryBindings::new(
        state.root(),
        CheckoutRegistry::new(directory.path().join("registry"), "host").unwrap(),
    );
    bindings
        .remember("local", directory.path(), Some("old-id"), false, None, None)
        .unwrap();
    bindings
        .remember("local", directory.path(), None, false, None, None)
        .unwrap();
    let scope = herdr_scope(Path::new("/fixture"), None);
    {
        let mut guard = state.stream_lock(Some(&scope), "local").unwrap();
        guard
            .write_association(
                &Association::new(
                    "local",
                    "workspace",
                    Some(LaunchReference::new("old-id", "exact command").unwrap()),
                    true,
                )
                .unwrap(),
            )
            .unwrap();
    }
    let streams = BTreeSet::from(["local".into()]);
    let inputs = InstallInputs {
        catalog: &prepared,
        runtime: Some((
            bindings
                .prepare_cutover(&prepared.session_ids, &streams)
                .unwrap(),
            state
                .prepare_cutover(&prepared.session_ids, &streams)
                .unwrap(),
        )),
    };
    let archive = directory.path().join("archive");
    install(
        &root,
        &archive,
        &inputs,
        Strategy::None,
        None,
        "records-v2",
        Policy::default(),
    )
    .unwrap();
    let id = Session::id_for("host", "pi", "native");
    assert!(bindings.read("local", Some(&id)).unwrap().is_some());
    assert!(bindings.read("local", None).unwrap().is_some());
    assert_eq!(
        state
            .association(&scope, "workspace")
            .unwrap()
            .unwrap()
            .launch()
            .unwrap()
            .session_id(),
        id
    );
    assert!(archive.join("runtime/session-directories").exists());
    assert!(archive.join("runtime/associations").exists());
    assert!(
        YamlRecordStore::new(&root, "host")
            .unwrap()
            .inventory()
            .is_ok()
    );
}
