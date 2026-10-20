use serde_json::{Map, json};
use streams_temper::model::NamingMode;
use streams_temper::model::RecordKind;
use streams_temper::operations::{Actor, DomainOutcome, Reconciliation};
use streams_temper::storage::StorageError;
use streams_temper::{
    codec::{record_from_data, record_to_data},
    model::{Catalog, Checkout, Document, Host, Link, Record, Repository, Session, Stream},
    operations::{Operation, reconcile, update},
    sync::{
        envelope::{decode_operation, encode_operation, load_json},
        local::LocalCatalog,
        outcomes::LocalStatus,
    },
    yaml_store::{YamlStore, encode_component},
};

fn creation(id: &str) -> Operation {
    Operation::new(
        id,
        None,
        Record::Stream(Stream::new("work", "Work", None, "host").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}

#[test]
fn record_and_operation_roundtrip() {
    let op = creation("create");
    let data = record_to_data(op.after());
    assert_eq!(
        &record_from_data(RecordKind::Stream, &data).unwrap(),
        op.after()
    );
    let encoded = encode_operation(&op, None).unwrap();
    assert_eq!(decode_operation(&encoded).unwrap().0, op);
    assert!(load_json(r#"{"a":1,"a":2}"#).is_err());
    assert!(record_from_data(RecordKind::Stream, &json!({"extra": true})).is_err());
}

#[test]
fn local_save_retry_and_changed_identity_are_fenced() {
    let dir = tempfile::tempdir().unwrap();
    let local = LocalCatalog::new(YamlStore::new(dir.path()), "host").unwrap();
    let op = creation("create");
    let saved = local.save(&op, None).unwrap();
    assert_eq!(saved.local, LocalStatus::Persisted);
    assert!(saved.accepted());
    let reopened = LocalCatalog::new(YamlStore::new(dir.path()), "host").unwrap();
    assert_eq!(
        reopened.save(&op, None).unwrap().domain.unwrap().outcome(),
        DomainOutcome::NoChange
    );
    assert_eq!(reopened.authority("work").unwrap().unwrap().token, "create");
    let different = Operation::new(
        "create",
        None,
        Record::Stream(Stream::new("work", "Other", None, "host").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(reopened.save(&different, None).unwrap().requires_repair());
    assert!(!dir.path().join(".git").exists());
    assert!(!dir.path().join(".catalog-sync/save.json").exists());
}

#[test]
fn user_manual_mode_protects_against_automatic_changes() {
    let create = creation("create");
    let catalog = Catalog::default()
        .replacing(create.after().clone())
        .unwrap();
    let changes: Map<_, _> = json!({"title":"Chosen"}).as_object().unwrap().clone();
    let op = update("edit", create.after(), &changes, Actor::User, None, vec![]).unwrap();
    let result = reconcile(&catalog, &op, None);
    let Reconciliation::Changes { record, .. } = result else {
        panic!()
    };
    let Record::Stream(s) = *record else { panic!() };
    assert_eq!(s.title_mode, NamingMode::Manual);
    let data = record_to_data(&Record::Stream(s));
    assert_eq!(data["title"], "Chosen");
}

#[test]
fn catalog_constructor_checks_identities_and_attachment_relationships() {
    let stream = Stream::new("work", "Work", None, "host").unwrap();
    let session = Session::new("session", "host", "work", "pi", "native", "Session").unwrap();
    let host = Host::new(
        "host",
        "Host",
        chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z").unwrap(),
    )
    .unwrap();
    let mut alias = session.clone();
    alias.id = "alias".into();
    let mut missing_stream = session.clone();
    missing_stream.stream_id = "missing".into();
    let mut checkout = session.clone();
    checkout.checkout = Some(Checkout::new("repo", "main", ".").unwrap());
    for (streams, sessions, hosts, expected) in [
        (
            vec![stream.clone(), stream.clone()],
            vec![],
            vec![],
            "duplicate streams record identity",
        ),
        (
            vec![stream.clone()],
            vec![session.clone(), session.clone()],
            vec![],
            "duplicate sessions record identity",
        ),
        (
            vec![stream.clone()],
            vec![session.clone(), alias],
            vec![],
            "duplicate native session identity",
        ),
        (
            vec![],
            vec![],
            vec![host.clone(), host.clone()],
            "duplicate hosts record identity",
        ),
        (
            vec![stream.clone()],
            vec![missing_stream],
            vec![],
            "unknown session stream: missing",
        ),
        (
            vec![stream.clone()],
            vec![checkout.clone()],
            vec![],
            "unknown checkout repository: repo",
        ),
    ] {
        assert_eq!(
            Catalog::new(streams, sessions, hosts)
                .unwrap_err()
                .to_string(),
            expected
        );
    }
    let catalog = Catalog::new(vec![stream.clone()], vec![session.clone()], vec![]).unwrap();
    assert_eq!(catalog.streams(), std::slice::from_ref(&stream));
    assert_eq!(catalog.sessions(), &[session]);
    assert!(catalog.hosts().is_empty());
    let mut with_repository = stream;
    with_repository
        .repositories
        .insert("repo".into(), Repository::new("url").unwrap());
    assert!(Catalog::new(vec![with_repository], vec![checkout], vec![host]).is_ok());
}

#[test]
fn catalog_order_is_canonical_and_noop_replacements_preserve_equality() {
    let streams = ["a", "z"].map(|id| Stream::new(id, id, None, "host").unwrap());
    let sessions = [
        Session::new("z", "a", "a", "pi", "first", "First").unwrap(),
        Session::new("a", "a", "z", "pi", "second", "Second").unwrap(),
        Session::new("a", "z", "a", "pi", "third", "Third").unwrap(),
    ];
    let checked_at = chrono::DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z").unwrap();
    let hosts = ["a", "z"].map(|id| Host::new(id, id, checked_at).unwrap());
    let catalog = Catalog::new(streams.to_vec(), sessions.to_vec(), hosts.to_vec()).unwrap();
    let mut reversed_streams = streams.to_vec();
    reversed_streams.reverse();
    let mut permuted_sessions = sessions.to_vec();
    permuted_sessions.rotate_left(1);
    let mut reversed_hosts = hosts.to_vec();
    reversed_hosts.reverse();
    let permuted = Catalog::new(reversed_streams, permuted_sessions, reversed_hosts).unwrap();
    assert_eq!(catalog, permuted);
    assert_eq!(
        catalog
            .sessions()
            .iter()
            .map(|s| (s.host_id.as_str(), s.id.as_str()))
            .collect::<Vec<_>>(),
        [("a", "a"), ("a", "z"), ("z", "a")]
    );
    for record in catalog.records() {
        assert_eq!(catalog.replacing(record).unwrap(), catalog);
    }
}

#[test]
fn catalog_canonicalization_preserves_semantic_reference_order() {
    let mut stream = Stream::new("work", "Work", None, "host").unwrap();
    stream
        .repositories
        .insert("repo".into(), Repository::new("url").unwrap());
    stream.documents = ["z.md", "a.md"]
        .map(|path| Document::new("repo", "main", path, "", None).unwrap())
        .to_vec();
    stream.issues = ["issue-z", "issue-a"]
        .map(|url| Link::new(url, "").unwrap())
        .to_vec();
    stream.prs = ["pr-z", "pr-a"]
        .map(|url| Link::new(url, "").unwrap())
        .to_vec();
    let catalog = Catalog::new(vec![stream.clone()], vec![], vec![]).unwrap();
    assert_eq!(catalog.stream("work"), Some(&stream));
    for field in ["documents", "issues", "prs"] {
        let mut reordered = stream.clone();
        match field {
            "documents" => reordered.documents.reverse(),
            "issues" => reordered.issues.reverse(),
            "prs" => reordered.prs.reverse(),
            _ => unreachable!(),
        }
        assert_ne!(
            catalog.replacing(Record::Stream(reordered)).unwrap(),
            catalog,
            "{field}"
        );
    }
}

#[test]
fn snapshot_compare_and_swap_and_path_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let store = YamlStore::new(dir.path());
    store
        .with_transaction::<_, StorageError>(|tx| {
            let empty = tx.read()?;
            let rec = creation("create").after().clone();
            tx.save(&rec, &empty)?;
            assert!(tx.save(&rec, &empty).unwrap_err().is_revision_conflict());
            Ok(())
        })
        .unwrap();
    assert_eq!(encode_component("../A/é"), "..%2F%41%2F%C3%A9");
    assert_eq!(encode_component(".."), "%2E%2E");
}
