use super::*;
use std::sync::{Arc, Mutex};

fn save_during_network(root: &Path, host: &str, goal: &str) -> Result<(), GitError> {
    let catalog = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join(".catalog.lock"))
        .unwrap();
    if catalog.try_lock().is_err() {
        return Err(GitError::new(
            "catalog lock held during network call",
            Some(1),
        ));
    }
    catalog.unlock().unwrap();
    let transport = fs::File::open(root.join(".catalog-local/transport.lock")).unwrap();
    assert!(
        transport.try_lock().is_err(),
        "sync must retain its own transport lock"
    );
    let records = FileRecordStore::new(root, host).unwrap();
    let mut current = stream(&records, host);
    current.goal = Some(goal.into());
    put(&records, Record::Stream(current));
    assert_eq!(stream(&records, host).goal.as_deref(), Some(goal));
    Ok(())
}

#[test]
fn network_calls_leave_catalog_writable_and_push_only_the_prepared_snapshot() {
    for phase in ["ls-remote", "fetch", "push"] {
        let directory = tempfile::tempdir().unwrap();
        let remote = directory.path().join("remote");
        bare(&remote);
        let seed = directory.path().join("seed");
        setup(&seed, "seed");
        assert_eq!(
            transport(&seed, "seed", Some(&remote)).sync().sharing,
            SharingStatus::Confirmed
        );
        let root = directory.path().join("catalog");
        let records = setup(&root, "host");
        let mut sync = transport(&root, "host", Some(&remote));
        let observed_root = root.clone();
        sync.git = sync.git.clone().with_hook(Arc::new(Mutex::new(
            move |_: &Git, args: &[&str], _: Option<&[u8]>, _: Option<&Path>| {
                if args[0] == phase
                    && let Err(error) = save_during_network(&observed_root, "host", phase)
                {
                    return Some(Err(error));
                }
                None
            },
        )));
        let result = sync.sync();
        assert_eq!(
            result.sharing,
            SharingStatus::Confirmed,
            "{phase}: {}",
            result.detail
        );
        assert_eq!(stream(&records, "host").goal.as_deref(), Some(phase));
        let address = RecordAddress::new("host", RecordKind::Stream, "work").unwrap();
        let path = crate::file_store::records::relative_record_path(&address).unwrap();
        let published =
            RecordSync::tree(&Git::new(&remote, 30.0), "refs/heads/records-v2").unwrap();
        let Record::Stream(published) =
            crate::file_store::records::decode_record(&address, &published[&path]).unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            published.goal.as_deref(),
            (phase != "push").then_some(phase)
        );
        if phase == "push" {
            sync.git = Git::new(&root, 30.0);
            assert_eq!(sync.sync().sharing, SharingStatus::Confirmed);
            let published =
                RecordSync::tree(&Git::new(&remote, 30.0), "refs/heads/records-v2").unwrap();
            let Record::Stream(published) =
                crate::file_store::records::decode_record(&address, &published[&path]).unwrap()
            else {
                unreachable!()
            };
            assert_eq!(published.goal.as_deref(), Some("push"));
        }
    }
}

#[test]
fn a_save_during_fetch_is_checkpointed_before_same_file_divergence_is_decided() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("remote");
    bare(&remote);
    let a = directory.path().join("a");
    let b = directory.path().join("b");
    let records = setup(&a, "owner");
    let mut sync = transport(&a, "owner", Some(&remote));
    assert_eq!(sync.sync().sharing, SharingStatus::Confirmed);
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
    let peer = RecordSync::open(&b, "owner").unwrap();
    for (key, value) in [
        ("user.name", "Streams"),
        ("user.email", "streams@localhost"),
        ("commit.gpgsign", "false"),
    ] {
        peer.git
            .run(&["config", key, value], None, None, &[0])
            .unwrap();
    }
    let peer_records = FileRecordStore::new(&b, "owner").unwrap();
    let mut incoming = stream(&peer_records, "owner");
    incoming.goal = Some("remote work".into());
    put(&peer_records, Record::Stream(incoming));
    assert_eq!(peer.sync().sharing, SharingStatus::Confirmed);
    sync.git = sync.git.clone().with_hook(Arc::new(Mutex::new(
        move |_: &Git, args: &[&str], _: Option<&[u8]>, _: Option<&Path>| {
            if args[0] == "fetch"
                && let Err(error) = save_during_network(&a, "owner", "local during fetch")
            {
                return Some(Err(error));
            }
            None
        },
    )));
    let result = sync.sync();
    assert_eq!(result.sharing, SharingStatus::Blocked, "{}", result.detail);
    assert_eq!(result.local, LocalStatus::Committed);
    assert!(
        result.detail.contains("same record file"),
        "{}",
        result.detail
    );
    assert_eq!(
        stream(&records, "owner").goal.as_deref(),
        Some("local during fetch")
    );
    assert_eq!(
        stream(&peer_records, "owner").goal.as_deref(),
        Some("remote work")
    );
}
