mod artifacts;
mod concurrency;
mod regressions;

use super::*;
use crate::{
    model::{Record, RecordKind, Session, Stream},
    storage::records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
};
use serde_json::json;

fn put(store: &YamlRecordStore, id: &str, record: Record) {
    let request = RecordRequest::new(
        RecordAddress::for_record(&record),
        id,
        &json!({"record":record}),
    )
    .unwrap();
    store
        .save::<StorageError>(&request, |_, _| {
            Ok(RecordChange {
                record,
                outcome: Default::default(),
            })
        })
        .unwrap();
}
fn setup(path: &Path, host: &str) -> YamlRecordStore {
    let store = YamlRecordStore::new(path, host).unwrap();
    store.initialize().unwrap();
    put(
        &store,
        "stream",
        Record::Stream(Stream::new("work", "Work", None, host).unwrap()),
    );
    put(
        &store,
        "session",
        Record::Session(
            Session::new(
                Session::id_for(host, "pi", "native"),
                host,
                "work",
                "pi",
                "native",
                "Work",
            )
            .unwrap(),
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
fn stream(store: &YamlRecordStore, host: &str) -> Stream {
    let address = RecordAddress::new(host, RecordKind::Stream, "work").unwrap();
    let read = store
        .with_reader::<_, StorageError>(|reader| reader.read(&address))
        .unwrap()
        .unwrap();
    let Record::Stream(stream) = read.record else {
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
fn git_checkpoints_embedded_receipts_but_never_opens_legacy_operation_history() {
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
    put(
        &records,
        "receipt-only",
        Record::Stream(stream(&records, "host")),
    );
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
    assert_eq!(stream(&a_records, "b-host").owner_host_id, "b-host");
    assert_eq!(stream(&b_records, "a-host").owner_host_id, "a-host");
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
        crate::yaml_store::records::relative_record_path(
            &RecordAddress::new("b-host", RecordKind::Stream, "work").unwrap(),
        )
        .unwrap(),
    );
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    envelope["record"]["goal"] = json!("foreign edit");
    fs::write(&path, serde_json::to_vec(&envelope).unwrap()).unwrap();
    let result = a_sync.sync();
    assert_eq!(result.sharing, SharingStatus::Blocked);
    assert!(result.detail.contains("another producer"));
    assert_eq!(a_sync.git.head().unwrap(), head);
    let mut own = stream(&a_records, "a-host");
    own.goal = Some("local remains writable".into());
    put(&a_records, "own-write", Record::Stream(own));
    assert_eq!(
        stream(&a_records, "a-host").goal.as_deref(),
        Some("local remains writable")
    );
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap(),
        envelope
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
    let b_records = YamlRecordStore::new(&b, "owner").unwrap();
    let mut left = stream(&a_records, "owner");
    left.goal = Some("left".into());
    put(&a_records, "left", Record::Stream(left));
    let mut right = stream(&b_records, "owner");
    right.title = "right".into();
    put(&b_records, "right", Record::Stream(right));
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    let result = b_sync.sync();
    assert_eq!(result.sharing, SharingStatus::Blocked);
    assert!(result.detail.contains("same record file"));
    assert_eq!(stream(&b_records, "owner").title, "right");
    assert!(stream(&b_records, "owner").goal.is_none());
    assert!(!b.join(".git/MERGE_HEAD").exists());
}

#[test]
fn last_updated_ms_uses_mtime_for_local_and_cached_git_times_plus_dirty_mtime_for_git() {
    use crate::storage::records::RecordAddress;
    use std::time::{Duration, SystemTime};

    // Local-only backend uses file mtime.
    let local_dir = tempfile::tempdir().unwrap();
    let local_root = local_dir.path();
    let local_records = YamlRecordStore::new(local_root, "owner").unwrap();
    local_records.initialize().unwrap();
    let local_sync =
        RecordSync::initialize(local_root, "owner", Strategy::None, None, "main").unwrap();
    put(
        &local_records,
        "init",
        Record::Stream(Stream::new("work", "Work", None, "owner").unwrap()),
    );
    let work_addr = RecordAddress::new("owner", crate::model::RecordKind::Stream, "work").unwrap();
    let local_times = local_sync
        .last_updated_ms(std::slice::from_ref(&work_addr))
        .unwrap();
    assert!(local_times[&work_addr] > 0);

    // Git backend uses committed Git timestamp, caches it, and updates incrementally + for dirty files.
    let git_dir = tempfile::tempdir().unwrap();
    let git_root = git_dir.path();
    let git_records = YamlRecordStore::new(git_root, "owner").unwrap();
    git_records.initialize().unwrap();
    put(
        &git_records,
        "s1",
        Record::Stream(Stream::new("one", "One", None, "owner").unwrap()),
    );
    let git_sync = RecordSync::initialize(git_root, "owner", Strategy::Git, None, "main").unwrap();
    let one_addr = RecordAddress::new("owner", crate::model::RecordKind::Stream, "one").unwrap();
    let two_addr = RecordAddress::new("owner", crate::model::RecordKind::Stream, "two").unwrap();
    let first = git_sync
        .last_updated_ms(std::slice::from_ref(&one_addr))
        .unwrap();
    let committed_one = first[&one_addr];
    assert!(committed_one > 0);
    assert!(git_root.join(GIT_TIMES_CACHE).exists());

    // Cache hit returns the exact committed timestamp even if on-disk mtime is altered when clean?
    // Actually Git times are in seconds (* 1000), while dirty files get on-disk mtime.
    let second_cached = git_sync
        .last_updated_ms(std::slice::from_ref(&one_addr))
        .unwrap();
    assert_eq!(second_cached[&one_addr], committed_one);

    // Uncommitted dirty/untracked record uses local file mtime instead of missing/old Git time.
    let future_mtime = SystemTime::now() + Duration::from_secs(120);
    let mut two = Stream::new("two", "Two", None, "owner").unwrap();
    two.goal = Some("dirty".into());
    put(&git_records, "s2", Record::Stream(two));
    let two_file = git_root.join(relative_record_path(&two_addr).unwrap());
    fs::File::options()
        .write(true)
        .open(&two_file)
        .unwrap()
        .set_modified(future_mtime)
        .unwrap();
    let dirty_times = git_sync
        .last_updated_ms(&[one_addr.clone(), two_addr.clone()])
        .unwrap();
    assert_eq!(dirty_times[&one_addr], committed_one);
    assert!(dirty_times[&two_addr] > committed_one + 60_000);

    // Syncing commits "two" and incremental cache update captures its new HEAD timestamp.
    assert_eq!(git_sync.sync().sharing, SharingStatus::LocalOnly);
    let synced_times = git_sync
        .last_updated_ms(&[one_addr.clone(), two_addr.clone()])
        .unwrap();
    assert_eq!(synced_times[&one_addr], committed_one);
    assert!(synced_times[&two_addr] >= committed_one);
    let cache: GitTimesCache = decode_json(
        &read_bytes(&git_root.join(GIT_TIMES_CACHE))
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(cache.head, git_sync.local_head().unwrap());
    assert!(cache.times.contains_key("hosts/owner/streams/two.yaml"));
}
