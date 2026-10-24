use super::*;
use crate::model::{Checkout, Status, TitleMode};

fn records() -> (Session, Stream, Stream, MoveIntent) {
    let mut session = Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "a",
        "pi",
        "native",
        "My work",
    )
    .unwrap();
    session.title_mode = TitleMode::Manual;
    session.status = Status::Blocked;
    session.state = "Conversation-local progress".into();
    session.state_short = Some("Progress".into());
    session.remaining = "More work".into();
    session.checkout = Some(Checkout {
        repository: "old-label".into(),
        repository_url: Some("https://example.com/original.git".into()),
        r#ref: "feature".into(),
        directory_within_checkout: "src".into(),
    });
    let a = Stream::new("a", "A", None, "host").unwrap();
    let b = Stream::new("b", "B", None, "host").unwrap();
    let intent = MoveIntent::new("host", "pi", "native", "b").unwrap();
    (session, a, b, intent)
}

#[test]
fn stable_identity_preserves_the_existing_ascii_json_preimage() {
    assert_eq!(
        Session::id_for("host", "pi", "native"),
        "ab441ac4-d600-5fd6-babf-fc43ab65623d"
    );
    assert_eq!(
        Session::id_for("høst", "pi", "native😀"),
        "3f549f4d-2ba8-5de0-bee3-ab0937b49dd1"
    );
    assert_ne!(
        Session::id_for("host", "pi", "native"),
        Session::id_for("other", "pi", "native")
    );
    assert_ne!(
        Session::id_for("host", "pi", "native"),
        Session::id_for("host", "codex", "native")
    );
}

#[test]
fn move_changes_only_membership_and_its_period() {
    let (session, a, b, intent) = records();
    let mut expected = session.clone();
    expected.stream_id = "b".into();
    expected.previous_stream_ids = vec!["a".into()];
    assert_eq!(
        apply_move(&session, &a, &b, &intent).unwrap(),
        Some(expected)
    );
    // Neither Stream knows the checkout label; the Session already owns the URL.
    assert!(a.repositories.is_empty());
    assert!(b.repositories.is_empty());
}

#[test]
fn unchanged_membership_creates_no_new_period_and_revisit_keeps_both_transitions() {
    let (session, a, b, to_b) = records();
    let to_a = MoveIntent::new("host", "pi", "native", "a").unwrap();
    assert_eq!(apply_move(&session, &a, &a, &to_a).unwrap(), None);
    let moved = apply_move(&session, &a, &b, &to_b).unwrap().unwrap();
    assert_eq!(apply_move(&moved, &b, &b, &to_b).unwrap(), None);
    let returned = apply_move(&moved, &b, &a, &to_a).unwrap().unwrap();
    assert_eq!(returned.stream_id, session.stream_id);
    assert_eq!(returned.previous_stream_ids, ["a", "b"]);
    assert_ne!(returned.previous_stream_ids, session.previous_stream_ids);
    assert_eq!(returned.id, session.id);
}

#[test]
fn wrong_native_identity_or_named_relationship_is_a_conflict() {
    let (session, a, b, intent) = records();
    for bad in [
        MoveIntent::new("host", "pi", "different", "b").unwrap(),
        MoveIntent::new("host", "codex", "native", "b").unwrap(),
        MoveIntent::new("host", "pi", "native", "a").unwrap(),
    ] {
        assert!(apply_move(&session, &a, &b, &bad).is_err());
    }
    assert!(apply_move(&session, &b, &a, &intent).is_err());
    let mut legacy = session.clone();
    legacy.id = "old ID".into();
    assert!(apply_move(&legacy, &a, &b, &intent).is_err());
}

#[test]
fn every_record_must_belong_to_the_configured_producer() {
    let (session, a, b, intent) = records();
    let foreign = MoveIntent::new("other", "pi", "native", "b").unwrap();
    assert!(apply_move(&session, &a, &b, &foreign).is_err());
    let mut foreign_source = a.clone();
    foreign_source.owner_host_id = "other".into();
    assert!(apply_move(&session, &foreign_source, &b, &intent).is_err());
    let mut foreign_destination = b.clone();
    foreign_destination.owner_host_id = "other".into();
    assert!(apply_move(&session, &a, &foreign_destination, &intent).is_err());
    let mut foreign_session = session.clone();
    foreign_session.host_id = "other".into();
    foreign_session.id = Session::id_for("other", "pi", "native");
    assert!(apply_move(&foreign_session, &a, &b, &intent).is_err());
}

#[test]
fn incomplete_portable_checkout_is_not_repaired_by_a_move() {
    let (mut session, a, b, intent) = records();
    session.checkout.as_mut().unwrap().repository_url = None;
    assert!(
        apply_move(&session, &a, &b, &intent)
            .unwrap_err()
            .to_string()
            .contains("portable repository URL")
    );
}

#[test]
fn intent_constructor_rejects_invalid_fields() {
    for fields in [
        ("", "pi", "native", "b"),
        ("host", "", "native", "b"),
        ("host", "pi", "\0", "b"),
        ("host", "pi", "native", " "),
    ] {
        assert!(MoveIntent::new(fields.0, fields.1, fields.2, fields.3).is_err());
    }
}
