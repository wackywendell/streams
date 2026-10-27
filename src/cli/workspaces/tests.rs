use super::*;
use std::{fs, sync::Mutex};
use streams::{
    config::Config,
    yaml_store::records::{RecordIoEvent, YamlRecordStore, relative_record_path},
};

#[test]
fn selected_session_key_reads_only_its_record_without_catalog_discovery() {
    for unrelated in [0, 64] {
        let directory = tempfile::tempdir().unwrap();
        let cwd = directory.path().canonicalize().unwrap();
        let environment = BTreeMap::from([("HOME".into(), cwd.to_str().unwrap().into())]);
        let config = Config::new(&environment, &cwd).unwrap();
        fs::create_dir_all(config.host_path().parent().unwrap()).unwrap();
        fs::write(
            config.host_path(),
            json!({"version":1,"id":"01234567-89ab-4def-8123-456789abcdef"}).to_string(),
        )
        .unwrap();
        let host = config.host_identity().unwrap();
        let settings = config.load().unwrap();
        YamlRecordStore::new(&settings.catalog_root, host.as_str())
            .unwrap()
            .initialize()
            .unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let opened = application::open_records_observed(&config, move |event| {
            observed.lock().unwrap().push(event)
        })
        .unwrap();
        let stream = Stream::new("work", "Work", None, host.as_str()).unwrap();
        let mut member = Session::new(
            Session::id_for(host.as_str(), "pi", "native"),
            host.as_str(),
            "work",
            "pi",
            "native",
            "Lane",
        )
        .unwrap();
        member.status = Status::Done;
        application::create_record(opened.records(), "stream", &Record::Stream(stream.clone()))
            .unwrap();
        application::create_record(
            opened.records(),
            "session",
            &Record::Session(member.clone()),
        )
        .unwrap();
        let neighbor = opened.root().join("hosts/unrelated/streams");
        fs::create_dir_all(&neighbor).unwrap();
        for index in 0..unrelated {
            fs::write(neighbor.join(format!("bad-{index}.yaml")), "not a record").unwrap();
        }
        let selection = SessionSelection {
            reference: None,
            key: Some(format!("{}/{}", host.as_str(), member.id)),
        };
        events.lock().unwrap().clear();
        assert_eq!(
            selected_sessions(&opened, &stream, &selection, None).unwrap(),
            [member.clone()]
        );
        let reads: Vec<_> = events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                RecordIoEvent::Read(path) => Some(path.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            reads,
            [
                opened.root().join(".catalog-local/records.json"),
                opened.root().join(
                    relative_record_path(&RecordAddress::for_record(&Record::Session(member)))
                        .unwrap()
                )
            ]
        );
        assert!(
            !events
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event, RecordIoEvent::List(_)))
        );
        let other_stream = Stream::new("other", "Other", None, host.as_str()).unwrap();
        assert!(selected_sessions(&opened, &other_stream, &selection, None).is_err());
        for key in [
            "not-qualified",
            "foreign/session",
            &format!("{}/missing", host.as_str()),
        ] {
            assert!(
                selected_sessions(
                    &opened,
                    &stream,
                    &SessionSelection {
                        reference: None,
                        key: Some(key.into())
                    },
                    None
                )
                .is_err()
            );
        }
    }
}

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
