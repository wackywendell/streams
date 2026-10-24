use super::*;
use crate::{
    model::{Checkout, Document, Host, Repository, Stream},
    operations::Authority,
    storage::Snapshot,
};

pub(super) fn old() -> Inspection {
    let mut local = Stream::new("local", "Local", None, "host").unwrap();
    local.repositories.insert(
        "repo".into(),
        Repository::new("https://example.com/original.git").unwrap(),
    );
    let mut document =
        Document::new("repo", "main", "design.md", "Design", Some("commit".into())).unwrap();
    document.source_session_id = Some("old-id".into());
    local.documents.push(document);
    let foreign = Stream::new("foreign", "Foreign", None, "other").unwrap();
    let mut session = Session::new("old-id", "host", "local", "pi", "native", "Work").unwrap();
    session.previous_stream_ids = vec!["historical".into()];
    session.checkout = Some(Checkout {
        repository: "repo".into(),
        repository_url: None,
        r#ref: "feature".into(),
        directory_within_checkout: "src".into(),
    });
    let remote = Session::new("foreign-id", "other", "foreign", "pi", "remote", "Remote").unwrap();
    Inspection {
        snapshot: Snapshot {
            catalog: Catalog::new(
                vec![local, foreign],
                vec![session, remote],
                vec![
                    Host::new(
                        "host",
                        "Local host",
                        "2026-10-24T00:00:00Z".parse().unwrap(),
                    )
                    .unwrap(),
                    Host::new(
                        "other",
                        "Other host",
                        "2026-10-24T00:00:00Z".parse().unwrap(),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
            revisions: BTreeMap::new(),
        },
        authorities: BTreeMap::from([
            (
                "local".into(),
                Authority {
                    host_id: "host".into(),
                    token: "old-creation".into(),
                },
            ),
            (
                "foreign".into(),
                Authority {
                    host_id: "other".into(),
                    token: "remote-creation".into(),
                },
            ),
        ]),
    }
}

#[test]
fn cutover_migrates_only_this_producer_and_translates_usable_references() {
    let inspection = old();
    let prepared = prepare(&inspection, "host").unwrap();
    assert_eq!(prepared.dropped_foreign_records(), 3);
    let canonical = Session::id_for("host", "pi", "native");
    assert_eq!(
        prepared.session_id("host", "old-id"),
        Some(canonical.as_str())
    );
    assert_eq!(prepared.session_id("other", "foreign-id"), None);
    assert_eq!(prepared.session_id("other", "old-id"), None);
    assert!(matches!(&prepared.records()[2], Record::Host(host) if host.id == "host"));
    assert_eq!(prepared.records().len(), 3);
    let Record::Stream(stream) = &prepared.records()[0] else {
        unreachable!()
    };
    assert_eq!(stream.owner_host_id, "host");
    assert_eq!(
        stream.documents[0].source_session_id.as_deref(),
        Some(canonical.as_str())
    );
    let Record::Session(session) = &prepared.records()[1] else {
        unreachable!()
    };
    assert_eq!(session.id, canonical);
    assert_eq!(session.previous_stream_ids, ["historical"]);
    assert_eq!(
        session.checkout.as_ref().unwrap().repository_url.as_deref(),
        Some("https://example.com/original.git")
    );
    for record in prepared.records() {
        record.validate_portable().unwrap();
    }
    assert_eq!(inspection.snapshot.catalog.sessions()[0].id, "old-id");
}

#[test]
fn unknown_or_pending_local_ownership_requires_explicit_recreation() {
    let mut inspection = old();
    inspection.authorities.remove("local");
    assert!(
        prepare(&inspection, "host")
            .unwrap_err()
            .to_string()
            .contains("ambiguous ownership")
    );
    let mut inspection = old();
    inspection.authorities.get_mut("local").unwrap().host_id = "other".into();
    assert!(prepare(&inspection, "host").is_err());
    let mut inspection = old();
    inspection.authorities.get_mut("foreign").unwrap().host_id = "host".into();
    assert!(prepare(&inspection, "host").is_err());
}

#[test]
fn local_cross_host_participation_is_not_silently_dropped_or_transferred() {
    let mut inspection = old();
    let mut session = inspection
        .snapshot
        .catalog
        .session("host", "old-id")
        .unwrap()
        .clone();
    session.stream_id = "foreign".into();
    session.checkout = None;
    inspection.snapshot.catalog = Catalog::new(
        inspection.snapshot.catalog.streams().to_vec(),
        vec![session],
        vec![],
    )
    .unwrap();
    assert!(
        prepare(&inspection, "host")
            .unwrap_err()
            .to_string()
            .contains("another host's Stream")
    );
}

#[test]
fn a_host_with_only_foreign_replicas_starts_empty_instead_of_rewriting_them() {
    let prepared = prepare(&old(), "third").unwrap();
    assert!(prepared.records().is_empty());
    assert_eq!(prepared.dropped_foreign_records(), 6);
}

#[test]
fn stale_or_foreign_document_provenance_is_cleared() {
    for source in ["foreign-id", "unknown-id"] {
        let mut inspection = old();
        let old = &inspection.snapshot.catalog;
        let mut streams = old.streams().to_vec();
        streams
            .iter_mut()
            .find(|stream| stream.id == "local")
            .unwrap()
            .documents[0]
            .source_session_id = Some(source.into());
        inspection.snapshot.catalog =
            Catalog::new(streams, old.sessions().to_vec(), old.hosts().to_vec()).unwrap();
        let prepared = prepare(&inspection, "host").unwrap();
        let Record::Stream(stream) = &prepared.records()[0] else {
            unreachable!()
        };
        assert_eq!(stream.documents[0].source_session_id, None);
    }
}

#[test]
fn portable_checkout_keeps_its_url_even_when_the_old_alias_is_gone() {
    let mut inspection = old();
    let old = &inspection.snapshot.catalog;
    let mut sessions = old.sessions().to_vec();
    let checkout = sessions
        .iter_mut()
        .find(|session| session.host_id == "host")
        .unwrap()
        .checkout
        .as_mut()
        .unwrap();
    checkout.repository = "removed-alias".into();
    checkout.repository_url = Some("https://example.com/earlier.git".into());
    inspection.snapshot.catalog =
        Catalog::new(old.streams().to_vec(), sessions, old.hosts().to_vec()).unwrap();
    let prepared = prepare(&inspection, "host").unwrap();
    let Record::Session(session) = &prepared.records()[1] else {
        unreachable!()
    };
    assert_eq!(
        session.checkout.as_ref().unwrap().repository_url.as_deref(),
        Some("https://example.com/earlier.git")
    );
}
