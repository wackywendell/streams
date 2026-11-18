use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};

use serde_json::json;
use sha2::{Digest, Sha256};
use streams::config::{Config, ConfigKey, HostIdentity, resolve_path, source_id};
use streams::frontend::{FrontendId, FrontendMode};
use streams::model::{NamingMode, TitleDefaults};

const HOST: &str = "01234567-89ab-4def-8123-456789abcdef";

struct Fixture {
    _temp: tempfile::TempDir,
    home: PathBuf,
    config: Config,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().to_path_buf();
        let env = BTreeMap::from([
            ("HOME".into(), home.to_str().unwrap().into()),
            (
                "XDG_CONFIG_HOME".into(),
                home.join("config").to_str().unwrap().into(),
            ),
            (
                "XDG_DATA_HOME".into(),
                home.join("data").to_str().unwrap().into(),
            ),
            (
                "XDG_STATE_HOME".into(),
                home.join("state").to_str().unwrap().into(),
            ),
        ]);
        let config = Config::new(&env, &home).unwrap();
        Self {
            _temp: temp,
            home,
            config,
        }
    }
    fn write(&self, text: &str) {
        fs::create_dir_all(self.config.config_path().parent().unwrap()).unwrap();
        fs::write(self.config.config_path(), text).unwrap();
    }
    fn host(&self, text: &str) {
        fs::create_dir_all(self.config.host_path().parent().unwrap()).unwrap();
        fs::write(self.config.host_path(), text).unwrap();
    }
}

#[test]
fn invalid_settings_preserve_reader_writer_diagnostics_and_contents() {
    let f = Fixture::new();
    for document in [
        "[",
        "[frontend]\nmode = \"none\"\nmode = \"auto\"\n",
        "[catalog]\nunknown = \"x\"\n",
        "[sync]\nstream_seconds = \"12\"\n",
    ] {
        f.write(document);
        let error = f.config.read_overrides().unwrap_err().to_string();
        assert!(!error.is_empty());
        assert!(f.config.load().is_err());
        assert_eq!(
            f.config
                .set(ConfigKey::FrontendMode, "herdr")
                .unwrap_err()
                .to_string(),
            error
        );
        assert_eq!(
            fs::read_to_string(f.config.config_path()).unwrap(),
            document
        );
    }
}

#[test]
fn defaults_and_reads_create_nothing() {
    let f = Fixture::new();
    assert_eq!(
        f.config.config_path(),
        f.home.join("config/streams/config.toml")
    );
    assert_eq!(f.config.read_overrides().unwrap(), json!({}));
    let settings = f.config.load().unwrap();
    assert_eq!(settings.catalog_root, f.home.join("data/streams/catalog"));
    assert_eq!(settings.frontend, FrontendMode::Auto);
    assert_eq!(settings.naming, TitleDefaults::default());
    assert_eq!(
        f.config.checkout_state_dir(),
        f.home.join("state/streams/checkouts")
    );
    assert!(
        f.config
            .runtime_state_dir_for(&settings.catalog_root)
            .unwrap()
            .starts_with(f.home.join("state/streams/catalogs"))
    );
    assert!(f.config.host_identity().is_err());
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
}

#[test]
fn generation_defaults_are_persisted_and_resolved_independently_of_frontend_policy() {
    let f = Fixture::new();
    f.config.set(ConfigKey::StreamNaming, "manual").unwrap();
    let settings = f.config.load().unwrap();
    assert_eq!(settings.naming.stream, NamingMode::Manual);
    assert_eq!(settings.naming.session, NamingMode::Automatic);
    assert_eq!(settings.frontend, FrontendMode::Auto);
    f.config.set(ConfigKey::SessionNaming, "manual").unwrap();
    f.config.set(ConfigKey::FrontendMode, "none").unwrap();
    f.config.set(ConfigKey::StreamNaming, "automatic").unwrap();
    assert_eq!(
        f.config.values().unwrap()["naming"],
        json!({"stream":"automatic","session":"manual"})
    );
    assert_eq!(
        f.config.read_overrides().unwrap()["naming"],
        json!({"stream":"automatic","session":"manual"})
    );
}

#[test]
fn home_fallbacks_and_explicit_environment() {
    let f = Fixture::new();
    let env = BTreeMap::from([
        ("HOME".into(), f.home.to_str().unwrap().into()),
        ("XDG_CONFIG_HOME".into(), "".into()),
        ("XDG_DATA_HOME".into(), "".into()),
        ("XDG_STATE_HOME".into(), "".into()),
    ]);
    let config = Config::new(&env, &f.home).unwrap();
    assert_eq!(
        config.config_path(),
        f.home.join(".config/streams/config.toml")
    );
    assert_eq!(
        config.load().unwrap().catalog_root,
        f.home.join(".local/share/streams/catalog")
    );
    assert_eq!(
        config.checkout_state_dir(),
        f.home.join(".local/state/streams/checkouts")
    );
    assert!(Config::new(&BTreeMap::new(), &f.home).is_err());
    assert!(Config::new(&env, Path::new("relative")).is_err());
    let mut relative = env.clone();
    relative.insert("XDG_CONFIG_HOME".into(), "relative".into());
    assert_eq!(
        Config::new(&relative, &f.home).unwrap().config_path(),
        f.home.join("relative/streams/config.toml")
    );
    relative.insert("XDG_DATA_HOME".into(), "relative".into());
    let config = Config::new(&relative, &f.home).unwrap();
    assert!(config.load().is_err());
    config
        .set(ConfigKey::CatalogRoot, "~/valid-override")
        .unwrap();
    assert_eq!(
        config.load().unwrap().catalog_root,
        f.home.join("valid-override")
    );
}

#[test]
fn expanduser_preserves_home_with_redundant_slashes() {
    let f = Fixture::new();
    for (text, expected) in [
        ("~", f.home.clone()),
        ("~///", f.home.clone()),
        ("~//foo", f.home.join("foo")),
    ] {
        f.config.set(ConfigKey::CatalogRoot, text).unwrap();
        assert_eq!(f.config.load().unwrap().catalog_root, expected);
    }
    let mut size = 4096;
    let (name, home) = loop {
        let mut buffer = vec![0u8; size];
        let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut found = std::ptr::null_mut();
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                entry.as_mut_ptr(),
                buffer.as_mut_ptr().cast(),
                size,
                &mut found,
            )
        };
        if rc == libc::ERANGE {
            size *= 2;
            continue;
        }
        assert_eq!(rc, 0);
        assert!(!found.is_null());
        let name = unsafe { std::ffi::CStr::from_ptr((*found).pw_name) }
            .to_str()
            .unwrap()
            .to_owned();
        let home = unsafe { std::ffi::CStr::from_ptr((*found).pw_dir) }
            .to_str()
            .unwrap()
            .to_owned();
        break (name, PathBuf::from(home));
    };
    for (tail, expected) in [
        ("", home.clone()),
        ("///", home.clone()),
        ("//foo", home.join("foo")),
    ] {
        f.config
            .set(ConfigKey::CatalogRoot, &format!("~{name}{tail}"))
            .unwrap();
        assert_eq!(f.config.load().unwrap().catalog_root, expected);
    }
}

#[test]
fn overrides_get_set_and_closed_frontends() {
    let f = Fixture::new();
    f.config.set(ConfigKey::CatalogRoot, "~/Catalog").unwrap();
    assert!(
        f.config
            .set(ConfigKey::FrontendMode, "third-party-adapter")
            .is_err()
    );
    f.config.set(ConfigKey::FrontendMode, "hc").unwrap();
    let settings = f.config.load().unwrap();
    assert_eq!(settings.catalog_root, f.home.join("Catalog"));
    assert_eq!(settings.frontend, FrontendMode::Explicit(FrontendId::Hc));
    assert_eq!(
        f.config.get(ConfigKey::CatalogRoot).unwrap(),
        json!(f.home.join("Catalog"))
    );
    assert_eq!(f.config.get(ConfigKey::FrontendMode).unwrap(), "hc");
    assert_eq!(
        f.config.read_overrides().unwrap(),
        json!({
            "catalog":{"root":f.home.join("Catalog")}, "frontend":{"mode":"hc"}
        })
    );
    assert!(!f.home.join("Catalog").exists());
    f.config.set(ConfigKey::FrontendMode, "none").unwrap();
    assert_eq!(f.config.load().unwrap().frontend, FrontendMode::None);
    f.config.set(ConfigKey::FrontendMode, "auto").unwrap();
    assert_eq!(f.config.load().unwrap().frontend, FrontendMode::Auto);
}

#[test]
fn invalid_setting_text_is_rejected_before_locking_or_writing() {
    let f = Fixture::new();
    assert!(f.config.set(ConfigKey::FrontendMode, "").is_err());
    assert_eq!(fs::read_dir(&f.home).unwrap().count(), 0);
    let document = "[frontend]\nmode = \"none\"\n";
    f.write(document);
    assert!(f.config.set(ConfigKey::CatalogRoot, "relative").is_err());
    assert!(
        f.config
            .with_lock(|lock| lock.set(ConfigKey::StreamNaming, "inherit"))
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(f.config.config_path()).unwrap(),
        document
    );
}

#[test]
fn settings_symlink_updates_preserve_link_and_reject_broken_or_cyclic_links() {
    let f = Fixture::new();
    f.write("");
    let target = f.home.join("target.toml");
    fs::write(&target, "").unwrap();
    fs::remove_file(f.config.config_path()).unwrap();
    symlink("../../target.toml", f.config.config_path()).unwrap();
    assert_eq!(f.config.read_overrides().unwrap(), json!({}));
    f.config.set(ConfigKey::FrontendMode, "none").unwrap();
    assert_eq!(f.config.load().unwrap().frontend, FrontendMode::None);
    assert_eq!(
        fs::read_link(f.config.config_path()).unwrap(),
        Path::new("../../target.toml")
    );
    assert_eq!(
        toml::from_str::<toml::Table>(&fs::read_to_string(&target).unwrap()).unwrap(),
        toml::from_str::<toml::Table>("[frontend]\nmode = \"none\"\n").unwrap()
    );
    fs::remove_file(&target).unwrap();
    assert!(f.config.read_overrides().is_err());
    assert!(f.config.set(ConfigKey::FrontendMode, "auto").is_err());
    assert!(!target.exists());
    symlink(f.config.config_path(), &target).unwrap();
    assert!(f.config.read_overrides().is_err());
    assert!(f.config.set(ConfigKey::FrontendMode, "auto").is_err());
    assert!(target.is_symlink());
    assert!(f.config.config_path().is_symlink());
}

#[test]
fn host_and_lock_symlinks_fail_closed() {
    let f = Fixture::new();
    f.write("{}");
    let target = f.home.join("target.json");
    fs::write(&target, "{}").unwrap();
    fs::remove_file(f.config.config_path()).unwrap();
    fs::remove_file(&target).unwrap();
    symlink(&target, f.config.host_path()).unwrap();
    assert!(f.config.host_identity().is_err());
    assert!(
        f.config
            .with_lock(|guard| guard.host_identity_for_new_catalog())
            .is_err()
    );
    assert!(!target.exists());
    fs::write(&target, json!({"version":1,"id":HOST}).to_string()).unwrap();
    assert!(f.config.host_identity().is_err());
    assert!(
        f.config
            .with_lock(|guard| guard.host_identity_for_new_catalog())
            .is_err()
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        json!({"version":1,"id":HOST}).to_string()
    );
    let lock = f
        .config
        .config_path()
        .with_file_name(".catalog-config.lock");
    fs::remove_file(&lock).unwrap();
    symlink(&target, lock).unwrap();
    assert!(f.config.lock().is_err());
}

#[test]
fn host_requires_explicit_creation_is_private_and_stable() {
    let f = Fixture::new();
    assert!(f.config.host_identity().is_err());
    assert!(!f.config.host_path().exists());
    let id = f
        .config
        .with_lock(|guard| guard.host_identity_for_new_catalog())
        .unwrap();
    assert_eq!(HostIdentity::parse(id.as_str()).unwrap(), id);
    assert_eq!(
        fs::metadata(f.config.host_path())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    let before = fs::read(f.config.host_path()).unwrap();
    assert_eq!(f.config.host_identity().unwrap(), id);
    assert_eq!(
        f.config
            .with_lock(|guard| guard.host_identity_for_new_catalog())
            .unwrap(),
        id
    );
    f.config
        .with_lock(|lock| {
            assert_eq!(lock.settings()?.frontend, FrontendMode::Auto);
            lock.set(ConfigKey::FrontendMode, "herdr")?;
            Ok(())
        })
        .unwrap();
    assert_eq!(fs::read(f.config.host_path()).unwrap(), before);
}

#[test]
fn malformed_host_is_never_regenerated_and_uuid_is_exact() {
    let f = Fixture::new();
    for text in [
        "null".into(),
        "[]".into(),
        "not json".into(),
        json!({"version":2,"id":HOST}).to_string(),
        json!({"version":true,"id":HOST}).to_string(),
        format!(r#"{{"version":1.0,"id":"{HOST}"}}"#),
        json!({"version":1,"id":HOST,"extra":false}).to_string(),
        json!({"version":1,"id":HOST.to_uppercase()}).to_string(),
        json!({"version":1,"id":HOST.replace('-', "")}).to_string(),
        json!({"version":1,"id":"not uuid"}).to_string(),
        format!(r#"{{"version":1,"id":"{HOST}","id":"{HOST}"}}"#),
    ] {
        f.host(&text);
        assert!(f.config.host_identity().is_err(), "{text}");
        assert!(
            f.config
                .with_lock(|guard| guard.host_identity_for_new_catalog())
                .is_err(),
            "{text}"
        );
        assert_eq!(fs::read_to_string(f.config.host_path()).unwrap(), text);
    }
    f.host(&json!({"version":1,"id":HOST}).to_string());
    assert_eq!(f.config.host_identity().unwrap().as_str(), HOST);
}

#[test]
fn existing_catalog_marker_never_allows_missing_identity_creation() {
    let f = Fixture::new();
    let root = f.config.load().unwrap().catalog_root;
    let state = root.join(".catalog-sync");
    fs::create_dir_all(&state).unwrap();
    let marker = state.join("cli.json");
    for contents in [
        "not json".into(),
        json!({"version":1,"strategy":"none","host":HOST}).to_string(),
    ] {
        fs::write(&marker, contents).unwrap();
        assert!(
            f.config
                .with_lock(|guard| guard.host_identity_for_new_catalog())
                .is_err()
        );
        assert!(!f.config.host_path().exists());
    }
    fs::remove_file(&marker).unwrap();
    symlink(f.home.join("missing"), marker).unwrap();
    assert!(
        f.config
            .with_lock(|guard| guard.host_identity_for_new_catalog())
            .is_err()
    );
    assert!(!f.config.host_path().exists());
}

#[test]
fn catalog_machinery_symlinks_block_host_creation() {
    for name in [".catalog-sync", ".catalog.lock", ".catalog-staging"] {
        for existing in [false, true] {
            let f = Fixture::new();
            let root = f.config.load().unwrap().catalog_root;
            fs::create_dir_all(&root).unwrap();
            let target = f.home.join("target");
            if existing {
                fs::create_dir(&target).unwrap();
            }
            symlink(&target, root.join(name)).unwrap();
            assert!(
                f.config
                    .with_lock(|guard| guard.host_identity_for_new_catalog())
                    .is_err(),
                "{name}"
            );
            assert!(!f.config.host_path().exists());
        }
    }
}

#[test]
fn lock_rejects_fifo_and_directory_without_blocking() {
    for directory in [false, true] {
        let f = Fixture::new();
        f.write("{}");
        let lock = f
            .config
            .config_path()
            .with_file_name(".catalog-config.lock");
        if directory {
            fs::create_dir(&lock).unwrap();
        } else {
            let path = std::ffi::CString::new(lock.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        }
        assert!(f.config.lock().is_err());
    }
}

#[test]
fn concurrent_updates_and_host_creation_serialize() {
    let f = Fixture::new();
    let barrier = Arc::new(Barrier::new(4));
    let mut workers = Vec::new();
    for (key, value) in [
        (ConfigKey::FrontendMode, "herdr"),
        (ConfigKey::CatalogRoot, "~/catalog"),
        (ConfigKey::StreamNaming, "manual"),
        (ConfigKey::SessionNaming, "manual"),
    ] {
        let config = f.config.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            config.set(key, value).unwrap();
            config
                .with_lock(|guard| guard.host_identity_for_new_catalog())
                .unwrap()
        }));
    }
    let identities: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert!(identities.iter().all(|id| id == &identities[0]));
    assert_eq!(
        f.config.read_overrides().unwrap(),
        json!({"catalog":{"root":f.home.join("catalog")},
        "frontend":{"mode":"herdr"},"naming":{"stream":"manual","session":"manual"}})
    );
    assert_eq!(
        fs::read_dir(f.config.config_path().parent().unwrap())
            .unwrap()
            .count(),
        3
    );
}

#[test]
fn resolve_missing_paths_symlinks_and_runtime_identity() {
    let f = Fixture::new();
    fs::create_dir_all(f.home.join("actual/child")).unwrap();
    symlink(f.home.join("actual/child"), f.home.join("alias")).unwrap();
    let root = f.home.join("alias/../missing/leaf/../next");
    let resolved_home = fs::canonicalize(&f.home).unwrap();
    assert_eq!(
        resolve_path(&root).unwrap(),
        resolved_home.join("actual/missing/next")
    );
    f.config
        .set(ConfigKey::CatalogRoot, root.to_str().unwrap())
        .unwrap();
    let first = f
        .config
        .runtime_state_dir_for(&f.config.load().unwrap().catalog_root)
        .unwrap();
    f.config
        .set(
            ConfigKey::CatalogRoot,
            resolved_home.join("actual/missing/next").to_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.config
            .runtime_state_dir_for(&f.config.load().unwrap().catalog_root)
            .unwrap(),
        first
    );
    f.config.set(ConfigKey::FrontendMode, "none").unwrap();
    assert_eq!(
        f.config
            .runtime_state_dir_for(&f.config.load().unwrap().catalog_root)
            .unwrap(),
        first
    );
    symlink("broken-target", f.home.join("broken")).unwrap();
    assert_eq!(
        resolve_path(&f.home.join("broken/child")).unwrap(),
        resolved_home.join("broken-target/child")
    );
    symlink("loop", f.home.join("loop")).unwrap();
    assert!(resolve_path(&f.home.join("loop")).is_err());
    for index in 0..60 {
        symlink(
            if index == 59 {
                "actual".into()
            } else {
                format!("link{}", index + 1)
            },
            f.home.join(format!("link{index}")),
        )
        .unwrap();
    }
    assert_eq!(
        resolve_path(&f.home.join("link0/missing")).unwrap(),
        resolved_home.join("actual/missing")
    );
    let repeated = format!("{}alias", "alias/../../".repeat(60));
    assert_eq!(
        resolve_path(&f.home.join(repeated)).unwrap(),
        resolved_home.join("actual/child")
    );
}

#[test]
fn source_uses_host_and_canonical_path_not_directory_object_identity() {
    let f = Fixture::new();
    let directory = f.home.join("café-\u{1f680}-\u{7f}");
    fs::create_dir(&directory).unwrap();
    symlink(&directory, f.home.join("alias")).unwrap();
    let host = HostIdentity::parse(HOST).unwrap();
    let canonical = fs::canonicalize(&directory).unwrap();
    let path = canonical
        .to_str()
        .unwrap()
        .replace('é', "\\u00e9")
        .replace('\u{1f680}', "\\ud83d\\ude80")
        .replace('\u{7f}', "\\u007f");
    let preimage = format!("[\"{path}\", \"{HOST}\"]");
    let expected = format!("{:x}", Sha256::digest(preimage.as_bytes()));
    assert_eq!(source_id(&directory, &host).unwrap(), expected);
    assert_eq!(
        source_id(&f.home.join("alias/../alias"), &host).unwrap(),
        expected
    );
    assert_ne!(
        source_id(
            &directory,
            &HostIdentity::parse("11234567-89ab-4def-8123-456789abcdef").unwrap()
        )
        .unwrap(),
        expected
    );
    let retired = f.home.join("retired-catalog");
    fs::rename(&directory, &retired).unwrap();
    fs::create_dir(&directory).unwrap();
    assert_eq!(source_id(&directory, &host).unwrap(), expected);
    assert_ne!(source_id(&retired, &host).unwrap(), expected);
    fs::write(f.home.join("file"), "not a catalog").unwrap();
    assert!(source_id(&f.home.join("file"), &host).is_err());
    assert!(source_id(&f.home.join("missing"), &host).is_err());
}

#[test]
fn obsolete_bookkeeping_never_generates_a_host_identity() {
    for directory in [false, true] {
        let f = Fixture::new();
        let root = f.config.load().unwrap().catalog_root;
        let marker = root.join(".catalog-sync/cli.json");
        fs::create_dir_all(marker.parent().unwrap()).unwrap();
        if directory {
            fs::create_dir(&marker).unwrap();
        } else {
            let path = std::ffi::CString::new(marker.as_os_str().as_encoded_bytes()).unwrap();
            assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        }
        assert!(
            f.config
                .with_lock(|guard| guard.host_identity_for_new_catalog())
                .is_err()
        );
        assert!(!f.config.host_path().exists());
        assert!(!root.join(".catalog.lock").exists());
    }
}
