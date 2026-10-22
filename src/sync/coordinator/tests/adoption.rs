//! Adoption recovery is independent of how a repository materializes revisions.
use super::*;
use crate::storage::{Store, Transaction};
use crate::sync::{
    baseline::{Baseline, encode_baseline},
    local::LocalCatalog,
};

use crate::memory;

#[test]
fn imported_baseline_keeps_creation_authority_and_retired_retries_without_writes() {
    let store = memory::MemoryStore::default();
    let local = LocalCatalog::new(store.clone(), "host").unwrap();
    let creation = create("original-create", "work", "host");
    let retired = crate::operations::update(
        "retired-edit",
        creation.after(),
        json!({"title":"Edited"}).as_object().unwrap(),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    accepted(local.save(&creation, None).unwrap());
    accepted(local.save(&retired, None).unwrap());
    let (catalog, baseline, objects) = store
        .with_transaction(|tx| -> Result<_, StorageError> {
            let state = State::from_journal(tx.journal());
            let creations = serde_json::from_value(
                state.read("local.json").unwrap().unwrap()["creations"].clone(),
            )
            .unwrap();
            let baseline = Baseline::new("host", creations, state.fingerprints().unwrap()).unwrap();
            let mut objects = BTreeMap::new();
            for name in tx.journal().names()? {
                objects.insert(name.clone(), tx.journal().read(&name)?.unwrap());
            }
            Ok((Transaction::read(tx)?.catalog, baseline, objects))
        })
        .unwrap();
    let histories = Rc::<RefCell<Histories>>::default();
    let root = histories.borrow_mut().append(
        None,
        Some(retired.after()),
        &encode_baseline(&baseline).unwrap(),
    );
    let repo = MemoryRepository::new(
        histories,
        Rc::new(RefCell::new(Some(root.clone()))),
        "host",
        false,
    );
    // Import domain data, not another store's opaque revision tokens.
    assert_eq!(repo.snapshot().catalog, catalog);
    let snapshot = repo.snapshot();
    for (name, value) in objects {
        repo.journal.write(&name, value).unwrap();
    }
    let original = repo.journal.objects.borrow().clone();
    let authority = repo.run(|core| core.authority("work")).unwrap().unwrap();
    assert_eq!(authority.host_id, "host");
    assert_eq!(authority.token, creation.id());
    let history = repo.history(Some(&root)).unwrap();
    assert!(history.operations().is_empty());
    for operation in [&creation, &retired] {
        assert!(history.has_operation(operation.id()));
        let retry = repo.save(operation);
        assert!(retry.accepted());
        assert_eq!(retry.domain.unwrap().outcome(), DomainOutcome::NoChange);
    }
    assert_eq!(repo.snapshot(), snapshot);
    assert_eq!(repo.head().unwrap(), root);
    assert_eq!(*repo.journal.objects.borrow(), original);
}

#[test]
fn checkout_failure_retains_both_histories_and_fences_fresh_coordinators() {
    let (a, b) = pair();
    accepted(a.save(&create("local", "local", "a")));
    accepted(b.save(&create("remote", "remote", "b")));
    confirmed(b.sync());
    let old = a.head().unwrap();
    a.checkout_failure.set(true);
    a.now.set(2000.0);
    let result = a.sync();
    assert_eq!(
        (result.sharing, result.local),
        (SharingStatus::Confirmed, LocalStatus::RepairRequired)
    );
    let marker = a.marker().unwrap();
    let PublicationMarker::Adopting(adoption) = &marker else {
        panic!("must fence adoption failure")
    };
    let candidate = adoption.adoption_to().as_str();
    assert_eq!(adoption.adoption_from().as_str(), old);
    assert_eq!(a.head().unwrap(), old);
    assert_eq!(
        marker.attempt().confirmed_at(),
        Some(Seconds::try_from(2000.0).unwrap())
    );
    assert_eq!(a.metadata().last_push, marker.attempt().confirmed_at());
    let refs = a.retained.borrow();
    assert!(refs.iter().any(|tip| tip == &old));
    assert!(refs.iter().any(|tip| tip.as_str() == candidate));
    for tip in refs.iter() {
        a.history(Some(tip)).unwrap();
    }
    drop(refs);
    assert!(a.run(|core| core.snapshot()).is_err());
    assert!(a.save(&create("later", "later", "a")).requires_repair());
    assert!(a.run(|core| core.sync(SyncEvent::Force)).requires_repair());
    assert_eq!(a.marker(), Some(marker));
}

#[test]
fn retained_attempt_preserves_original_intent_and_provenance_after_later_save() {
    let (a, b) = pair();
    let operation = create("first", "first", "a");
    let origin = Origin::from([
        ("host".into(), "original-host".into()),
        ("session".into(), "original-session".into()),
    ]);
    accepted(a.run(|core| core.save(&operation, Some(&origin)).unwrap()));
    a.pushes.borrow_mut().push_back(Push::LoseAck {
        accepted: true,
        offline: true,
    });
    assert_eq!(a.sync().sharing, SharingStatus::Uncertain);
    accepted(a.save(&create("later", "later", "a")));
    a.fetch_unavailable.set(false);
    confirmed(a.sync());
    assert!(a.marker().is_none());
    confirmed(b.sync());
    let history = b.history(Some(&b.head().unwrap())).unwrap();
    assert_eq!(history.operation("first").unwrap().operation, operation);
    assert_eq!(history.operation("first").unwrap().origin, origin);
    assert!(history.has_operation("later"));
}
