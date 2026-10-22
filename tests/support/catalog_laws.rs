// Resolve the library consistently from both unit and integration harnesses.
use super::catalog_api::{
    model::{Host, Record, Session, Stream},
    operations::{Actor, DomainOutcome, Operation, update},
    sync::CatalogBackend,
};
use serde_json::json;

pub fn creation() -> Operation {
    Operation::new(
        "create",
        None,
        Record::Stream(Stream::new("work", "Work", None, "host").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap()
}

fn edit(id: &str, before: &Record, title: &str, actor: Actor) -> Operation {
    update(
        id,
        before,
        json!({"title": title}).as_object().unwrap(),
        actor,
        (actor == Actor::Automatic).then(|| "create".into()),
        vec![],
    )
    .unwrap()
}

pub fn identical_retries_and_creation_authority_survive_reopening(
    open: &dyn Fn() -> Box<dyn CatalogBackend>,
) {
    let operation = creation();
    assert!(open().save(&operation, None).unwrap().accepted());
    let backend = open();
    let saved = backend.snapshot().unwrap();
    let retry = backend.save(&operation, None).unwrap();
    assert!(retry.accepted());
    assert_eq!(retry.domain.unwrap().outcome(), DomainOutcome::NoChange);
    assert_eq!(backend.snapshot().unwrap(), saved);
    let authority = backend.authority("work").unwrap().unwrap();
    assert_eq!(authority.host_id, "host");
    assert_eq!(authority.token, "create");
    assert!(backend.authority("absent").unwrap().is_none());

    let altered = Operation::new(
        "create",
        None,
        Record::Stream(Stream::new("work", "Different", None, "host").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    let rejected = backend.save(&altered, None).unwrap();
    assert!(rejected.requires_repair());
    assert!(rejected.domain.is_none());
    assert_eq!(backend.snapshot().unwrap(), saved);
}

pub fn captured_bases_and_manual_fields_are_not_refreshed_on_retry(
    open: &dyn Fn() -> Box<dyn CatalogBackend>,
) {
    let operation = creation();
    let backend = open();
    backend.save(&operation, None).unwrap();
    let inferred = edit("inferred", operation.after(), "Inferred", Actor::Automatic);
    backend
        .save(
            &edit("manual", operation.after(), "Chosen", Actor::User),
            None,
        )
        .unwrap();
    let chosen = backend.snapshot().unwrap();
    assert_eq!(
        open()
            .save(&inferred, None)
            .unwrap()
            .domain
            .unwrap()
            .outcome(),
        DomainOutcome::Superseded
    );
    assert_eq!(open().snapshot().unwrap(), chosen);
    let conflicting = edit("conflicting", operation.after(), "Other", Actor::User);
    let result = backend.save(&conflicting, None).unwrap();
    assert_eq!(result.domain.unwrap().outcome(), DomainOutcome::Conflict);
    assert_eq!(backend.snapshot().unwrap(), chosen);
}

pub fn producer_partitions_and_ownership_tokens_are_enforced(
    open: &dyn Fn() -> Box<dyn CatalogBackend>,
) {
    let backend = open();
    backend.save(&creation(), None).unwrap();
    let original = backend.snapshot().unwrap();
    let foreign = Operation::new(
        "foreign",
        None,
        Record::Session(
            Session::new("session", "other-host", "work", "pi", "native", "Work").unwrap(),
        ),
        Actor::User,
        Some("create".into()),
        vec![],
    )
    .unwrap();
    assert!(backend.save(&foreign, None).is_err());
    let foreign_host = Operation::new(
        "foreign-host",
        None,
        Record::Host(
            Host::new(
                "other-host",
                "Other",
                chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap(),
            )
            .unwrap(),
        ),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(backend.save(&foreign_host, None).is_err());
    let foreign_stream = Operation::new(
        "foreign-stream",
        None,
        Record::Stream(Stream::new("foreign", "Foreign", None, "other-host").unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(backend.save(&foreign_stream, None).is_err());
    let stale = Operation::new(
        "stale",
        None,
        Record::Session(Session::new("session", "host", "work", "pi", "native", "Work").unwrap()),
        Actor::User,
        Some("old-ownership-period".into()),
        vec![],
    )
    .unwrap();
    assert_eq!(
        backend
            .save(&stale, None)
            .unwrap()
            .domain
            .unwrap()
            .outcome(),
        DomainOutcome::Conflict
    );
    assert_eq!(backend.snapshot().unwrap(), original);
}

pub fn changed_cross_record_inputs_retire_generated_output_and_conflict_manual_intent(
    open: &dyn Fn() -> Box<dyn CatalogBackend>,
) {
    let backend = open();
    let create = creation();
    backend.save(&create, None).unwrap();
    let captured = Record::Host(
        Host::new(
            "host",
            "Original",
            chrono::DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z").unwrap(),
        )
        .unwrap(),
    );
    let checkin =
        Operation::new("checkin", None, captured.clone(), Actor::User, None, vec![]).unwrap();
    backend.save(&checkin, None).unwrap();
    let refreshed = update(
        "new-checkin",
        &captured,
        json!({"label": "Changed"}).as_object().unwrap(),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    backend.save(&refreshed, None).unwrap();
    let saved = backend.snapshot().unwrap();
    for (actor, expected) in [
        (Actor::Automatic, DomainOutcome::Superseded),
        (Actor::User, DomainOutcome::Conflict),
    ] {
        let generated = update(
            actor.as_ref(),
            create.after(),
            json!({"title": "Stale output"}).as_object().unwrap(),
            actor,
            (actor == Actor::Automatic).then(|| "create".into()),
            vec![captured.clone()],
        )
        .unwrap();
        let result = open().save(&generated, None).unwrap();
        assert_eq!(result.domain.unwrap().outcome(), expected);
        assert_eq!(backend.snapshot().unwrap(), saved);
    }
}
