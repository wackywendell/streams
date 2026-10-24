use super::*;
use crate::model::NamingMode;
#[test]
fn offline_relationships_rekey_saved_subjects_drop_claims_and_preserve_pending_attempt_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let state = RuntimeState::new(directory.path().join("state"), "host").unwrap();
    let scope = herdr_scope(Path::new("/fixture"), None);
    {
        let mut guard = state.enrollment(Some((&scope, "workspace")), None).unwrap();
        guard.claim("work").unwrap();
    }
    let binding = Association::new(
        "work",
        "workspace",
        Some(LaunchReference::new("old", "pi --session native").unwrap()),
        true,
    )
    .unwrap();
    let plan = Arrangement::new(
        "work".into(),
        "workspace".into(),
        vec![
            ArrangementTab::new(
                "tab".into(),
                Some("Manual".into()),
                NamingMode::Manual,
                vec![
                    ArrangedSession::new(
                        "old".into(),
                        "pi".into(),
                        "native".into(),
                        None,
                        Some("pane".into()),
                        Some("terminal".into()),
                    )
                    .unwrap(),
                ],
            )
            .unwrap(),
        ],
    )
    .unwrap();
    {
        let mut guard = state.stream_lock(Some(&scope), "work").unwrap();
        guard.write_association(&binding).unwrap();
        guard.write_arrangement(&plan).unwrap();
    }
    {
        let mut guard = state.stream_lock(Some(&scope), "foreign").unwrap();
        guard
            .write_association(
                &Association::new("foreign", "foreign-workspace", None, true).unwrap(),
            )
            .unwrap();
    }
    {
        let mut guard = state.stream_lock(Some(&scope), "pending").unwrap();
        let _attempt = guard
            .begin_attempt(None, directory.path().to_str().unwrap(), None)
            .unwrap();
    }
    let pending_path = state.path("attempts", &scope, "pending").unwrap();
    let before = fs::read(&pending_path).unwrap();
    let canonical = Session::id_for("host", "pi", "native");
    let prepared = state
        .prepare_cutover(
            &BTreeMap::from([("old".into(), canonical.clone())]),
            &BTreeSet::from(["work".into(), "pending".into()]),
        )
        .unwrap();
    let archive = directory.path().join("archive");
    fs::create_dir(&archive).unwrap();
    prepared.install(&archive).unwrap();
    drop(prepared);
    assert_eq!(
        state
            .association(&scope, "workspace")
            .unwrap()
            .unwrap()
            .launch()
            .unwrap()
            .session_id(),
        canonical
    );
    assert_eq!(
        state.arrangement(&scope, "work").unwrap().unwrap().tabs()[0].sessions()[0].session_id(),
        canonical
    );
    assert_eq!(
        state.arrangement(&scope, "work").unwrap().unwrap().tabs()[0].label(),
        Some("Manual")
    );
    assert!(
        state
            .association(&scope, "foreign-workspace")
            .unwrap()
            .is_none()
    );
    assert!(
        state
            .enrollment_claim(&scope, "workspace")
            .unwrap()
            .is_none()
    );
    assert_eq!(fs::read(&pending_path).unwrap(), before);
    assert!(state.stream_lock(Some(&scope), "pending").is_err());
    assert!(archive.join("runtime/arrangements").exists());
    assert!(archive.join("runtime/enrollments").exists());
    assert!(!archive.join("runtime-staging").exists());
}
#[test]
fn changed_relationship_while_waiting_for_its_class_lock_refuses_without_rewriting_it() {
    let directory = tempfile::tempdir().unwrap();
    let state = RuntimeState::new(directory.path(), "host").unwrap();
    let scope = herdr_scope(Path::new("/fixture"), None);
    let binding = Association::new("work", "workspace", None, true).unwrap();
    let mut held = state.stream_lock(Some(&scope), "work").unwrap();
    held.write_association(&binding).unwrap();
    let workspace_lock = state
        .path("enrollments", &scope, "workspace")
        .unwrap()
        .with_extension("lock");
    let working = state.clone();
    let task = std::thread::spawn(move || {
        working.prepare_cutover(&BTreeMap::new(), &BTreeSet::from(["work".into()]))
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !workspace_lock.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "cutover did not reach class locks"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let updated = Association::new("work", "workspace", None, false).unwrap();
    held.write_association(&updated).unwrap();
    let path = state.path("associations", &scope, "workspace").unwrap();
    let changed = fs::read(&path).unwrap();
    drop(held);
    let result = task.join().unwrap();
    assert!(result.is_err());
    assert!(
        result
            .err()
            .unwrap()
            .to_string()
            .contains("changed while acquiring cutover locks")
    );
    assert_eq!(fs::read(path).unwrap(), changed);
    assert_eq!(
        state.association(&scope, "workspace").unwrap(),
        Some(updated)
    );
}

#[test]
fn unusable_native_references_are_not_turned_into_new_permissions_or_guessed() {
    let directory = tempfile::tempdir().unwrap();
    let state = RuntimeState::new(directory.path(), "host").unwrap();
    let scope = herdr_scope(Path::new("/fixture"), None);
    let binding = Association::new(
        "work",
        "workspace",
        Some(LaunchReference::new("missing", "exact command").unwrap()),
        true,
    )
    .unwrap();
    {
        let mut guard = state.stream_lock(Some(&scope), "work").unwrap();
        guard.write_association(&binding).unwrap();
    }
    assert!(
        state
            .prepare_cutover(&BTreeMap::new(), &BTreeSet::from(["work".into()]))
            .is_err()
    );
    assert_eq!(
        state.association(&scope, "workspace").unwrap(),
        Some(binding)
    );
}
