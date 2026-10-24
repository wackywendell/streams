use super::*;
use crate::runtime::checkouts::CheckoutRegistry;
fn bindings(root: &Path) -> DirectoryBindings {
    DirectoryBindings::new(
        root.join("runtime"),
        CheckoutRegistry::new(root.join("registry"), "host").unwrap(),
    )
}
fn legacy(bindings: &DirectoryBindings, stream: &str, session: Option<&str>, cwd: &Path) {
    write_json(
        &bindings.stream_path(stream, session),
        &Current {
            version: 2,
            stream_id: Some(stream.into()),
            session_id: session.map(str::to_owned),
            location: Location::Directory { cwd: cwd.into() },
        },
    )
    .unwrap();
}
#[test]
fn migration_rekeys_session_choices_keeps_stream_defaults_and_removes_legacy_sources() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let bindings = bindings(root);
    let archive = root.join("archive");
    fs::create_dir(&archive).unwrap();
    legacy(&bindings, "a", Some("old"), root);
    legacy(&bindings, "b", Some("old"), root);
    legacy(&bindings, "a", None, root);
    assert!(bindings.read("a", Some("new")).unwrap().is_none());
    let prepared = bindings
        .prepare_cutover(
            &BTreeMap::from([("old".into(), "new".into())]),
            &BTreeSet::from(["a".into(), "b".into()]),
        )
        .unwrap();
    prepared.install(&archive).unwrap();
    let expected = canonical_directory(root).unwrap();
    assert_eq!(
        bindings.read("a", Some("new")).unwrap().unwrap().cwd,
        expected
    );
    assert_eq!(
        bindings.read("b", Some("new")).unwrap().unwrap().cwd,
        expected
    );
    assert_eq!(bindings.read("a", None).unwrap().unwrap().cwd, expected);
    assert!(!bindings.stream_path("a", Some("old")).exists());
    assert!(!bindings.stream_path("b", Some("old")).exists());
    let wire: serde_json::Value =
        serde_json::from_slice(&fs::read(bindings.path("a", Some("new"))).unwrap()).unwrap();
    assert_eq!(wire["version"], 3);
    assert!(wire.get("stream_id").is_none());
    assert!(archive.join("runtime/directories").exists());
    assert!(!archive.join("runtime-staging").exists());
}
#[test]
fn conflicting_choices_refuse_without_selecting_one_by_order_or_rewriting_data() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let bindings = bindings(root);
    let other = root.join("other");
    fs::create_dir(&other).unwrap();
    legacy(&bindings, "a", Some("old"), root);
    legacy(&bindings, "b", Some("old"), &other);
    assert!(
        bindings
            .prepare_cutover(
                &BTreeMap::from([("old".into(), "new".into())]),
                &BTreeSet::from(["a".into(), "b".into()])
            )
            .is_err()
    );
    assert!(bindings.stream_path("a", Some("old")).exists());
    assert!(bindings.stream_path("b", Some("old")).exists());
    assert!(!bindings.path("a", Some("new")).exists());
}
#[test]
fn changed_selection_while_waiting_for_its_lock_refuses_without_rewriting_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let bindings = bindings(root);
    legacy(&bindings, "a", None, root);
    legacy(&bindings, "b", None, root);
    let mut paths = [
        bindings.stream_path("a", None),
        bindings.stream_path("b", None),
    ];
    paths.sort();
    let held = lock(&paths[1].with_extension("lock")).unwrap();
    let working = bindings.clone();
    let task = std::thread::spawn(move || {
        working.prepare_cutover(&BTreeMap::new(), &BTreeSet::from(["a".into(), "b".into()]))
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !paths[0].with_extension("lock").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "cutover did not reach class locks"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let mut selection: Current = crate::file_io::read_json(&paths[1]).unwrap().unwrap();
    let other = root.join("other");
    fs::create_dir(&other).unwrap();
    selection.location = Location::Directory { cwd: other };
    write_json(&paths[1], &selection).unwrap();
    let changed = fs::read(&paths[1]).unwrap();
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
    assert_eq!(fs::read(&paths[1]).unwrap(), changed);
}

#[test]
fn normal_reads_never_list_or_promote_legacy_choices() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let bindings = bindings(root);
    legacy(&bindings, "a", Some("old"), root);
    fs::write(
        bindings.state_dir.join("directories/unrelated.json"),
        "not json",
    )
    .unwrap();
    assert!(bindings.read("a", Some("old")).unwrap().is_none());
    assert!(bindings.stream_path("a", Some("old")).exists());
    assert!(!bindings.path("a", Some("old")).exists());
}
