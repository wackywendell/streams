use super::*;
use crate::model::{Link, Status};
use serde_json::json;

fn changes(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}
fn stream(id: &str) -> Stream {
    Stream::new(id, id, None, "host").unwrap()
}
fn session() -> Session {
    Session::new(
        Session::id_for("host", "pi", "native"),
        "host",
        "a",
        "pi",
        "native",
        "Work",
    )
    .unwrap()
}
fn changed(result: Reconciliation) -> Record {
    let Reconciliation::Changes { record, .. } = result else {
        panic!("expected changes, got {result:?}")
    };
    *record
}

#[test]
fn conversation_report_survives_move_and_return_without_restoring_old_membership() {
    let before = session();
    let edit = SessionEdit::new(
        "report",
        &before,
        &changes(json!({"state":"progress", "remaining":"next"})),
        Actor::Automatic,
    )
    .unwrap();
    for (id, history) in [("b", vec!["a".into()]), ("a", vec!["a".into(), "b".into()])] {
        let mut current = before.clone();
        current.stream_id = id.into();
        current.previous_stream_ids = history;
        let Record::Session(after) = changed(edit.apply(&current, &stream(id))) else {
            unreachable!()
        };
        assert_eq!(after.stream_id, current.stream_id);
        assert_eq!(after.previous_stream_ids, current.previous_stream_ids);
        assert_eq!(after.id, before.id);
        assert_eq!(after.state, "progress");
        assert_eq!(after.remaining, "next");
    }
}

#[test]
fn effort_prose_cannot_cross_a_move_or_reenter_after_a_return() {
    let before = session();
    let target = stream("a");
    let edit = StreamEdit::new(
        "report",
        &target,
        &changes(json!({"goal":"old effort prose"})),
        Actor::Automatic,
        Some(before.clone()),
    )
    .unwrap();
    assert!(matches!(
        edit.apply(&target, Some(&before)),
        Reconciliation::Changes { .. }
    ));
    for (id, history) in [("b", vec!["a".into()]), ("a", vec!["a".into(), "b".into()])] {
        let mut current = before.clone();
        current.stream_id = id.into();
        current.previous_stream_ids = history;
        assert!(matches!(
            edit.apply(&target, Some(&current)),
            Reconciliation::Superseded { .. }
        ));
    }
    assert!(matches!(
        edit.apply(&target, None),
        Reconciliation::Superseded { .. }
    ));
}

#[test]
fn local_work_and_stream_input_changes_supersede_old_inference() {
    let before = session();
    let edit = SessionEdit::new(
        "session",
        &before,
        &changes(json!({"state":"old generated work"})),
        Actor::Automatic,
    )
    .unwrap();
    let mut current = before.clone();
    current.remaining = "new instructions".into();
    assert!(matches!(
        edit.apply(&current, &stream("a")),
        Reconciliation::Superseded { .. }
    ));
    let target = stream("a");
    let edit = StreamEdit::new(
        "stream",
        &target,
        &changes(json!({"goal":"old goal"})),
        Actor::Automatic,
        Some(before.clone()),
    )
    .unwrap();
    assert!(matches!(
        edit.apply(&target, Some(&current)),
        Reconciliation::Superseded { .. }
    ));
    let mut current_target = target.clone();
    current_target.archived = true;
    assert!(matches!(
        edit.apply(&current_target, Some(&before)),
        Reconciliation::Superseded { .. }
    ));
}

#[test]
fn manual_text_and_grouped_conversation_fields_remain_protected() {
    let before = session();
    let edit = SessionEdit::new(
        "automatic",
        &before,
        &changes(json!({"title":"generated"})),
        Actor::Automatic,
    )
    .unwrap();
    let mut current = before.clone();
    current.title = "manual".into();
    current.title_mode = NamingMode::Manual;
    assert!(matches!(
        edit.apply(&current, &stream("a")),
        Reconciliation::Superseded { .. }
    ));
    let edit = SessionEdit::new(
        "user",
        &before,
        &changes(json!({"state":"edited state"})),
        Actor::User,
    )
    .unwrap();
    let mut current = before.clone();
    current.remaining = "concurrent remaining".into();
    assert!(matches!(
        edit.apply(&current, &stream("a")),
        Reconciliation::Conflict { .. }
    ));
    let edit = SessionEdit::new(
        "rename",
        &before,
        &changes(json!({"title":"User name"})),
        Actor::User,
    )
    .unwrap();
    let Record::Session(after) = changed(edit.apply(&current, &stream("a"))) else {
        unreachable!()
    };
    assert_eq!(after.remaining, current.remaining);
    assert_eq!(after.title_mode, NamingMode::Manual);
}

#[test]
fn human_reference_additions_merge_without_replacing_concurrent_rows() {
    let before = stream("a");
    let edit = StreamEdit::new(
        "add",
        &before,
        &changes(json!({"issues":[{"url":"https://example.com/new", "description":"new"}]})),
        Actor::User,
        None,
    )
    .unwrap();
    let mut current = before.clone();
    current
        .issues
        .push(Link::new("https://example.com/concurrent", "current").unwrap());
    let Record::Stream(after) = changed(edit.apply(&current, None)) else {
        unreachable!()
    };
    assert_eq!(after.issues[0], current.issues[0]);
    assert_eq!(after.issues[1].url, "https://example.com/new");
}

#[test]
fn constructors_reject_membership_identity_lifecycle_and_unscoped_generated_effort_edits() {
    let before = session();
    for value in [
        json!({"id":"other"}),
        json!({"stream_id":"b"}),
        json!({"previous_stream_ids":["b"]}),
        json!({"harness":"claude"}),
    ] {
        assert!(SessionEdit::new("op", &before, &changes(value), Actor::User).is_err());
    }
    assert!(
        SessionEdit::new(
            "op",
            &before,
            &changes(json!({"title_mode":"manual"})),
            Actor::Automatic
        )
        .is_err()
    );
    assert!(
        SessionEdit::new(
            "op",
            &before,
            &changes(json!({"state":"native"})),
            Actor::Native
        )
        .is_err()
    );
    let target = stream("a");
    assert!(
        StreamEdit::new(
            "op",
            &target,
            &changes(json!({"goal":"generated"})),
            Actor::Automatic,
            None
        )
        .is_err()
    );
    assert!(
        StreamEdit::new(
            "user",
            &target,
            &changes(json!({"goal":"manual"})),
            Actor::User,
            Some(before.clone())
        )
        .is_err()
    );
    let mut foreign = target.clone();
    foreign.owner_host_id = "other".into();
    assert!(
        StreamEdit::new(
            "op",
            &foreign,
            &changes(json!({"goal":"generated"})),
            Actor::Automatic,
            Some(before)
        )
        .is_err()
    );
}

#[test]
fn foreign_or_mismatched_current_relationships_never_admit_a_local_edit() {
    let before = session();
    let edit = SessionEdit::new(
        "op",
        &before,
        &changes(json!({"status":"waiting"})),
        Actor::User,
    )
    .unwrap();
    let mut target = stream("a");
    target.owner_host_id = "other".into();
    assert!(matches!(
        edit.apply(&before, &target),
        Reconciliation::Conflict { .. }
    ));
    let Record::Session(after) = changed(edit.apply(&before, &stream("a"))) else {
        unreachable!()
    };
    assert_eq!(after.status, Status::Waiting);
    assert!(matches!(
        edit.apply(&after, &stream("a")),
        Reconciliation::NoChange { .. }
    ));
}
