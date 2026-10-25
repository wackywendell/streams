use super::*;

fn session(host: &str, stream: &str, native: &str, status: Status) -> RecordRead {
    let mut session = Session::new(
        Session::id_for(host, "pi", native),
        host,
        stream,
        "pi",
        native,
        "Lane",
    )
    .unwrap();
    session.status = status;
    RecordRead {
        record: Record::Session(session),
        revision: "observed".into(),
    }
}

#[test]
fn reopen_selects_current_active_local_members_not_native_or_saved_layout_state() {
    let stream = Stream::new("work", "Work", None, "local").unwrap();
    let records = vec![
        session("local", "work", "first", Status::Active),
        session("local", "work", "parked", Status::Parked),
        session("local", "work", "done", Status::Done),
        session("local", "other", "moved", Status::Active),
        session("remote", "work", "replica", Status::Active),
        session("local", "work", "second", Status::Active),
        RecordRead {
            record: Record::Stream(stream.clone()),
            revision: "observed".into(),
        },
    ];
    let selected = requested_sessions(&stream, &records, None).unwrap();
    assert_eq!(selected.len(), 2);
    assert_eq!(selected[0].harness_session_id, "first");
    assert_eq!(selected[1].harness_session_id, "second");
    let mut reversed = records.clone();
    reversed.reverse();
    let selected = requested_sessions(&stream, &reversed, None).unwrap();
    let ids: std::collections::BTreeSet<_> = selected.iter().map(|session| &session.id).collect();
    assert_eq!(
        ids,
        requested_sessions(&stream, &records, None)
            .unwrap()
            .iter()
            .map(|session| &session.id)
            .collect()
    );
}

#[test]
fn explicit_session_selection_can_resume_parked_or_done_but_never_another_membership() {
    let stream = Stream::new("work", "Work", None, "local").unwrap();
    let records = vec![
        session("local", "work", "done", Status::Done),
        session("local", "work", "parked", Status::Parked),
        session("local", "other", "moved", Status::Active),
    ];
    assert!(
        requested_sessions(&stream, &records, None)
            .unwrap()
            .is_empty()
    );
    let members = member_sessions(&stream, &records);
    for session in members {
        let selected =
            requested_sessions(&stream, &records, Some(&format!("local/{}", session.id))).unwrap();
        assert_eq!(selected, [session]);
    }
    assert!(matches!(
        requested_sessions(&stream, &records, Some("Lane")),
        Err(CliError::Ambiguous { .. })
    ));
    let Record::Session(moved) = &records[2].record else {
        unreachable!()
    };
    assert!(matches!(
        requested_sessions(&stream, &records, Some(&moved.id)),
        Err(CliError::NotFound(_))
    ));
}
