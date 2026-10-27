use super::*;
use std::cell::Cell;

thread_local! {
    static INSPECTIONS: Cell<usize> = const { Cell::new(0) };
}
fn inspect(path: &Path) -> Result<Option<RepositoryInspection>> {
    INSPECTIONS.set(INSPECTIONS.get() + 1);
    Ok(Some(RepositoryInspection {
        root: path.to_owned(),
        relative_cwd: ".".into(),
    }))
}

#[test]
fn display_snapshot_reads_the_registry_once_and_inspects_each_requested_root_once() {
    for aliases in [1, 4] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let registry = CheckoutRegistry::new(root.join("state"), "host")
            .unwrap()
            .with_inspector(inspect);
        let mut checkout = None;
        for index in 0..aliases {
            checkout = Some(
                registry
                    .register(&root, Some(&format!("repository-{index}")))
                    .unwrap(),
            );
        }
        let checkout = checkout.unwrap();
        INSPECTIONS.set(0);
        let mut snapshot = registry.snapshot().unwrap();
        // A display operation uses its captured registry, not another filesystem read per alias.
        fs::write(registry.path(), "not valid registry JSON").unwrap();
        assert_eq!(snapshot.directory(&checkout.id, ".").unwrap(), root);
        assert_eq!(snapshot.get(&checkout.id).unwrap(), checkout);
        for index in 0..aliases {
            assert_eq!(
                snapshot
                    .resolve(&format!("repository-{index}"), Some(&checkout.id), None)
                    .unwrap(),
                checkout
            );
        }
        assert_eq!(INSPECTIONS.get(), 1);
        assert!(
            registry.get(&checkout.id).is_err(),
            "actions must still reopen the current registry"
        );
        assert!(
            registry.snapshot().is_err(),
            "the next display operation must reopen the registry too"
        );
    }
}
