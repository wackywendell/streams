mod artifacts;
mod concurrency;
mod regressions;

use super::*;
use crate::{
    model::{Goal, Record, Session, Stream, Title},
    storage::records::{RecordAddress, RecordChange, RecordStore},
};
use serde_json::json;

fn put(store: &FileRecordStore, record: Record) {
    store
        .save::<_, StorageError>(&RecordAddress::for_record(&record), |_, _| {
            Ok(RecordChange {
                record,
                outcome: (),
            })
        })
        .unwrap();
}
fn setup(path: &Path, host: &str) -> FileRecordStore {
    let store = FileRecordStore::new(path, host).unwrap();
    store.initialize().unwrap();
    put(
        &store,
        Record::Stream(Stream::new(host, "work", Title::new("Work", false), None).unwrap()),
    );
    put(
        &store,
        Record::Session(
            Session::new(host, "pi", "native", "work", Title::new("Work", false)).unwrap(),
        ),
    );
    store
}
fn transport(path: &Path, host: &str, remote: Option<&Path>) -> RecordSync {
    RecordSync::initialize(
        path,
        host,
        Strategy::Git,
        remote.map(|path| path.to_str().unwrap().into()),
        "records-v2",
    )
    .unwrap()
}
fn bare(path: &Path) {
    fs::create_dir_all(path).unwrap();
    Git::new(path, 30.0)
        .run(&["init", "--quiet", "--bare"], None, None, &[0])
        .unwrap();
}
fn stream(store: &FileRecordStore, host: &str) -> Stream {
    let address = RecordAddress::stream(host, "work").unwrap();
    let read = store
        .with_reader::<_, StorageError>(|reader| reader.read(&address))
        .unwrap()
        .unwrap();
    let Record::Stream(stream) = read else {
        unreachable!()
    };
    stream
}

#[test]
fn local_only_transport_is_explicit_versioned_and_does_not_create_git_or_history() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    setup(root, "host");
    assert!(RecordSync::open(root, "host").is_err());
    let sync = RecordSync::initialize(root, "host", Strategy::None, None, "records-v2").unwrap();
    assert!(!root.join(".git").exists());
    assert!(!root.join(".catalog-sync").exists());
    assert_eq!(sync.sync().sharing, SharingStatus::LocalOnly);
    assert!(RecordSync::open(root, "other").is_err());
    assert!(RecordSync::initialize(root, "host", Strategy::Git, None, "records-v2").is_err());
    fs::write(
        root.join(CONFIG),
        br#"{"version":2,"mode":{"strategy":"none"},"unknown":true}"#,
    )
    .unwrap();
    assert!(RecordSync::open(root, "host").is_err());
}

#[test]
fn git_checkpoints_changed_records_but_never_opens_legacy_operation_history() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let records = setup(root, "host");
    let sync = transport(root, "host", None);
    let first = sync.git.head().unwrap();
    fs::create_dir_all(root.join(".catalog-sync")).unwrap();
    fs::write(
        root.join(".catalog-sync/operations"),
        "malformed obsolete history",
    )
    .unwrap();
    let mut changed = stream(&records, "host");
    changed.goal = Some(Goal::new("checkpointed", None, false));
    put(&records, Record::Stream(changed));
    let result = sync.sync();
    assert_eq!(result.sharing, SharingStatus::LocalOnly);
    assert_eq!(result.local, LocalStatus::Committed);
    assert_ne!(sync.git.head().unwrap(), first);
    assert_eq!(sync.sync().local, LocalStatus::Unchanged);
    assert!(result.domain.is_none());
    assert!(
        sync.git
            .files(&sync.git.head().unwrap())
            .unwrap()
            .keys()
            .all(|path| path.starts_with("hosts/"))
    );
}

#[test]
fn fresh_producer_baselines_join_disjoint_files_and_repopulate_foreign_replicas() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("remote");
    bare(&remote);
    let a = directory.path().join("a");
    let b = directory.path().join("b");
    let a_records = setup(&a, "a-host");
    let b_records = setup(&b, "b-host");
    let a_sync = transport(&a, "a-host", Some(&remote));
    let b_sync = transport(&b, "b-host", Some(&remote));
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(b_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(a_records.inventory().unwrap().len(), 4);
    assert_eq!(b_records.inventory().unwrap().len(), 4);
    assert_eq!(stream(&a_records, "b-host").host_id, "b-host");
    assert_eq!(stream(&b_records, "a-host").host_id, "a-host");
    assert_eq!(
        a_sync.git.files(&a_sync.git.head().unwrap()).unwrap(),
        b_sync.git.files(&b_sync.git.head().unwrap()).unwrap()
    );
}

#[test]
fn foreign_working_tree_edits_are_not_published_or_repaired() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("remote");
    bare(&remote);
    let a = directory.path().join("a");
    let b = directory.path().join("b");
    let a_records = setup(&a, "a-host");
    setup(&b, "b-host");
    let a_sync = transport(&a, "a-host", Some(&remote));
    let b_sync = transport(&b, "b-host", Some(&remote));
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(b_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    let head = a_sync.git.head().unwrap();
    let path = a.join(
        crate::file_store::records::relative_record_path(
            &RecordAddress::stream("b-host", "work").unwrap(),
        )
        .unwrap(),
    );
    let mut record: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    record["goal"] = json!({"text": "foreign edit", "short": null, "manual": false});
    fs::write(&path, serde_json::to_vec(&record).unwrap()).unwrap();
    let result = a_sync.sync();
    assert_eq!(result.sharing, SharingStatus::Blocked);
    assert!(result.detail.contains("another producer"));
    assert_eq!(a_sync.git.head().unwrap(), head);
    let mut own = stream(&a_records, "a-host");
    own.goal = Some(Goal::new("local remains writable", None, false));
    put(&a_records, Record::Stream(own));
    assert_eq!(
        stream(&a_records, "a-host")
            .goal
            .as_ref()
            .map(|g| g.text.as_str()),
        Some("local remains writable")
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap(),
        record
    );
}

#[test]
fn same_record_divergence_refuses_even_when_text_fields_do_not_overlap() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("remote");
    bare(&remote);
    let a = directory.path().join("a");
    let b = directory.path().join("b");
    let a_records = setup(&a, "owner");
    let a_sync = transport(&a, "owner", Some(&remote));
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    Git::new(directory.path(), 30.0)
        .run(
            &[
                "clone",
                "--quiet",
                "--branch",
                "records-v2",
                remote.to_str().unwrap(),
                b.to_str().unwrap(),
            ],
            None,
            None,
            &[0],
        )
        .unwrap();
    fs::create_dir_all(b.join(".catalog-local")).unwrap();
    for name in ["records.json", "transport.json"] {
        fs::copy(
            a.join(".catalog-local").join(name),
            b.join(".catalog-local").join(name),
        )
        .unwrap();
    }
    fs::write(b.join(".catalog.lock"), "").unwrap();
    let b_sync = RecordSync::open(&b, "owner").unwrap();
    for (key, value) in [
        ("user.name", "Streams"),
        ("user.email", "streams@localhost"),
        ("commit.gpgsign", "false"),
    ] {
        b_sync
            .git
            .run(&["config", key, value], None, None, &[0])
            .unwrap();
    }
    let b_records = FileRecordStore::new(&b, "owner").unwrap();
    let mut left = stream(&a_records, "owner");
    left.goal = Some(Goal::new("left", None, false));
    put(&a_records, Record::Stream(left));
    let mut right = stream(&b_records, "owner");
    right.title = Title::new("right", false);
    put(&b_records, Record::Stream(right));
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    let result = b_sync.sync();
    assert_eq!(result.sharing, SharingStatus::Blocked);
    assert!(result.detail.contains("same record file"));
    assert_eq!(stream(&b_records, "owner").title.text, "right");
    assert!(stream(&b_records, "owner").goal.is_none());
    assert!(!b.join(".git/MERGE_HEAD").exists());
}
