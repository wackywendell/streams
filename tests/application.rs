use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

use serde_json::json;
use streams_temper::{
    application::{self, ApplicationError},
    config::{Config, ConfigKey, Marker, Strategy},
    model::{Record, Stream},
    operations::{Actor, Operation},
    storage::Store,
    sync::state::State,
    yaml_store::YamlStore,
};

const HOST: &str = "01234567-89ab-4def-8123-456789abcdef";
const OTHER: &str = "11234567-89ab-4def-8123-456789abcdef";

struct Fixture {
    temp: tempfile::TempDir,
    config: Config,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let environment = BTreeMap::from([
            ("HOME".into(), temp.path().to_str().unwrap().into()),
            (
                "XDG_CONFIG_HOME".into(),
                temp.path().join("config").to_str().unwrap().into(),
            ),
            (
                "XDG_DATA_HOME".into(),
                temp.path().join("data").to_str().unwrap().into(),
            ),
        ]);
        let config = Config::new(&environment, temp.path()).unwrap();
        let root = config.load().unwrap().catalog_root;
        Self { temp, config, root }
    }
    fn init(&self) -> application::Result<application::OpenedCatalog> {
        application::initialize(&self.config, Strategy::None, None, "main", false)
    }
    fn host(&self, id: &str) {
        fs::create_dir_all(self.config.host_path().parent().unwrap()).unwrap();
        fs::write(
            self.config.host_path(),
            json!({"version":1,"id":id}).to_string(),
        )
        .unwrap();
    }
}

#[test]
fn local_factory_generates_once_and_uses_only_backend_contract() {
    let f = Fixture::new();
    assert!(application::open(&f.config).is_err());
    assert!(!f.config.host_path().exists());
    assert!(!f.root.exists());
    let catalog = f.init().unwrap();
    let host = catalog.host().as_str().to_owned();
    assert_eq!(catalog.root(), f.root);
    let source = catalog.source_id().unwrap();
    let create = Operation::new(
        "create",
        None,
        Record::Stream(Stream::new("work", "Work", None, &host).unwrap()),
        Actor::User,
        None,
        vec![],
    )
    .unwrap();
    assert!(catalog.backend().save(&create, None).unwrap().accepted());
    let saved = catalog.backend().snapshot().unwrap();
    let reopened = application::open(&f.config).unwrap();
    assert_eq!(reopened.host().as_str(), host);
    assert_eq!(reopened.source_id().unwrap(), source);
    assert_eq!(reopened.backend().snapshot().unwrap(), saved);
    let retry = f.init().unwrap();
    assert_eq!(retry.host().as_str(), host);
    assert_eq!(retry.backend().snapshot().unwrap(), saved);
    assert!(!f.root.join(".git").exists());
}

#[test]
fn rejected_uninitialized_reads_and_adoptions_leave_catalog_untouched() {
    for (present, plain_git) in [(false, false), (true, false), (true, true)] {
        let f = Fixture::new();
        if present {
            fs::create_dir_all(&f.root).unwrap();
        }
        if plain_git {
            fs::create_dir(f.root.join(".git")).unwrap();
        }
        assert!(application::open(&f.config).is_err());
        assert!(application::initialize(&f.config, Strategy::Git, None, "main", true).is_err());
        assert_eq!(f.root.exists(), present);
        assert!(!f.config.host_path().exists());
        if present {
            let names: Vec<_> = fs::read_dir(&f.root)
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            assert_eq!(names.len(), usize::from(plain_git));
            if plain_git {
                assert_eq!(names[0], ".git");
            }
        }
    }
}

#[test]
fn nonregular_initialization_markers_are_rejected_without_catalog_lock_creation() {
    for link in [false, true] {
        let f = Fixture::new();
        let marker = f.root.join(".catalog-sync/cli.json");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        if link {
            let target = f.temp.path().join("marker-target");
            fs::write(&target, "{}").unwrap();
            symlink(target, &marker).unwrap();
        } else {
            fs::create_dir(&marker).unwrap();
        }
        assert!(application::open(&f.config).is_err());
        assert!(application::initialize(&f.config, Strategy::Git, None, "main", true).is_err());
        assert!(f.init().is_err());
        assert!(!f.root.join(".catalog.lock").exists());
        assert!(!f.config.host_path().exists());
    }
}

#[test]
fn marked_catalog_requires_restoration_not_regeneration() {
    let f = Fixture::new();
    f.host(HOST);
    f.init().unwrap();
    fs::remove_file(f.config.host_path()).unwrap();
    for outcome in [f.init(), application::open(&f.config)] {
        assert!(matches!(outcome, Err(ApplicationError::Config(_))));
    }
    assert!(!f.config.host_path().exists());
    f.host(OTHER);
    assert!(f.init().is_err());
    assert!(application::open(&f.config).is_err());
    f.host(HOST);
    assert_eq!(f.init().unwrap().host().as_str(), HOST);
    fs::write(f.config.host_path(), "not JSON").unwrap();
    assert!(f.init().is_err());
    assert_eq!(
        fs::read_to_string(f.config.host_path()).unwrap(),
        "not JSON"
    );
}

#[test]
fn mixed_strategy_adoption_and_remote_rejections_have_no_git_effects() {
    let f = Fixture::new();
    f.init().unwrap();
    for (strategy, remote, adopt) in [
        (Strategy::Git, None, false),
        (Strategy::None, Some("origin"), false),
        (Strategy::None, None, true),
        (Strategy::Git, Some("origin"), true),
    ] {
        assert!(application::initialize(&f.config, strategy, remote, "main", adopt).is_err());
        assert!(!f.root.join(".git").exists());
    }
    let fresh = Fixture::new();
    assert!(application::initialize(&fresh.config, Strategy::Git, None, "main", true).is_err());
    assert!(!fresh.root.exists());
    assert!(!fresh.config.host_path().exists());
}

#[test]
fn corrupt_markers_fail_before_host_creation() {
    for marker in [
        json!({"version":1,"strategy":"bogus","host":HOST}),
        json!({"version":1,"strategy":"none","host":HOST,"extra":true}),
        json!({"version":2,"strategy":"none","host":HOST}),
        json!({"version":1,"strategy":"none","host":"not-uuid"}),
    ] {
        let f = Fixture::new();
        fs::create_dir_all(f.root.join(".catalog-sync")).unwrap();
        fs::write(f.root.join(".catalog-sync/cli.json"), marker.to_string()).unwrap();
        assert!(f.init().is_err());
        assert!(!f.config.host_path().exists());
        assert!(application::open(&f.config).is_err());
    }
}

#[test]
fn nonempty_and_symlinked_roots_and_machinery_are_rejected() {
    let f = Fixture::new();
    fs::create_dir_all(&f.root).unwrap();
    fs::write(f.root.join("valuable"), "retain").unwrap();
    assert!(f.init().is_err());
    assert_eq!(
        fs::read_to_string(f.root.join("valuable")).unwrap(),
        "retain"
    );
    assert!(!f.root.join(".catalog-sync/cli.json").exists());
    for name in ["root", ".catalog-sync", ".catalog.lock", ".catalog-staging"] {
        let f = Fixture::new();
        let target = f.temp.path().join("target");
        fs::create_dir(&target).unwrap();
        if name == "root" {
            fs::create_dir_all(f.root.parent().unwrap()).unwrap();
            symlink(&target, &f.root).unwrap();
        } else {
            fs::create_dir_all(&f.root).unwrap();
            symlink(&target, f.root.join(name)).unwrap();
        }
        assert!(f.init().is_err(), "{name}");
        assert!(application::open(&f.config).is_err(), "{name}");
        assert!(fs::read_dir(target).unwrap().next().is_none());
    }
}

#[test]
fn nonempty_bookkeeping_and_opaque_import_fences_fail_closed() {
    let f = Fixture::new();
    let store = YamlStore::new(&f.root);
    store
        .with_transaction(|_| {
            State::new(&f.root)
                .write(
                    "receipts.json",
                    json!({"operations":{}}).as_object().unwrap().clone(),
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(f.init().is_err());
    assert!(!f.root.join(".catalog-sync/cli.json").exists());
    let f = Fixture::new();
    f.init().unwrap();
    let store = YamlStore::new(&f.root);
    store
        .with_transaction(|_| {
            State::new(&f.root)
                .write(
                    "save.json",
                    json!({"adoption":{"baseline":"opaque"}})
                        .as_object()
                        .unwrap()
                        .clone(),
                )
                .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        application::open(&f.config),
        Err(ApplicationError::Repair(_))
    ));
    assert!(f.init().is_err());
    assert!(f.root.join(".catalog-sync/save.json").exists());
}

#[test]
fn initialization_reads_settings_after_acquiring_config_lock() {
    let f = Fixture::new();
    let mut lock = f.config.lock().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let ready = barrier.clone();
    let config = f.config.clone();
    let thread = std::thread::spawn(move || {
        ready.wait();
        let opened = application::initialize(&config, Strategy::None, None, "main", false).unwrap();
        opened.root().to_path_buf()
    });
    barrier.wait();
    let selected = f.temp.path().join("selected");
    lock.set(ConfigKey::CatalogRoot, selected.to_str().unwrap())
        .unwrap();
    drop(lock);
    assert_eq!(thread.join().unwrap(), selected);
    assert!(!f.root.exists());
    let store = YamlStore::new(&selected);
    let marker = store
        .with_transaction(|_| Ok(Marker::read(&store.journal()).unwrap()))
        .unwrap()
        .unwrap();
    assert_eq!(marker.host, f.config.host_identity().unwrap());
}
