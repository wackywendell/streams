use chrono::DateTime;
use serde_json::{Value, json};
use streams_temper::model::NamingMode;
use streams_temper::operations::{Actor, DomainOutcome, Reconciliation};
use streams_temper::{
    model::{Catalog, Host, Record, Repository, Session, Stream},
    operations::{Operation, reconcile, update},
};

fn stream() -> Record {
    let mut stream = Stream::new("work", "Work", None, "host").unwrap();
    stream.repositories.insert(
        "repo".to_string(),
        Repository::new("https://example.com/repo").unwrap(),
    );
    Record::Stream(stream)
}

fn edit(before: &Record, changes: Value) -> Operation {
    update(
        "edit",
        before,
        changes.as_object().unwrap(),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}

fn entry(field: &str, name: &str) -> Value {
    if field == "documents" {
        json!({"repository": "repo", "ref": "main", "path": name, "description": name})
    } else {
        json!({"url": format!("https://example.com/{name}"), "description": name})
    }
}

#[test]
fn only_stream_creations_establish_authority_for_either_actor() {
    for actor in [Actor::User, Actor::Automatic] {
        let creation = Operation::new("create", None, stream(), actor, None, vec![]).unwrap();
        let (id, authority) = creation.created_authority().unwrap();
        assert_eq!(id, "work");
        assert_eq!(authority.host_id, "host");
        assert_eq!(authority.token, "create");
    }
    let edited = edit(&stream(), json!({"title": "Edited"}));
    assert!(edited.created_authority().is_none());
    let session = Record::Session(
        Session::new("session", "host", "work", "pi", "native", "Session").unwrap(),
    );
    let host = Record::Host(
        Host::new(
            "host",
            "Host",
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z").unwrap(),
        )
        .unwrap(),
    );
    for actor in [Actor::User, Actor::Automatic] {
        for record in [&session, &host] {
            let token = matches!(record, Record::Session(_)).then(|| "create".into());
            let operation =
                Operation::new("create", None, record.clone(), actor, token, vec![]).unwrap();
            assert!(operation.created_authority().is_none());
        }
    }
}

#[test]
fn same_instant_at_different_offsets_is_not_a_changed_field() {
    let host = |timestamp| {
        Record::Host(
            Host::new(
                "host",
                "Host",
                DateTime::parse_from_rfc3339(timestamp).unwrap(),
            )
            .unwrap(),
        )
    };
    let before = host("2025-01-01T00:00:00Z");
    let after = host("2024-12-31T19:00:00-05:00");
    let operation = Operation::new(
        "edit",
        Some(before.clone()),
        after,
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(operation.changed_fields().is_empty());
    let catalog = Catalog::default().replacing(before).unwrap();
    assert_eq!(
        reconcile(&catalog, &operation, None).outcome(),
        DomainOutcome::NoChange
    );
}

#[test]
fn concurrent_additive_collections_keep_current_order_then_append_desired() {
    for field in ["documents", "issues", "prs"] {
        let old = entry(field, "old");
        let local = entry(field, "local");
        let remote = entry(field, "remote");
        let shared = entry(field, "shared");
        let before = edit(&stream(), json!({field: [old]})).after().clone();
        let operation = edit(&before, json!({field: [old, local, shared]}));
        let current = edit(&before, json!({field: [remote, shared, old]}))
            .after()
            .clone();
        let catalog = Catalog::default().replacing(current).unwrap();
        let result = reconcile(&catalog, &operation, None);
        assert_eq!(result.outcome(), DomainOutcome::Changes, "{field}");
        let Reconciliation::Changes { record, .. } = result else {
            panic!("expected changes");
        };
        let merged = streams_temper::codec::record_to_data(&record);
        assert_eq!(
            merged[field],
            json!([remote, shared, old, local]),
            "{field}"
        );
    }
}

#[test]
fn concurrent_removal_or_change_of_old_collection_entries_conflicts() {
    for field in ["documents", "issues", "prs"] {
        let old = entry(field, "old");
        let before = edit(&stream(), json!({field: [old]})).after().clone();
        let mut changed = old.clone();
        changed["description"] = json!("changed");
        for replacement in [json!([]), json!([changed])] {
            for mutate_desired in [false, true] {
                let additive = json!([old, entry(field, "added")]);
                let (desired, current) = if mutate_desired {
                    (replacement.clone(), additive)
                } else {
                    (additive, replacement.clone())
                };
                let operation = edit(&before, json!({field: desired}));
                let current = edit(&before, json!({field: current})).after().clone();
                let catalog = Catalog::default().replacing(current).unwrap();
                let result = reconcile(&catalog, &operation, None);
                assert_eq!(result.outcome(), DomainOutcome::Conflict, "{field}");
                assert_eq!(result.conflicts(), &[field], "{field}");
                assert!(matches!(result, Reconciliation::Conflict { .. }));
            }
        }
    }
}

#[test]
fn concurrent_additions_with_same_identity_but_different_contents_conflict() {
    for field in ["documents", "issues", "prs"] {
        let before = stream();
        let desired = entry(field, "new");
        let mut current = desired.clone();
        if field == "documents" {
            current["ref"] = json!("other-branch");
        } else {
            current["description"] = json!("other description");
        }
        let operation = edit(&before, json!({field: [desired]}));
        let current = edit(&before, json!({field: [current]})).after().clone();
        let catalog = Catalog::default().replacing(current).unwrap();
        let result = reconcile(&catalog, &operation, None);
        assert_eq!(result.outcome(), DomainOutcome::Conflict, "{field}");
        assert_eq!(result.conflicts(), &[field], "{field}");
    }
}

#[test]
fn unrelated_concurrent_field_edits_merge() {
    let before = stream();
    let operation = edit(&before, json!({"goal": "Ship the feature"}));
    let current = edit(&before, json!({"archived": true})).after().clone();
    let catalog = Catalog::default().replacing(current).unwrap();
    let result = reconcile(&catalog, &operation, None);
    assert_eq!(result.outcome(), DomainOutcome::Changes);
    let Reconciliation::Changes { record, .. } = result else {
        panic!("expected changes");
    };
    let Record::Stream(merged) = *record else {
        panic!("expected a stream");
    };
    assert_eq!(merged.goal.as_deref(), Some("Ship the feature"));
    assert_eq!(merged.goal_mode, NamingMode::Manual);
    assert!(merged.archived);
}

#[test]
fn applying_fields_normalizes_records_without_capturing_or_mutating_intent() {
    let Record::Stream(mut before) = stream() else {
        unreachable!()
    };
    before.goal = Some("Original".into());
    before.goal_short = Some("Summary".into());
    let before = Record::Stream(before);
    let unchanged = streams_temper::operations::apply_changes(
        &before,
        json!({"goal":"Original"}).as_object().unwrap(),
        Actor::Automatic,
    )
    .unwrap();
    assert_eq!(unchanged, before);
    let changes = json!({"title":"Renamed", "goal":"Revised"});
    let after = streams_temper::operations::apply_changes(
        &before,
        changes.as_object().unwrap(),
        Actor::User,
    )
    .unwrap();
    assert_eq!(edit(&before, changes).after(), &after);
    let Record::Stream(after) = after else {
        unreachable!()
    };
    assert_eq!(after.title_mode, NamingMode::Manual);
    assert_eq!(after.goal_mode, NamingMode::Manual);
    assert_eq!(after.goal_short, None);
    assert!(
        streams_temper::operations::apply_changes(
            &before,
            json!({"goal":1}).as_object().unwrap(),
            Actor::User,
        )
        .is_err()
    );
    let Record::Stream(before) = before else {
        unreachable!()
    };
    assert_eq!(before.title, "Work");
    assert_eq!(before.goal_short.as_deref(), Some("Summary"));
}

#[test]
fn reconciliation_receipts_keep_the_four_canonical_wire_shapes() {
    let record = stream().validated().unwrap();
    let record_wire = serde_json::to_string(&record).unwrap();
    let cases = [
        (
            Reconciliation::Changes { operation_id: "original".into(), record: Box::new(record.clone()) },
            format!(r#"{{"operation_id":"original","outcome":"changes","record":{record_wire},"conflicts":[],"reason":""}}"#),
        ),
        (
            Reconciliation::no_change("original", "already saved"),
            r#"{"operation_id":"original","outcome":"no-change","record":null,"conflicts":[],"reason":"already saved"}"#.into(),
        ),
        (
            Reconciliation::Superseded {
                operation_id: "original".into(),
                conflicts: vec!["inputs".into()],
                reason: "captured catalog input changed".into(),
            },
            r#"{"operation_id":"original","outcome":"superseded","record":null,"conflicts":["inputs"],"reason":"captured catalog input changed"}"#.into(),
        ),
        (
            Reconciliation::Conflict {
                operation_id: "original".into(),
                conflicts: vec!["identity".into()],
                reason: "record already exists".into(),
            },
            r#"{"operation_id":"original","outcome":"conflict","record":null,"conflicts":["identity"],"reason":"record already exists"}"#.into(),
        ),
    ];
    for (receipt, expected) in cases {
        assert_eq!(receipt.operation_id(), "original");
        assert_eq!(serde_json::to_string(&receipt).unwrap(), expected);
        let wire: Value = serde_json::from_str(&expected).unwrap();
        assert_eq!(wire["outcome"], json!(receipt.outcome()));
        assert_eq!(wire["conflicts"], json!(receipt.conflicts()));
        assert_eq!(wire["reason"], receipt.reason());
        if let Reconciliation::Changes {
            record: changed, ..
        } = receipt
        {
            assert_eq!(*changed, record);
            assert_eq!(wire["record"], serde_json::to_value(&record).unwrap());
        } else {
            assert!(wire["record"].is_null());
        }
    }
}

#[test]
fn native_reassociation_preserves_work_and_uses_source_authority() {
    use streams_temper::{
        operations::Authority,
        sync::envelope::{decode_operation, encode_operation},
    };
    let mut before = Session::new("session", "host", "a", "pi", "native", "Session").unwrap();
    before.title_mode = NamingMode::Manual;
    before.state = "Working".into();
    before.state_short = Some("Work".into());
    before.remaining = "Finish".into();
    before.checkout = Some(streams_temper::model::Checkout::new("repo", "branch", ".").unwrap());
    let streams: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|id| {
            let mut stream = Stream::new(id, id, None, "host").unwrap();
            stream
                .repositories
                .insert("repo".into(), Repository::new("url").unwrap());
            stream
        })
        .collect();
    let base = Catalog::new(streams, vec![before.clone()], vec![]).unwrap();
    let operation = Operation::reassociate(
        "move",
        &before,
        base.stream("a").unwrap(),
        "b",
        "source-token".into(),
    )
    .unwrap();
    assert_eq!(operation.actor(), Actor::Native);
    assert_eq!(
        operation.changed_fields().into_iter().collect::<Vec<_>>(),
        ["checkout", "previous_stream_ids", "stream_id"]
    );
    assert!(operation.check_producer("host").is_ok());
    assert!(operation.check_producer("other").is_err());
    let authorities = std::collections::BTreeMap::from([
        ("a".into(), Authority::new("host", "source-token").unwrap()),
        (
            "b".into(),
            Authority::new("host", "destination-token").unwrap(),
        ),
    ]);
    assert_eq!(operation.authority_for(&authorities), authorities.get("a"));
    let encoded = encode_operation(&operation, None).unwrap();
    assert_eq!(decode_operation(&encoded).unwrap().0, operation);
    for authority in [None, authorities.get("b")] {
        assert_eq!(
            reconcile(&base, &operation, authority).outcome(),
            DomainOutcome::Conflict
        );
    }
    let mut current = before.clone();
    current.state = "Concurrent progress".into();
    let catalog = base.replacing(Record::Session(current.clone())).unwrap();
    let result = reconcile(&catalog, &operation, operation.authority_for(&authorities));
    let Reconciliation::Changes { record, .. } = result else {
        panic!("expected move")
    };
    current.stream_id = "b".into();
    current.previous_stream_ids = vec!["a".into()];
    current.checkout.as_mut().unwrap().repository_url = Some("url".into());
    assert_eq!(*record, Record::Session(current.clone()));
    let moved = catalog.replacing(*record).unwrap();
    let generated = update(
        "generated",
        &Record::Session(before.clone()),
        json!({"state": "Generated"}).as_object().unwrap(),
        Actor::Automatic,
        Some("source-token".into()),
        vec![],
    )
    .unwrap();
    assert_eq!(
        reconcile(&moved, &generated, generated.authority_for(&authorities)).outcome(),
        DomainOutcome::Superseded,
    );
    // Membership guards must match as a group, not accept a mix of base and desired.
    let mut partial_membership = before.clone();
    partial_membership.stream_id = "b".into();
    let partial = base.replacing(Record::Session(partial_membership)).unwrap();
    assert_eq!(
        reconcile(&partial, &operation, authorities.get("a")).outcome(),
        DomainOutcome::Conflict,
    );
    assert_eq!(
        reconcile(&moved, &operation, authorities.get("a")).outcome(),
        DomainOutcome::NoChange
    );
    let noop = Operation::reassociate(
        "noop",
        &current,
        moved.stream("b").unwrap(),
        "b",
        "destination-token".into(),
    )
    .unwrap();
    assert!(noop.changed_fields().is_empty());
    assert_eq!(noop.after(), &Record::Session(current.clone()));
    assert_eq!(
        reconcile(&moved, &noop, authorities.get("b")).outcome(),
        DomainOutcome::NoChange
    );
    let revisit = Operation::reassociate(
        "return",
        &current,
        moved.stream("b").unwrap(),
        "a",
        "destination-token".into(),
    )
    .unwrap();
    let returned = moved.replacing(revisit.after().clone()).unwrap();
    assert_eq!(
        returned
            .session("host", "session")
            .unwrap()
            .previous_stream_ids,
        ["a", "b"]
    );
    for stale in [&operation, &noop] {
        assert_eq!(
            reconcile(&returned, stale, stale.authority_for(&authorities)).outcome(),
            DomainOutcome::Conflict
        );
    }
    let other_move = Operation::reassociate(
        "other",
        &before,
        base.stream("a").unwrap(),
        "c",
        "source-token".into(),
    )
    .unwrap();
    let moved_elsewhere = base.replacing(other_move.after().clone()).unwrap();
    assert_eq!(
        reconcile(&moved_elsewhere, &operation, authorities.get("a")).outcome(),
        DomainOutcome::Conflict
    );
    for field in ["harness", "harness_session_id"] {
        let mut altered = before.clone();
        match field {
            "harness" => altered.harness = "different".into(),
            _ => altered.harness_session_id = "different".into(),
        }
        let catalog = base.replacing(Record::Session(altered)).unwrap();
        assert_eq!(
            reconcile(&catalog, &operation, authorities.get("a")).conflicts(),
            &[field]
        );
    }
    let mut invalid_destination = base.clone();
    let mut destination = invalid_destination.stream("b").unwrap().clone();
    destination.repositories.clear();
    invalid_destination = invalid_destination
        .replacing(Record::Stream(destination))
        .unwrap();
    let result = reconcile(&invalid_destination, &operation, authorities.get("a"));
    assert_eq!(result.outcome(), DomainOutcome::Changes);
    let Reconciliation::Changes { record, .. } = result else {
        unreachable!()
    };
    let mut expected = current.clone();
    expected.state = before.state.clone();
    assert_eq!(*record, Record::Session(expected));

    let mut destination = base.stream("b").unwrap().clone();
    destination.owner_host_id = "foreign".into();
    let foreign_destination = base.replacing(Record::Stream(destination)).unwrap();
    assert_eq!(
        reconcile(&foreign_destination, &operation, authorities.get("a")).conflicts(),
        &["ownership"]
    );

    let stale_user = update(
        "late-user",
        &Record::Session(before),
        json!({"title": "Late title"}).as_object().unwrap(),
        Actor::User,
        Some("source-token".into()),
        vec![],
    )
    .unwrap();
    assert_eq!(
        reconcile(&moved, &stale_user, stale_user.authority_for(&authorities)).outcome(),
        DomainOutcome::Conflict
    );
}

#[test]
fn moved_checkout_keeps_its_repository_despite_alias_collisions_or_later_moves() {
    use streams_temper::{model::Checkout, operations::Authority};
    let mut a = Stream::new("a", "A", None, "host").unwrap();
    a.repositories.insert(
        "app".into(),
        Repository::new("https://example.test/original.git").unwrap(),
    );
    let mut b = Stream::new("b", "B", None, "host").unwrap();
    b.repositories.insert(
        "app".into(),
        Repository::new("https://example.test/different.git").unwrap(),
    );
    let mut session = Session::new("session", "host", "a", "pi", "native", "Work").unwrap();
    session.checkout = Some(Checkout::new("app", "feature", "services/api").unwrap());
    let catalog = Catalog::new(vec![a, b], vec![session.clone()], vec![]).unwrap();
    let operation = Operation::reassociate(
        "move",
        &session,
        catalog.stream("a").unwrap(),
        "b",
        "a-token".into(),
    )
    .unwrap();
    let auth = Authority::new("host", "a-token").unwrap();
    let mut changed_source = catalog.stream("a").unwrap().clone();
    changed_source.repositories.get_mut("app").unwrap().url =
        "https://example.test/replaced.git".into();
    let changed = catalog.replacing(Record::Stream(changed_source)).unwrap();
    assert_eq!(
        reconcile(&changed, &operation, Some(&auth)).conflicts(),
        &["inputs"]
    );
    let Reconciliation::Changes { record, .. } = reconcile(&catalog, &operation, Some(&auth))
    else {
        panic!("expected move")
    };
    let Record::Session(moved) = record.as_ref() else {
        unreachable!()
    };
    let checkout = moved.checkout.as_ref().unwrap();
    assert_eq!(
        checkout.repository_url.as_deref(),
        Some("https://example.test/original.git")
    );
    assert_eq!(checkout.r#ref, "feature");
    assert_eq!(checkout.directory_within_checkout, "services/api");
    assert_eq!(
        checkout.repository_url(catalog.stream("b").unwrap()),
        Some("https://example.test/original.git")
    );
    let mut catalog = catalog.replacing(*record).unwrap();
    // Neither source alias edits nor an unrelated next Stream may reinterpret the checkout.
    let mut a = catalog.stream("a").unwrap().clone();
    a.repositories.clear();
    catalog = catalog.replacing(Record::Stream(a)).unwrap();
    let current = catalog.session("host", "session").unwrap();
    let operation = Operation::reassociate(
        "return",
        current,
        catalog.stream("b").unwrap(),
        "a",
        "b-token".into(),
    )
    .unwrap();
    let auth = Authority::new("host", "b-token").unwrap();
    let Reconciliation::Changes { record, .. } = reconcile(&catalog, &operation, Some(&auth))
    else {
        panic!("expected return")
    };
    let Record::Session(returned) = record.as_ref() else {
        unreachable!()
    };
    assert_eq!(returned.checkout, current.checkout);
    assert_eq!(returned.previous_stream_ids, ["a", "b"]);
    let encoded = streams_temper::codec::record_to_data(record.as_ref());
    assert_eq!(
        streams_temper::codec::record_from_data(
            streams_temper::model::RecordKind::Session,
            &encoded
        )
        .unwrap(),
        *record
    );
}

#[test]
fn native_move_rejects_checkout_attached_or_replaced_after_capture() {
    use streams_temper::{model::Checkout, operations::Authority};
    let mut source = Stream::new("a", "Source", None, "host").unwrap();
    source.repositories.insert(
        "app".into(),
        Repository::new("https://example.test/original.git").unwrap(),
    );
    let mut destination = Stream::new("b", "Destination", None, "host").unwrap();
    destination.repositories.insert(
        "app".into(),
        Repository::new("https://example.test/different.git").unwrap(),
    );
    let mut bound = Checkout::new("app", "main", ".").unwrap();
    bound.repository_url = Some("https://example.test/original.git".into());
    for checkout in [None, Some(bound)] {
        let mut before = Session::new("session", "host", "a", "pi", "native", "Work").unwrap();
        before.checkout = checkout;
        let operation =
            Operation::reassociate("move", &before, &source, "b", "token".into()).unwrap();
        let mut current = before.clone();
        current.checkout = Some(Checkout::new("app", "main", ".").unwrap());
        let catalog = Catalog::new(
            vec![source.clone(), destination.clone()],
            vec![current],
            vec![],
        )
        .unwrap();
        let authority = Authority::new("host", "token").unwrap();
        let result = reconcile(&catalog, &operation, Some(&authority));
        assert_eq!(result.outcome(), DomainOutcome::Conflict);
        assert_eq!(result.conflicts(), &["checkout"]);
        assert_eq!(catalog.session("host", "session").unwrap().stream_id, "a");
    }
}

#[test]
fn native_noop_reassociation_still_validates_the_source_checkout_alias() {
    use streams_temper::model::Checkout;
    let source = Stream::new("a", "Source", None, "host").unwrap();
    let mut before = Session::new("session", "host", "a", "pi", "native", "Work").unwrap();
    before.checkout = Some(Checkout::new("missing", "main", ".").unwrap());
    assert!(Operation::reassociate("noop", &before, &source, "a", "token".into()).is_err());
}

#[test]
fn native_operations_reject_non_membership_shapes_and_generic_moves_stay_forbidden() {
    let before = Session::new("session", "host", "a", "pi", "native", "Session").unwrap();
    let source = Stream::new("a", "Source", None, "host").unwrap();
    let operation = Operation::reassociate("move", &before, &source, "b", "token".into()).unwrap();
    for actor in [Actor::User, Actor::Automatic] {
        for changes in [
            json!({"stream_id": "b"}),
            json!({"previous_stream_ids": ["old"]}),
        ] {
            assert!(
                update(
                    "generic",
                    &Record::Session(before.clone()),
                    changes.as_object().unwrap(),
                    actor,
                    Some("token".into()),
                    vec![]
                )
                .is_err()
            );
        }
    }
    for record in [stream(), Record::Session(before.clone())] {
        let token = matches!(&record, Record::Session(_)).then(|| "token".into());
        assert!(Operation::new("create", None, record, Actor::Native, token, vec![]).is_err());
    }
    assert!(
        Operation::new(
            "edit",
            Some(stream()),
            stream(),
            Actor::Native,
            None,
            vec![]
        )
        .is_err()
    );
    let host = Record::Host(
        Host::new(
            "host",
            "Host",
            DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z").unwrap(),
        )
        .unwrap(),
    );
    assert!(
        Operation::new(
            "host",
            Some(host.clone()),
            host,
            Actor::Native,
            None,
            vec![]
        )
        .is_err()
    );
    for field in [
        "title",
        "title_mode",
        "status",
        "state",
        "state_short",
        "remaining",
        "checkout",
        "harness",
        "harness_session_id",
        "host_id",
        "id",
        "previous_stream_ids",
    ] {
        let Record::Session(mut after) = operation.after().clone() else {
            unreachable!()
        };
        match field {
            "title" => after.title = "Changed".into(),
            "title_mode" => after.title_mode = NamingMode::Manual,
            "status" => after.status = streams_temper::model::Status::Done,
            "state" => after.state = "Changed".into(),
            "state_short" => after.state_short = Some("Changed".into()),
            "remaining" => after.remaining = "Changed".into(),
            "checkout" => {
                after.checkout =
                    Some(streams_temper::model::Checkout::new("repo", "main", ".").unwrap())
            }
            "harness" => after.harness = "other".into(),
            "harness_session_id" => after.harness_session_id = "other".into(),
            "host_id" => after.host_id = "other".into(),
            "id" => after.id = "other".into(),
            _ => after.previous_stream_ids = vec!["unrelated".into()],
        }
        assert!(
            Operation::new(
                "invalid",
                Some(Record::Session(before.clone())),
                Record::Session(after),
                Actor::Native,
                Some("token".into()),
                operation.inputs().to_vec()
            )
            .is_err(),
            "{field}"
        );
    }
    assert!(
        Operation::new(
            "inputs",
            operation.before().cloned(),
            operation.after().clone(),
            Actor::Native,
            Some("token".into()),
            vec![stream()]
        )
        .is_err()
    );
    let mut with_history = before.clone();
    with_history.previous_stream_ids = vec!["earlier".into()];
    let mut bad_prefix = with_history.clone();
    bad_prefix.stream_id = "b".into();
    bad_prefix.previous_stream_ids = vec!["unrelated".into(), "a".into()];
    assert!(
        Operation::new(
            "prefix",
            Some(Record::Session(with_history)),
            Record::Session(bad_prefix),
            Actor::Native,
            Some("token".into()),
            operation.inputs().to_vec(),
        )
        .is_err()
    );
    let mut missing_append = before.clone();
    missing_append.stream_id = "b".into();
    assert!(
        Operation::new(
            "append",
            Some(Record::Session(before.clone())),
            Record::Session(missing_append),
            Actor::Native,
            Some("token".into()),
            operation.inputs().to_vec(),
        )
        .is_err()
    );
    assert!(Operation::reassociate("move", &before, &source, "", "token".into()).is_err());
    assert!(Operation::reassociate("move", &before, &source, "b", "".into()).is_err());
    let mut same_stream = before.clone();
    same_stream.previous_stream_ids = vec!["other".into()];
    assert!(
        Operation::new(
            "history",
            Some(Record::Session(before)),
            Record::Session(same_stream),
            Actor::Native,
            Some("token".into()),
            operation.inputs().to_vec()
        )
        .is_err()
    );
}

#[test]
fn creation_changed_fields_use_all_logical_names() {
    let operation = Operation::new("create", None, stream(), Actor::User, None, vec![]).unwrap();
    let fields = operation.changed_fields();
    assert_eq!(fields.len(), 13);
    assert!(fields.contains("id"));
    assert!(fields.contains("owner_host_id"));
    assert!(!fields.contains("owner"));
}
