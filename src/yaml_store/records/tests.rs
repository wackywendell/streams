use std::fs;
use std::os::unix::fs::symlink;

use super::*;
use crate::model::{Record, Session, Stream};
use serde_json::json;

thread_local! {
    static READS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    static LISTINGS: std::cell::RefCell<Vec<PathBuf>> = const { std::cell::RefCell::new(Vec::new()) };
    static WRITES: std::cell::RefCell<Vec<(PathBuf, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn setup() -> (tempfile::TempDir, YamlRecordStore) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = YamlRecordStore::new(&root, "host")
        .unwrap()
        .with_io_observer(move |event| match event {
            RecordIoEvent::Read(path) | RecordIoEvent::Lock(path) => READS.with(|reads| {
                reads
                    .borrow_mut()
                    .push(path.strip_prefix(&root).unwrap().to_path_buf())
            }),
            RecordIoEvent::List(path) => LISTINGS.with(|listings| listings.borrow_mut().push(path)),
            RecordIoEvent::BytesRead(_) => {}
            RecordIoEvent::Written(path, bytes) => {
                WRITES.with(|writes| writes.borrow_mut().push((path, bytes)))
            }
        });
    store.initialize().unwrap();
    (dir, store)
}

fn stream(id: &str, title: &str) -> Record {
    Record::Stream(Stream::new(id, title, None, "host").unwrap())
}

fn put(store: &YamlRecordStore, id: &str, record: Record) -> RecordSave {
    let request = RecordRequest::new(
        RecordAddress::for_record(&record),
        id,
        &record_to_data(&record),
    )
    .unwrap();
    store
        .save::<StorageError>(&request, |_, _| {
            Ok(RecordChange {
                record,
                outcome: json!({"accepted":id}).as_object().unwrap().clone(),
            })
        })
        .unwrap()
}

fn reads<T>(f: impl FnOnce() -> T) -> (T, Vec<PathBuf>) {
    READS.with(|reads| reads.borrow_mut().clear());
    LISTINGS.with(|listings| listings.borrow_mut().clear());
    let result = f();
    LISTINGS.with(|listings| {
        assert!(
            listings.borrow().is_empty(),
            "ordinary operation listed a directory"
        )
    });
    let paths = READS.with(|reads| reads.borrow().clone());
    (result, paths)
}

fn read(store: &YamlRecordStore, key: &RecordAddress) -> Result<Option<RecordRead>, StorageError> {
    store.with_reader(|reader| reader.read(key))
}

#[test]
fn replacement_observation_distinguishes_installed_bytes_from_definite_failure() {
    let (_directory, store) = setup();
    let path = store.root.join("target.yaml");
    // The filesystem owner separately proves that after_write means persist succeeded.
    for (result, installed) in [
        (Ok(()), true),
        (Err(StorageError::new("replacement refused")), false),
        (
            Err(StorageError::new("directory flush failed").after_write()),
            true,
        ),
    ] {
        WRITES.with(|writes| writes.borrow_mut().clear());
        store.observe_replacement(path.clone(), 123, &result);
        assert_eq!(
            WRITES.with(|writes| writes.borrow().clone()),
            if installed {
                vec![(path.clone(), 123)]
            } else {
                Vec::new()
            }
        );
    }
}

#[test]
fn canonical_paths_include_host_for_every_kind_and_encode_components() {
    for (kind, id, suffix) in [
        (RecordKind::Host, "host/..", "host.yaml"),
        (RecordKind::Stream, "work/..", "streams/work%2F...yaml"),
        (
            RecordKind::Session,
            "work/..",
            "conversations/work%2F...yaml",
        ),
    ] {
        let key = RecordAddress::new("host/..", kind, id).unwrap();
        assert_eq!(
            relative_record_path(&key).unwrap(),
            PathBuf::from("hosts/host%2F..").join(suffix)
        );
    }
    assert!(RecordAddress::new("host", RecordKind::Host, "other").is_err());
    assert!(RecordAddress::new("", RecordKind::Stream, "work").is_err());
}

#[test]
fn exact_reads_create_nothing_and_require_explicit_completed_format() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("absent");
    let store = YamlRecordStore::new(&root, "host").unwrap();
    let key = RecordAddress::for_record(&stream("work", "Work"));
    assert!(read(&store, &key).is_err());
    assert!(!root.exists());
    fs::create_dir(&root).unwrap();
    let lock = lock(&root.join(".catalog.lock")).unwrap();
    drop(lock);
    assert!(
        read(&store, &key)
            .unwrap_err()
            .to_string()
            .contains("cutover")
    );
    assert!(!root.join(".catalog-local").exists());
    fs::create_dir(root.join("streams")).unwrap();
    assert!(store.initialize().is_err());
    assert!(!root.join(FORMAT).exists());
}

#[test]
fn portable_sessions_require_native_ids_and_resolved_checkout_urls() {
    let (_dir, store) = setup();
    let session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "work",
        "pi",
        "native",
        "Work",
    )
    .unwrap();
    let mut legacy = session.clone();
    legacy.id = "legacy".into();
    let key = RecordAddress::for_record(&Record::Session(legacy.clone()));
    let request = RecordRequest::new(key.clone(), "bad", &json!({})).unwrap();
    assert!(
        store
            .save::<StorageError>(&request, |_, _| Ok(RecordChange {
                record: Record::Session(legacy),
                outcome: Map::new()
            }))
            .is_err()
    );
    assert!(
        !store
            .root
            .join(relative_record_path(&key).unwrap())
            .exists()
    );
    let record = Record::Session(session);
    put(&store, "valid", record.clone());
    let key = RecordAddress::for_record(&record);
    let path = store.root.join(relative_record_path(&key).unwrap());
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    for edit in [
        |v: &mut Value| v["record"]["harness_session_id"] = json!("different"),
        |v: &mut Value| v["record"]["checkout"] = json!({"repository":"label", "ref":"main"}),
    ] {
        let mut value = original.clone();
        edit(&mut value);
        let bytes = value.to_string();
        fs::write(&path, &bytes).unwrap();
        assert!(read(&store, &key).is_err());
        assert!(decode_record(&key, bytes.as_bytes()).is_err());
    }
}

#[test]
fn format_marker_requires_strict_json_and_the_configured_producer() {
    let (_dir, store) = setup();
    let key = RecordAddress::for_record(&stream("work", "Work"));
    for bytes in [
        "version: 2\nhost: host\n",
        r#"[2,"host"]"#,
        r#"{"version":2,"version":2,"host":"host"}"#,
        r#"{"version":2,"host":"host","unknown":true}"#,
        r#"{"version":1,"host":"host"}"#,
        r#"{"version":"2","host":"host"}"#,
        r#"{"version":2,"host":"other"}"#,
    ] {
        fs::write(store.root.join(FORMAT), bytes).unwrap();
        assert!(read(&store, &key).is_err(), "{bytes}");
    }
    fs::write(store.root.join(FORMAT), r#"{"version":2,"host":"host"}"#).unwrap();
    assert!(read(&store, &key).unwrap().is_none());
}

#[test]
fn named_reads_and_saves_do_not_scan_or_depend_on_unrelated_records_or_history() {
    let (_dir, store) = setup();
    let original = stream("work", "Original");
    let key = RecordAddress::for_record(&original);
    put(&store, "create", original.clone());
    let expected = vec![
        PathBuf::from(".catalog.lock"),
        PathBuf::from(FORMAT),
        relative_record_path(&key).unwrap(),
    ];
    let (before, first_reads) = reads(|| read(&store, &key).unwrap().unwrap());
    assert_eq!(first_reads, expected);
    for i in 0..100 {
        let path = store.root.join(format!("hosts/elsewhere/streams/{i}.yaml"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "malformed: [").unwrap();
    }
    fs::create_dir(store.root.join("history")).unwrap();
    fs::write(store.root.join("history/unreadable"), "not a Git history").unwrap();
    let (after, next_reads) = reads(|| read(&store, &key).unwrap().unwrap());
    assert_eq!(before, after);
    assert_eq!(next_reads, expected);
    let (saved, save_reads) = reads(|| put(&store, "edit", stream("work", "Edited")));
    assert_eq!(save_reads, expected);
    assert_eq!(saved.current.record, stream("work", "Edited"));
    assert!(!store.root.join(".catalog-sync").exists());
    assert!(!store.root.join(".git").exists());
    assert!(!store.root.join("save.json").exists());
}

#[test]
fn shell_reads_only_explicit_dependencies_and_save_returns_without_reopening() {
    let (_dir, store) = setup();
    let source = stream("source", "Source");
    let destination = stream("destination", "Destination");
    let session = Record::Session(
        Session::new(
            Session::id_for("host", "pi", "native"),
            "host",
            "source",
            "pi",
            "native",
            "Work",
        )
        .unwrap(),
    );
    for (id, record) in [
        ("source", source.clone()),
        ("destination", destination.clone()),
        ("session", session.clone()),
    ] {
        put(&store, id, record);
    }
    let old_bytes = fs::read(
        store
            .root
            .join(relative_record_path(&RecordAddress::for_record(&source)).unwrap()),
    )
    .unwrap();
    let request = RecordRequest::new(
        RecordAddress::for_record(&session),
        "move",
        &json!({"destination":"destination"}),
    )
    .unwrap();
    let (saved, paths) = reads(|| {
        store
            .save::<StorageError>(&request, |reader, before| {
                assert_eq!(
                    reader
                        .read(&RecordAddress::for_record(&source))?
                        .unwrap()
                        .record,
                    source
                );
                assert_eq!(
                    reader
                        .read(&RecordAddress::for_record(&destination))?
                        .unwrap()
                        .record,
                    destination
                );
                let Record::Session(mut moved) = before.unwrap().record.clone() else {
                    unreachable!()
                };
                moved.stream_id = "destination".into();
                Ok(RecordChange {
                    record: Record::Session(moved),
                    outcome: Map::new(),
                })
            })
            .unwrap()
    });
    assert_eq!(
        paths,
        vec![
            PathBuf::from(".catalog.lock"),
            PathBuf::from(FORMAT),
            relative_record_path(&RecordAddress::for_record(&session)).unwrap(),
            relative_record_path(&RecordAddress::for_record(&source)).unwrap(),
            relative_record_path(&RecordAddress::for_record(&destination)).unwrap()
        ]
    );
    assert_eq!(
        fs::read(
            store
                .root
                .join(relative_record_path(&RecordAddress::for_record(&source)).unwrap())
        )
        .unwrap(),
        old_bytes
    );
    let Record::Session(moved) = saved.current.record else {
        unreachable!()
    };
    assert_eq!(moved.stream_id, "destination");
}

#[test]
fn retry_after_reopening_and_later_edits_keeps_current_record_and_original_outcome() {
    let (_dir, store) = setup();
    let original = stream("work", "Original");
    let first = put(&store, "first", original.clone());
    let key = RecordAddress::for_record(&original);
    let request = RecordRequest::new(key.clone(), "first", &record_to_data(&original)).unwrap();
    let later = put(&store, "later", stream("work", "Later"));
    let bytes = fs::read(store.root.join(relative_record_path(&key).unwrap())).unwrap();
    let reopened = YamlRecordStore::new(&store.root, "host").unwrap();
    let result = reopened
        .save::<StorageError>(&request, |_, _| panic!("retry ran apply"))
        .unwrap();
    assert!(result.repeated);
    assert_eq!(result.current, later.current);
    assert_eq!(result.outcome, first.outcome);
    assert_eq!(
        fs::read(store.root.join(relative_record_path(&key).unwrap())).unwrap(),
        bytes
    );
    let changed = RecordRequest::new(key, "first", &json!({"different":true})).unwrap();
    assert!(
        reopened
            .save::<StorageError>(&changed, |_, _| panic!("changed intent ran apply"))
            .unwrap_err()
            .to_string()
            .contains("changed intent")
    );
}

#[test]
fn no_change_keeps_logical_revision_but_embeds_its_own_receipt() {
    let (_dir, store) = setup();
    let record = stream("work", "Same");
    let before = put(&store, "first", record.clone());
    let after = put(&store, "second", record.clone());
    assert_eq!(before.current, after.current);
    let bytes = fs::read(
        store
            .root
            .join(relative_record_path(&RecordAddress::for_record(&record)).unwrap()),
    )
    .unwrap();
    let envelope: Envelope = decode(&bytes).unwrap();
    assert_eq!(envelope.receipts.len(), 2);
}

#[test]
fn foreign_writes_and_target_replacement_fail_before_any_replacement() {
    let (_dir, store) = setup();
    let foreign = RecordAddress::new("other", RecordKind::Stream, "work").unwrap();
    let request = RecordRequest::new(foreign, "bad", &json!({})).unwrap();
    let (error, paths) =
        reads(|| store.save::<StorageError>(&request, |_, _| panic!("foreign apply")));
    assert!(error.is_err());
    assert!(paths.is_empty());
    let own = stream("work", "Own");
    let request = RecordRequest::new(RecordAddress::for_record(&own), "bad", &json!({})).unwrap();
    let error = store
        .save::<StorageError>(&request, |_, _| {
            Ok(RecordChange {
                record: stream("different", "Wrong"),
                outcome: Map::new(),
            })
        })
        .unwrap_err();
    assert!(error.to_string().contains("identity or host"));
    assert!(read(&store, request.target()).unwrap().is_none());
    let wrong = YamlRecordStore::new(&store.root, "other").unwrap();
    assert!(read(&wrong, request.target()).is_err());
}

#[test]
fn foreign_replicas_are_directly_readable_but_never_writable() {
    let (_dir, store) = setup();
    let other_dir = tempfile::tempdir().unwrap();
    let other = YamlRecordStore::new(other_dir.path(), "other").unwrap();
    other.initialize().unwrap();
    let record = Record::Stream(Stream::new("work", "Foreign", None, "other").unwrap());
    let saved = put(&other, "create", record.clone());
    let key = RecordAddress::for_record(&record);
    let relative = relative_record_path(&key).unwrap();
    let target = store.root.join(&relative);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::copy(other.root.join(&relative), &target).unwrap();
    let (current, paths) = reads(|| read(&store, &key).unwrap().unwrap());
    assert_eq!(current, saved.current);
    assert_eq!(
        paths,
        vec![
            PathBuf::from(".catalog.lock"),
            PathBuf::from(FORMAT),
            relative
        ]
    );
    let request = RecordRequest::new(key, "change", &json!({})).unwrap();
    assert!(
        store
            .save::<StorageError>(&request, |_, _| panic!("foreign replica apply"))
            .is_err()
    );
    assert_eq!(read(&store, request.target()).unwrap().unwrap(), current);
}

#[test]
fn failed_atomic_replacement_leaves_record_and_receipts_together() {
    let (_dir, store) = setup();
    let record = stream("work", "Before");
    put(&store, "first", record.clone());
    let path = store
        .root
        .join(relative_record_path(&RecordAddress::for_record(&record)).unwrap());
    let bytes = fs::read(&path).unwrap();
    fs::remove_dir(store.root.join(".catalog-staging")).unwrap();
    fs::write(store.root.join(".catalog-staging"), "blocked").unwrap();
    WRITES.with(|writes| writes.borrow_mut().clear());
    let after = stream("work", "After");
    let request = RecordRequest::new(
        RecordAddress::for_record(&after),
        "second",
        &record_to_data(&after),
    )
    .unwrap();
    assert!(
        store
            .save::<StorageError>(&request, |_, _| Ok(RecordChange {
                record: after,
                outcome: Map::new()
            }))
            .is_err()
    );
    assert!(WRITES.with(|writes| writes.borrow().is_empty()));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let envelope: Envelope = decode(&bytes).unwrap();
    assert!(!envelope.receipts.contains_key("second"));
    assert_eq!(
        read(&store, request.target()).unwrap().unwrap().record,
        record
    );
}

#[test]
fn malformed_duplicate_key_and_wrong_identity_records_are_never_absence() {
    let (_dir, store) = setup();
    let record = stream("work", "Before");
    put(&store, "first", record.clone());
    let key = RecordAddress::for_record(&record);
    let path = store.root.join(relative_record_path(&key).unwrap());
    let original: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut variants = vec!["[]".to_string(), "{\"version\":2,\"version\":2}".into()];
    for edit in [
        |v: &mut Value| v["version"] = json!(3),
        |v: &mut Value| v["revision"] = json!(""),
        |v: &mut Value| v["unknown"] = json!(true),
        |v: &mut Value| v["record"]["id"] = json!("other"),
        |v: &mut Value| v["record"]["owner"]["host_id"] = json!("other"),
        |v: &mut Value| v["receipts"]["first"]["fingerprint"] = json!("bad"),
        |v: &mut Value| v["receipts"]["first"] = json!(["a".repeat(64), {}]),
        |v: &mut Value| v["receipts"]["first"]["unknown"] = json!(true),
        |v: &mut Value| v["record"] = json!([]),
    ] {
        let mut value = original.clone();
        edit(&mut value);
        variants.push(value.to_string());
    }
    for bytes in variants {
        fs::write(&path, &bytes).unwrap();
        assert!(read(&store, &key).is_err(), "{bytes}");
        assert!(decode_record(&key, bytes.as_bytes()).is_err(), "{bytes}");
    }
}

#[test]
fn unreadable_records_are_named_by_kind_path_and_cause() {
    let (_dir, store) = setup();
    let record = stream("work", "Before");
    put(&store, "first", record.clone());
    let key = RecordAddress::for_record(&record);
    let path = store.root.join(relative_record_path(&key).unwrap());
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["record"]["archived"] = json!(false);
    fs::write(&path, value.to_string()).unwrap();
    let error = read(&store, &key).unwrap_err().to_string();
    assert!(
        error.starts_with("Stream record hosts/host/streams/work.yaml is unreadable: "),
        "{error}"
    );
    assert!(error.contains("unknown field `archived`"), "{error}");
    assert_eq!(
        decode_record(&key, value.to_string().as_bytes())
            .unwrap_err()
            .to_string(),
        error
    );
}

#[test]
fn symlinked_ancestors_leaves_and_special_files_are_rejected_without_blocking() {
    for ancestor in [
        "hosts",
        "hosts/host",
        "hosts/host/streams",
        "hosts/host/streams/work.yaml",
    ] {
        let (_dir, store) = setup();
        let key = RecordAddress::for_record(&stream("work", "Work"));
        let path = store.root.join(ancestor);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink("missing", &path).unwrap();
        assert!(read(&store, &key).is_err(), "{ancestor}");
    }
    let (_dir, store) = setup();
    let key = RecordAddress::for_record(&stream("work", "Work"));
    let path = store.root.join(relative_record_path(&key).unwrap());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let name = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(read(&store, &key).is_err());
}
