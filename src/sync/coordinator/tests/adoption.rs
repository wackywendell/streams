//! Adoption recovery is independent of how a repository materializes revisions.
use super::*;

#[test]
fn partial_checkout_retains_both_histories_and_fences_fresh_coordinators() {
    let (a, b) = pair();
    accepted(a.save(&create("local", "local", "a")));
    accepted(b.save(&create("remote", "remote", "b")));
    confirmed(b.sync());
    let old = a.head().unwrap();
    let before = a.snapshot();
    let retained = a.retained.borrow().len();
    a.checkout_failure.set(Some(CheckoutFailure::AfterFiles));
    a.now.set(2000.0);
    let result = a.sync();
    assert_eq!(
        (result.sharing, result.local),
        (SharingStatus::Confirmed, LocalStatus::RepairRequired)
    );
    let marker = a.marker().unwrap();
    let PublicationMarker::Adopting(adoption) = &marker else {
        panic!("must fence partial adoption")
    };
    let candidate = adoption.adoption_to().as_str();
    assert_eq!(adoption.adoption_from().as_str(), old);
    assert_eq!(a.head().unwrap(), old);
    assert_ne!(*a.records.borrow(), before);
    assert_eq!(
        a.records.borrow().catalog,
        *a.history(Some(candidate)).unwrap().catalog()
    );
    assert_eq!(
        marker.attempt().confirmed_at(),
        Some(Seconds::try_from(2000.0).unwrap())
    );
    assert_eq!(a.metadata().last_push, marker.attempt().confirmed_at());
    let refs = a.retained.borrow();
    assert_eq!(refs.len(), retained + 3);
    assert_eq!(refs.iter().filter(|tip| *tip == &old).count(), 1);
    assert_eq!(
        refs.iter().filter(|tip| tip.as_str() == candidate).count(),
        2
    );
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
