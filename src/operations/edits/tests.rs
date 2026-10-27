use super::*;
use crate::model::{Link, Status};
use crate::operations::DomainOutcome;
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
fn each_progress_field_invalidates_only_its_own_short_form() {
    let mut before = session();
    before.recap = "Scope".into();
    before.state = "Now".into();
    before.remaining = "Next".into();
    before.recap_short = Some("Scope".into());
    before.state_short = Some("Now".into());
    before.remaining_short = Some("Next".into());
    for field in ["recap", "state", "remaining", "status"] {
        let value = if field == "status" {
            "waiting"
        } else {
            "Updated"
        };
        let after = apply_changes(
            &Record::Session(before.clone()),
            &changes(json!({field:value})),
            Actor::Automatic,
        )
        .unwrap();
        let Record::Session(after) = after else {
            unreachable!()
        };
        assert_eq!(after.recap_short.is_none(), field == "recap");
        assert_eq!(after.state_short.is_none(), field == "state");
        assert_eq!(after.remaining_short.is_none(), field == "remaining");
        if field != "status" {
            let short = format!("{field}_short");
            let after = apply_changes(
                &Record::Session(before.clone()),
                &changes(json!({field:value,short.clone():"Fresh"})),
                Actor::Automatic,
            )
            .unwrap();
            assert_eq!(crate::codec::record_to_data(&after)[short], "Fresh");
        }
    }
}

#[test]
fn progress_group_guards_cover_all_long_and_short_fields() {
    let before = session();
    for actor in [Actor::Automatic, Actor::User] {
        for field in [
            "recap",
            "recap_short",
            "state",
            "state_short",
            "remaining",
            "remaining_short",
            "status",
        ] {
            let edit = SessionEdit::new(
                "report",
                &before,
                &changes(json!({"recap":"Scope", "recap_short":"Scope", "remaining_short":"Next"})),
                actor,
                NamingMode::Automatic,
            )
            .unwrap();
            let value = if field == "status" {
                "waiting"
            } else {
                "Concurrent"
            };
            let Record::Session(current) = apply_changes(
                &Record::Session(before.clone()),
                &changes(json!({field:value})),
                Actor::User,
            )
            .unwrap() else {
                unreachable!()
            };
            let result = edit.apply(&current, &stream("a"), NamingMode::Automatic);
            assert_eq!(
                result.outcome(),
                if actor == Actor::Automatic {
                    DomainOutcome::Superseded
                } else {
                    DomainOutcome::Conflict
                },
                "{field}"
            );
        }
    }
}

#[test]
fn conversation_report_survives_move_and_return_without_restoring_old_membership() {
    let before = session();
    let edit = SessionEdit::new(
        "report",
        &before,
        &changes(json!({"state":"progress", "remaining":"next"})),
        Actor::Automatic,
        NamingMode::Automatic,
    )
    .unwrap();
    for (id, history) in [("b", vec!["a".into()]), ("a", vec!["a".into(), "b".into()])] {
        let mut current = before.clone();
        current.stream_id = id.into();
        current.previous_stream_ids = history;
        let Record::Session(after) =
            changed(edit.apply(&current, &stream(id), NamingMode::Automatic))
        else {
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
        NamingMode::Automatic,
    )
    .unwrap();
    assert!(matches!(
        edit.apply(&target, Some(&before), NamingMode::Automatic),
        Reconciliation::Changes { .. }
    ));
    for (id, history) in [("b", vec!["a".into()]), ("a", vec!["a".into(), "b".into()])] {
        let mut current = before.clone();
        current.stream_id = id.into();
        current.previous_stream_ids = history;
        assert!(matches!(
            edit.apply(&target, Some(&current), NamingMode::Automatic),
            Reconciliation::Superseded { .. }
        ));
    }
    assert!(matches!(
        edit.apply(&target, None, NamingMode::Automatic),
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
        NamingMode::Automatic,
    )
    .unwrap();
    let mut current = before.clone();
    current.remaining = "new instructions".into();
    assert!(matches!(
        edit.apply(&current, &stream("a"), NamingMode::Automatic),
        Reconciliation::Superseded { .. }
    ));
    let target = stream("a");
    let edit = StreamEdit::new(
        "stream",
        &target,
        &changes(json!({"goal":"old goal"})),
        Actor::Automatic,
        Some(before.clone()),
        NamingMode::Automatic,
    )
    .unwrap();
    assert!(matches!(
        edit.apply(&target, Some(&current), NamingMode::Automatic),
        Reconciliation::Superseded { .. }
    ));
    let mut current_target = target.clone();
    current_target.archived = true;
    assert!(matches!(
        edit.apply(&current_target, Some(&before), NamingMode::Automatic),
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
        NamingMode::Automatic,
    )
    .unwrap();
    let mut current = before.clone();
    current.title = "manual".into();
    current.title_mode = TitleMode::Manual;
    assert!(matches!(
        edit.apply(&current, &stream("a"), NamingMode::Automatic),
        Reconciliation::Superseded { .. }
    ));
    let edit = SessionEdit::new(
        "user",
        &before,
        &changes(json!({"state":"edited state"})),
        Actor::User,
        NamingMode::Automatic,
    )
    .unwrap();
    let mut current = before.clone();
    current.remaining = "concurrent remaining".into();
    assert!(matches!(
        edit.apply(&current, &stream("a"), NamingMode::Automatic),
        Reconciliation::Conflict { .. }
    ));
    let edit = SessionEdit::new(
        "rename",
        &before,
        &changes(json!({"title":"User name"})),
        Actor::User,
        NamingMode::Automatic,
    )
    .unwrap();
    let Record::Session(after) = changed(edit.apply(&current, &stream("a"), NamingMode::Automatic))
    else {
        unreachable!()
    };
    assert_eq!(after.remaining, current.remaining);
    assert_eq!(after.title_mode, TitleMode::Manual);
}

#[test]
fn effective_title_modes_control_generation_without_changing_saved_modes() {
    for mode in [TitleMode::Automatic, TitleMode::Manual, TitleMode::Inherit] {
        for default in [NamingMode::Automatic, NamingMode::Manual] {
            let mut before = session();
            before.title_mode = mode;
            let edit = SessionEdit::new(
                "report",
                &before,
                &changes(json!({"title":"Generated name","state":"progress"})),
                Actor::Automatic,
                default,
            )
            .unwrap();
            let Record::Session(after) = changed(edit.apply(&before, &stream("a"), default)) else {
                unreachable!()
            };
            assert_eq!(after.title_mode, mode);
            assert_eq!(
                after.title,
                if mode.resolve(default) == NamingMode::Automatic {
                    "Generated name"
                } else {
                    "Work"
                }
            );
            assert_eq!(after.state, "progress");

            let mut before = stream("a");
            before.title_mode = mode;
            let edit = StreamEdit::new(
                "report",
                &before,
                &changes(json!({"title":"Generated effort","goal":"Goal"})),
                Actor::Automatic,
                Some(session()),
                default,
            )
            .unwrap();
            let Record::Stream(after) = changed(edit.apply(&before, Some(&session()), default))
            else {
                unreachable!()
            };
            assert_eq!(after.title_mode, mode);
            assert_eq!(
                after.title,
                if mode.resolve(default) == NamingMode::Automatic {
                    "Generated effort"
                } else {
                    "a"
                }
            );
            assert_eq!(after.goal.as_deref(), Some("Goal"));
        }
    }
}

#[test]
fn user_mode_changes_do_not_generate_a_new_title_and_ignore_default_drift() {
    let before = session();
    for mode in ["automatic", "manual", "inherit"] {
        let edit = SessionEdit::new(
            "policy",
            &before,
            &changes(json!({"title_mode":mode})),
            Actor::User,
            NamingMode::Automatic,
        )
        .unwrap();
        let result = edit.apply(&before, &stream("a"), NamingMode::Manual);
        let after = match result {
            Reconciliation::NoChange { .. } => before.clone(),
            result => {
                let Record::Session(after) = changed(result) else {
                    unreachable!()
                };
                after
            }
        };
        assert_eq!(after.title, before.title);
        assert_eq!(after.title_mode.to_string(), mode);
    }
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
        NamingMode::Automatic,
    )
    .unwrap();
    let mut current = before.clone();
    current
        .issues
        .push(Link::new("https://example.com/concurrent", "current").unwrap());
    let Record::Stream(after) = changed(edit.apply(&current, None, NamingMode::Automatic)) else {
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
        assert!(
            SessionEdit::new(
                "op",
                &before,
                &changes(value),
                Actor::User,
                NamingMode::Automatic
            )
            .is_err()
        );
    }
    assert!(
        SessionEdit::new(
            "op",
            &before,
            &changes(json!({"title_mode":"manual"})),
            Actor::Automatic,
            NamingMode::Automatic
        )
        .is_err()
    );
    assert!(
        SessionEdit::new(
            "op",
            &before,
            &changes(json!({"state":"native"})),
            Actor::Native,
            NamingMode::Automatic
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
            None,
            NamingMode::Automatic
        )
        .is_err()
    );
    assert!(
        StreamEdit::new(
            "user",
            &target,
            &changes(json!({"goal":"manual"})),
            Actor::User,
            Some(before.clone()),
            NamingMode::Automatic
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
            Some(before),
            NamingMode::Automatic
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
        NamingMode::Automatic,
    )
    .unwrap();
    let mut target = stream("a");
    target.owner_host_id = "other".into();
    assert!(matches!(
        edit.apply(&before, &target, NamingMode::Automatic),
        Reconciliation::Conflict { .. }
    ));
    let Record::Session(after) = changed(edit.apply(&before, &stream("a"), NamingMode::Automatic))
    else {
        unreachable!()
    };
    assert_eq!(after.status, Status::Waiting);
    assert!(matches!(
        edit.apply(&after, &stream("a"), NamingMode::Automatic),
        Reconciliation::NoChange { .. }
    ));
}
