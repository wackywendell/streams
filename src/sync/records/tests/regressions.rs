use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn a_fresh_empty_catalog_can_import_before_its_first_record_save() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("remote");
    bare(&remote);
    let a = directory.path().join("a");
    let b = directory.path().join("b");
    let a_records = setup(&a, "a-host");
    assert_eq!(
        transport(&a, "a-host", Some(&remote)).sync().sharing,
        SharingStatus::Confirmed
    );
    let b_records = YamlRecordStore::new(&b, "b-host").unwrap();
    b_records.initialize().unwrap();
    assert!(!b.join(".catalog-staging").exists());
    let result = transport(&b, "b-host", Some(&remote)).sync();
    assert_eq!(result.sharing, SharingStatus::Confirmed, "{result:?}");
    assert_eq!(stream(&b_records, "a-host"), stream(&a_records, "a-host"));
    assert_eq!(
        b_records.inventory().unwrap(),
        a_records.inventory().unwrap()
    );
}

#[test]
fn pull_only_and_noop_sync_never_push_and_noop_does_not_fetch() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("remote");
    bare(&remote);
    let a = directory.path().join("a");
    let b = directory.path().join("b");
    setup(&a, "a-host");
    let b_records = setup(&b, "b-host");
    let mut a_sync = transport(&a, "a-host", Some(&remote));
    let b_sync = transport(&b, "b-host", Some(&remote));
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(b_sync.sync().sharing, SharingStatus::Confirmed);
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let observed = calls.clone();
    a_sync.git = a_sync.git.clone().with_hook(Arc::new(Mutex::new(
        move |_: &Git, args: &[&str], _: Option<&[u8]>, _: Option<&Path>| {
            observed.lock().unwrap().push(args[0].to_owned());
            if args[0] == "push" {
                return Some(Err(GitError::new(
                    "must not push an already published tip",
                    Some(1),
                )));
            }
            None
        },
    )));
    let result = a_sync.sync();
    assert_eq!(result.sharing, SharingStatus::Confirmed);
    assert_eq!(result.local, LocalStatus::Committed);
    assert_eq!(
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|call| *call == "fetch")
            .count(),
        1
    );
    assert!(!calls.lock().unwrap().iter().any(|call| call == "push"));
    calls.lock().unwrap().clear();
    let result = a_sync.sync();
    assert_eq!(result.sharing, SharingStatus::Confirmed);
    assert_eq!(result.local, LocalStatus::Unchanged);
    assert!(
        !calls
            .lock()
            .unwrap()
            .iter()
            .any(|call| call == "fetch" || call == "push" || call == "merge-base")
    );
    // Subsequent foreign changes are still fetched and installed without publication.
    let mut changed = stream(&b_records, "b-host");
    changed.goal = Some("new remote work".into());
    put(&b_records, "remote-change", Record::Stream(changed));
    assert_eq!(b_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    assert!(!calls.lock().unwrap().iter().any(|call| call == "push"));
}

#[test]
fn git_ignore_rules_cannot_hide_records_or_unexpected_physical_entries() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("catalog");
    let records = setup(&root, "host");
    let sync = transport(&root, "host", None);
    assert_eq!(
        sync.git
            .text(&["config", "--get", "core.excludesFile"], None, None, &[0])
            .unwrap(),
        "/dev/null"
    );
    let ignore = directory.path().join("ignore");
    fs::write(&ignore, "hosts/**\n").unwrap();
    sync.git
        .run(
            &[
                "config",
                "--local",
                "core.excludesFile",
                ignore.to_str().unwrap(),
            ],
            None,
            None,
            &[0],
        )
        .unwrap();
    put(
        &records,
        "ignored-record",
        Record::Stream(Stream::new("added", "Added", None, "host").unwrap()),
    );
    assert_eq!(sync.sync().sharing, SharingStatus::LocalOnly);
    assert!(
        sync.git
            .files(&sync.git.head().unwrap())
            .unwrap()
            .contains_key("hosts/host/streams/added.yaml")
    );
    let head = sync.git.head().unwrap();
    let unexpected = root.join("hosts/host/unexpected");
    fs::create_dir(&unexpected).unwrap();
    assert!(records.inventory().is_err());
    assert_eq!(sync.sync().sharing, SharingStatus::Blocked);
    assert_eq!(sync.git.head().unwrap(), head);
    fs::remove_dir(&unexpected).unwrap();
    fs::write(
        root.join("hosts/host/streams/ignored.txt"),
        "invalid ignored file",
    )
    .unwrap();
    assert!(records.inventory().is_err());
    assert_eq!(sync.sync().sharing, SharingStatus::Blocked);
    assert_eq!(sync.git.head().unwrap(), head);
}

#[test]
fn remote_deletion_combines_with_an_independent_local_change() {
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
    let relative = crate::yaml_store::records::relative_record_path(
        &RecordAddress::new(
            "b-host",
            RecordKind::Session,
            Session::id_for("b-host", "pi", "native"),
        )
        .unwrap(),
    )
    .unwrap();
    fs::remove_file(b.join(&relative)).unwrap();
    let mut own = stream(&a_records, "a-host");
    own.goal = Some("independent local edit".into());
    put(&a_records, "local-change", Record::Stream(own));
    assert_eq!(b_sync.sync().sharing, SharingStatus::Confirmed);
    let result = a_sync.sync();
    assert_eq!(
        result.sharing,
        SharingStatus::Confirmed,
        "{}",
        result.detail
    );
    assert!(!a.join(&relative).exists());
    assert_eq!(
        stream(&a_records, "a-host").goal.as_deref(),
        Some("independent local edit")
    );
    assert!(
        !a_sync
            .git
            .files(&a_sync.git.head().unwrap())
            .unwrap()
            .contains_key(relative.to_str().unwrap())
    );
    assert_eq!(a_records.inventory().unwrap().len(), 3);
}

#[test]
fn incoming_broken_membership_is_rejected_before_install_or_ref_advance() {
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
    let relative = crate::yaml_store::records::relative_record_path(
        &RecordAddress::new(
            "b-host",
            RecordKind::Session,
            Session::id_for("b-host", "pi", "native"),
        )
        .unwrap(),
    )
    .unwrap();
    let original = fs::read(a.join(&relative)).unwrap();
    let head = a_sync.git.head().unwrap();
    let mut envelope: serde_json::Value = serde_json::from_slice(&original).unwrap();
    envelope["record"]["stream_id"] = json!("missing");
    fs::write(b.join(&relative), serde_json::to_vec(&envelope).unwrap()).unwrap();
    // Simulate an invalid remote producer; do not pass through the validating save/sync API.
    for args in [
        vec!["add", "-A", "--", "hosts"],
        vec!["commit", "--quiet", "-m", "Invalid incoming membership"],
        vec!["push", "--quiet", "origin", "HEAD:refs/heads/records-v2"],
    ] {
        b_sync.git.run(&args, None, None, &[0]).unwrap();
    }
    let result = a_sync.sync();
    assert_eq!(result.sharing, SharingStatus::Blocked);
    assert_eq!(result.local, LocalStatus::Unchanged);
    assert!(
        result.detail.contains("same-host Stream is missing"),
        "{}",
        result.detail
    );
    assert_eq!(a_sync.git.head().unwrap(), head);
    assert_eq!(fs::read(a.join(&relative)).unwrap(), original);
    assert_eq!(a_records.inventory().unwrap().len(), 4);
}

#[test]
fn install_refuses_symlinked_ancestors_and_leaf_for_both_replacement_and_deletion() {
    for replaced in [
        "hosts",
        "hosts/host",
        "hosts/host/streams",
        "hosts/host/streams/work.yaml",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("catalog");
        setup(&root, "host");
        let sync = transport(&root, "host", None);
        let relative = PathBuf::from("hosts/host/streams/work.yaml");
        let old = fs::read(root.join(&relative)).unwrap();
        let mut new = old.clone();
        new.push(b'\n');
        let previous = Tree::from([(relative.clone(), old.clone())]);
        let next = Tree::from([(relative.clone(), new)]);
        let outside = directory.path().join("outside");
        fs::rename(root.join(replaced), &outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join(replaced)).unwrap();
        assert!(sync.install(&previous, &next).is_err(), "{replaced}");
        assert!(sync.install(&previous, &Tree::new()).is_err(), "{replaced}");
        let suffix = relative.strip_prefix(replaced).unwrap();
        let retained = if suffix.as_os_str().is_empty() {
            outside
        } else {
            outside.join(suffix)
        };
        assert_eq!(fs::read(retained).unwrap(), old);
    }
}
