use super::*;
use crate::model::{Record, Status};
use crate::operations::DomainOutcome;
use serde_json::{Value, json};

fn changes<P: serde::de::DeserializeOwned>(value: Value) -> P {
    serde_json::from_value(value).unwrap()
}
/// `before` as an edit by `actor` would leave it.
fn edited(before: &Session, value: Value, actor: Actor) -> Session {
    let patch: SessionPatch = changes(value);
    let mut after = before.clone();
    patch.normalized(before, actor).apply(&mut after);
    after
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
fn automatic_session_reports_allow_done_and_reject_closed() {
    let before = session();
    for allowed in [
        Status::Active,
        Status::Waiting,
        Status::Blocked,
        Status::Parked,
        Status::Done,
    ] {
        let edit = SessionEdit::new(
            &before,
            SessionPatch {
                status: Some(allowed),
                ..Default::default()
            },
            Actor::Automatic,
            Channel::Agent,
            NamingMode::Automatic,
        )
        .unwrap();
        let Record::Session(after) = (match edit.apply(&before, &stream("a"), NamingMode::Automatic)
        {
            Reconciliation::Changes { record } => *record,
            Reconciliation::NoChange { .. } => Record::Session(before.clone()),
            other => panic!("expected changes or no-change for {allowed}, got {other:?}"),
        }) else {
            unreachable!()
        };
        assert_eq!(after.status, allowed);
    }

    let err = SessionEdit::new(
        &before,
        SessionPatch {
            status: Some(Status::Closed),
            ..Default::default()
        },
        Actor::Automatic,
        Channel::Agent,
        NamingMode::Automatic,
    )
    .unwrap_err();
    assert!(err.to_string().contains("closed"), "{err}");

    let user_close = SessionEdit::new(
        &before,
        SessionPatch {
            status: Some(Status::Closed),
            ..Default::default()
        },
        Actor::User,
        Channel::Agent,
        NamingMode::Automatic,
    )
    .unwrap();
    let Record::Session(after) =
        changed(user_close.apply(&before, &stream("a"), NamingMode::Automatic))
    else {
        unreachable!()
    };
    assert_eq!(after.status, Status::Closed);
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
        let after = edited(&before, json!({field:value}), Actor::Automatic);
        assert_eq!(after.recap_short.is_none(), field == "recap");
        assert_eq!(after.state_short.is_none(), field == "state");
        assert_eq!(after.remaining_short.is_none(), field == "remaining");
        if field != "status" {
            let short = format!("{field}_short");
            let after = edited(
                &before,
                json!({field:value,short.clone():"Fresh"}),
                Actor::Automatic,
            );
            assert_eq!(serde_json::to_value(&after).unwrap()[short], "Fresh");
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
                &before,
                changes(json!({"recap":"Scope", "recap_short":"Scope", "remaining_short":"Next"})),
                actor,
                Channel::Agent,
                NamingMode::Automatic,
            )
            .unwrap();
            let value = if field == "status" {
                "waiting"
            } else {
                "Concurrent"
            };
            let mut current = edited(&before, json!({field:value}), Actor::User);
            current.revision += 1;
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
        &before,
        changes(json!({"state":"progress", "remaining":"next"})),
        Actor::Automatic,
        Channel::Agent,
        NamingMode::Automatic,
    )
    .unwrap();
    for (id, epoch) in [("b", 1), ("a", 2)] {
        let mut current = before.clone();
        current.stream_id = id.into();
        current.membership_epoch = epoch;
        let Record::Session(after) =
            changed(edit.apply(&current, &stream(id), NamingMode::Automatic))
        else {
            unreachable!()
        };
        assert_eq!(after.stream_id, current.stream_id);
        assert_eq!(after.membership_epoch, current.membership_epoch);
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
        &target,
        changes(json!({"goal":"old effort prose"})),
        Actor::Automatic,
        Channel::Agent,
        Some(before.clone()),
        NamingMode::Automatic,
    )
    .unwrap();
    assert!(matches!(
        edit.apply(&target, Some(&before), NamingMode::Automatic),
        Reconciliation::Changes { .. }
    ));
    for (id, epoch) in [("b", 1), ("a", 2)] {
        let mut current = before.clone();
        current.stream_id = id.into();
        current.membership_epoch = epoch;
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
        &before,
        changes(json!({"state":"old generated work"})),
        Actor::Automatic,
        Channel::Agent,
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
        &target,
        changes(json!({"goal":"old goal"})),
        Actor::Automatic,
        Channel::Agent,
        Some(before.clone()),
        NamingMode::Automatic,
    )
    .unwrap();
    // Conversation-local work edits and non-prose Stream metadata edits do not supersede Stream prose,
    // whereas a concurrent edit to Stream prose does.
    assert!(matches!(
        edit.apply(&target, Some(&current), NamingMode::Automatic),
        Reconciliation::Changes { .. }
    ));
    let mut renamed_target = target.clone();
    renamed_target.title_mode = TitleMode::Automatic;
    let Record::Stream(after) =
        changed(edit.apply(&renamed_target, Some(&before), NamingMode::Automatic))
    else {
        unreachable!()
    };
    assert_eq!(after.title_mode, TitleMode::Automatic);
    assert_eq!(after.goal.as_deref(), Some("old goal"));
    let mut prose_changed_target = target.clone();
    prose_changed_target.goal = Some("concurrent goal".into());
    assert!(matches!(
        edit.apply(&prose_changed_target, Some(&before), NamingMode::Automatic),
        Reconciliation::Superseded { .. }
    ));
}

#[test]
fn manual_text_and_grouped_conversation_fields_remain_protected() {
    let before = session();
    let edit = SessionEdit::new(
        &before,
        changes(json!({"title":"generated"})),
        Actor::Automatic,
        Channel::Agent,
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
        &before,
        changes(json!({"state":"edited state"})),
        Actor::User,
        Channel::Agent,
        NamingMode::Automatic,
    )
    .unwrap();
    let mut current = before.clone();
    current.remaining = "concurrent remaining".into();
    current.revision += 1;
    assert!(matches!(
        edit.apply(&current, &stream("a"), NamingMode::Automatic),
        Reconciliation::Conflict { .. }
    ));
    let edit = SessionEdit::new(
        &current,
        changes(json!({"title":"User name"})),
        Actor::User,
        Channel::Agent,
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
                &before,
                changes(json!({"title":"Generated name","state":"progress"})),
                Actor::Automatic,
                Channel::Agent,
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
                &before,
                changes(json!({"title":"Generated effort","goal":"Goal"})),
                Actor::Automatic,
                Channel::Agent,
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
            &before,
            changes(json!({"title_mode":mode})),
            Actor::User,
            Channel::Agent,
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
fn ordinary_edits_reject_reference_collections() {
    for field in [
        "references",
        "documents",
        "issues",
        "prs",
        "reference_changes",
    ] {
        assert!(
            serde_json::from_value::<StreamPatch>(json!({field:[]})).is_err(),
            "{field}"
        );
    }
}

#[test]
fn constructors_reject_membership_identity_lifecycle_and_unscoped_generated_effort_edits() {
    let before = session();
    for value in [
        json!({"id":"other"}),
        json!({"stream_id":"b"}),
        json!({"membership_epoch":2}),
        json!({"revision":2}),
        json!({"harness":"claude"}),
    ] {
        assert!(serde_json::from_value::<SessionPatch>(value).is_err());
    }
    assert!(
        SessionEdit::new(
            &before,
            changes(json!({"title_mode":"manual"})),
            Actor::Automatic,
            Channel::Agent,
            NamingMode::Automatic
        )
        .is_err()
    );
    assert!(
        SessionEdit::new(
            &before,
            changes(json!({"state":"native"})),
            Actor::Native,
            Channel::Agent,
            NamingMode::Automatic
        )
        .is_err()
    );
    let target = stream("a");
    assert!(
        StreamEdit::new(
            &target,
            changes(json!({"goal":"generated"})),
            Actor::Automatic,
            Channel::Agent,
            None,
            NamingMode::Automatic
        )
        .is_err()
    );
    assert!(
        StreamEdit::new(
            &target,
            changes(json!({"goal":"manual"})),
            Actor::User,
            Channel::Agent,
            Some(before.clone()),
            NamingMode::Automatic
        )
        .is_err()
    );
    let mut foreign = target.clone();
    foreign.owner_host_id = "other".into();
    assert!(
        StreamEdit::new(
            &foreign,
            changes(json!({"goal":"generated"})),
            Actor::Automatic,
            Channel::Agent,
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
        &before,
        changes(json!({"status":"waiting"})),
        Actor::User,
        Channel::Agent,
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
