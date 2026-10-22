use super::*;
use crate::model::Session;
use crate::sync::replay::{Replay, replay};
use crate::yaml_store::YamlStore;

const LEGACY: &str = "sessions/source/a/conversation.yaml";
const CANONICAL: &str = "conversations/a/conversation.yaml";

fn record_bytes(record: &Record) -> String {
    serde_json::to_string(&serde_json::json!({
        "version": 1, "revision": "fixture", "kind": record.kind(),
        "record": record_to_data(record),
    }))
    .unwrap()
}

fn seeded_legacy() -> (tempfile::TempDir, GitCatalog, Stream, Session) {
    let dir = tempfile::tempdir().unwrap();
    let mut catalog =
        GitCatalog::new(dir.path().join("publisher"), "a", Policy::default()).unwrap();
    fixture_git::install_hook(&mut catalog, |_, _, _, _| None);
    fixture_git::seed_bootstrap(&catalog);
    let source = Stream::new("source", "Source", None, "a").unwrap();
    let destination = Stream::new("destination", "Destination", None, "a").unwrap();
    let session = Session::new("conversation", "a", "source", "pi", "native", "Work").unwrap();
    let mut input = String::new();
    for (index, (id, record, authority)) in [
        ("source-create", Record::Stream(source.clone()), None),
        ("destination-create", Record::Stream(destination), None),
        (
            "session-create",
            Record::Session(session.clone()),
            Some("source-create".into()),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let operation = Operation::new(id, None, record, Actor::User, authority, vec![]).unwrap();
        let message = encode_operation(&operation, None).unwrap();
        let bytes = record_bytes(operation.after());
        let path = if matches!(operation.after(), Record::Session(_)) {
            LEGACY.into()
        } else {
            relative_record_path(operation.after())
        };
        let parent = if index == 0 {
            "from refs/heads/main^0\n"
        } else {
            ""
        };
        input.push_str(&format!(
            "commit refs/heads/main\ncommitter Streams <streams@example.invalid> 1700000000 +0000\ndata {}\n{}\n{}M 100644 inline {}\ndata {}\n{}\n\n",
            message.len(), message, parent, path.display(), bytes.len(), bytes,
        ));
    }
    input.push_str("done\n");
    catalog
        .git()
        .run(
            &["fast-import", "--quiet", "--done"],
            Some(input.as_bytes()),
            None,
            &[0],
        )
        .unwrap();
    catalog
        .git()
        .run(&["read-tree", "--reset", "-u", "HEAD"], None, None, &[0])
        .unwrap();
    (dir, catalog, source, session)
}

fn append(catalog: &GitCatalog, parent: &Oid, operation: &Operation, path: &str) -> Oid {
    let message = encode_operation(operation, None).unwrap();
    let bytes = record_bytes(operation.after());
    let input = format!(
        "commit refs/catalog/test-history\ncommitter Streams <streams@example.invalid> 1700000001 +0000\ndata {}\n{}\nfrom {}\nM 100644 inline {}\ndata {}\n{}\n\ndone\n",
        message.len(),
        message,
        parent,
        path,
        bytes.len(),
        bytes,
    );
    catalog
        .git()
        .run(
            &["fast-import", "--quiet", "--done"],
            Some(input.as_bytes()),
            None,
            &[0],
        )
        .unwrap();
    catalog
        .git()
        .text(
            &["rev-parse", "refs/catalog/test-history"],
            None,
            None,
            &[0],
        )
        .unwrap()
        .parse()
        .unwrap()
}

fn receiver(dir: &Path, publisher: &GitCatalog, tip: &Oid) -> GitCatalog {
    let remote = dir.join("remote.git");
    fs::create_dir(&remote).unwrap();
    fixture_git::test_command(
        &Git::new(&remote, 10.0),
        &["init", "--bare", "--initial-branch=main", "--template="],
        None,
        None,
    )
    .unwrap();
    publisher
        .git()
        .run(
            &["remote", "add", "origin", remote.to_str().unwrap()],
            None,
            None,
            &[0],
        )
        .unwrap();
    publisher.git().push_candidate(tip, "main").unwrap();
    let mut follower =
        GitCatalog::new(dir.join("follower"), "follower", Policy::default()).unwrap();
    fixture_git::install_hook(&mut follower, |_, _, _, _| None);
    follower
        .initialize(Some(remote.to_str().unwrap().into()), "main")
        .unwrap()
}

#[test]
fn legacy_membership_save_and_candidate_publication_keep_exact_history_and_live_checkout() {
    let (dir, catalog, source, session) = seeded_legacy();
    let parent: Oid = catalog.git().head().unwrap().parse().unwrap();
    let movement = Operation::reassociate(
        "native-move",
        &session,
        &source,
        "destination",
        "source-create".into(),
    )
    .unwrap();
    let origin = Origin::from([
        ("host".into(), "a".into()),
        ("session".into(), "original-session".into()),
    ]);
    assert!(catalog.save(&movement, Some(&origin)).unwrap().accepted());
    assert!(!catalog.root().join(LEGACY).exists());
    let bytes = fs::read(catalog.root().join(CANONICAL)).unwrap();
    let reopened = YamlStore::new(catalog.root())
        .with_inspection::<_, StorageError>(|tx| tx.read())
        .unwrap();
    assert_eq!(reopened.catalog.sessions().len(), 1);
    assert_eq!(
        reopened.catalog.get(&movement.after().key()).as_ref(),
        Some(movement.after())
    );

    let unrelated = Operation::new(
        "unrelated-create",
        None,
        Record::Stream(Stream::new("unrelated", "Unrelated", None, "b").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    let remote = append(&catalog, &parent, &unrelated, "streams/unrelated.yaml");
    let head = catalog.git().head().unwrap();
    let index = fs::read(catalog.root().join(".git/index")).unwrap();
    let local = read_history(catalog.git(), Some(&head)).unwrap();
    let remote = read_history(catalog.git(), Some(remote.as_str())).unwrap();
    assert!(remote.files().contains_key(LEGACY));
    assert!(!remote.files().contains_key(CANONICAL));
    let Replay::Ready { changes, .. } = replay(&local, &remote).unwrap() else {
        panic!("unrelated remote creation must permit the captured move")
    };
    assert_eq!(changes.len(), 1);
    let candidate: Oid = catalog
        .with_lock(|locked| locked.candidate(&local, &remote, changes))
        .unwrap()
        .parse()
        .unwrap();
    let follower = receiver(dir.path(), &catalog, &candidate);
    assert_eq!(follower.git().head().unwrap(), candidate.as_str());
    let history = read_history(follower.git(), Some(candidate.as_str())).unwrap();
    let entry = history.operation("native-move").unwrap();
    assert_eq!(entry.operation, movement);
    assert_eq!(entry.origin, origin);
    assert_eq!(history.catalog().sessions().len(), 1);
    assert_eq!(
        history.catalog().get(&movement.after().key()).as_ref(),
        Some(movement.after())
    );
    assert!(!history.files().contains_key(LEGACY));
    assert!(history.files().contains_key(CANONICAL));
    assert!(history.catalog().stream("unrelated").is_some());
    assert_eq!(catalog.git().head().unwrap(), head);
    assert_eq!(fs::read(catalog.root().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(catalog.root().join(CANONICAL)).unwrap(), bytes);
    assert!(!catalog.root().join(LEGACY).exists());
    assert!(!catalog.root().join("streams/unrelated.yaml").exists());
}

#[test]
fn published_duplicate_session_copy_is_rejected_before_checkout_adoption() {
    let (dir, catalog, source, session) = seeded_legacy();
    let parent: Oid = catalog.git().head().unwrap().parse().unwrap();
    let follower = receiver(dir.path(), &catalog, &parent);
    let index = fs::read(follower.root().join(".git/index")).unwrap();
    let bytes = fs::read(follower.root().join(LEGACY)).unwrap();
    let movement = Operation::reassociate(
        "native-move",
        &session,
        &source,
        "destination",
        "source-create".into(),
    )
    .unwrap();
    let duplicate = append(&catalog, &parent, &movement, CANONICAL);
    catalog.git().push_candidate(&duplicate, "main").unwrap();
    let result = follower.sync(SyncEvent::Explicit);
    assert!(result.requires_repair(), "{result:?}");
    let error = read_history(follower.git(), Some(duplicate.as_str())).unwrap_err();
    assert!(
        error.to_string().contains("unsupported catalog history"),
        "{error}"
    );
    assert_eq!(follower.git().head().unwrap(), parent.as_str());
    assert_eq!(fs::read(follower.root().join(".git/index")).unwrap(), index);
    assert_eq!(fs::read(follower.root().join(LEGACY)).unwrap(), bytes);
    assert!(!follower.root().join(CANONICAL).exists());
}
