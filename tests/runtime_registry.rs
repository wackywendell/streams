//! Host-local behavior ported from test_catalog_checkouts.py and test_catalog_locations.py.

use std::fs;
use std::os::unix::fs::{MetadataExt, symlink};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, Mutex};

use serde_json::{Value, json};
use streams_temper::process::{ProcessError, ProcessOutput, ProcessRequest, ProcessRunner};
use streams_temper::runtime::checkouts::{canonical_directory, inspect_directory_with_runner};
use streams_temper::runtime::{
    CheckoutRegistry, DirectoryBindings, RepositoryInspection, RuntimeError,
};

fn fake_inspector(path: &Path) -> Result<Option<RepositoryInspection>, RuntimeError> {
    let current = canonical_directory(path)?;
    for root in current.ancestors() {
        if root.join(".fake-checkout").is_file() {
            let relative = current.strip_prefix(root).unwrap();
            return Ok(Some(RepositoryInspection {
                root: root.to_owned(),
                relative_cwd: if relative.as_os_str().is_empty() {
                    ".".into()
                } else {
                    relative.to_str().unwrap().into()
                },
            }));
        }
    }
    Ok(None)
}

struct Fixture {
    _temp: tempfile::TempDir,
    root: PathBuf,
    registry: CheckoutRegistry,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(temp.path()).unwrap();
        let registry = CheckoutRegistry::new(root.join("state"), "test-host")
            .unwrap()
            .with_inspector(fake_inspector);
        Self {
            _temp: temp,
            root,
            registry,
        }
    }
    fn repo(&self, name: &str) -> PathBuf {
        let root = self.root.join(name);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(".fake-checkout"), "").unwrap();
        root
    }
    fn bindings(&self) -> DirectoryBindings {
        DirectoryBindings::new(self.root.join("runtime"), self.registry.clone())
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.registry.path()).unwrap()).unwrap()
    }
    fn overwrite(&self, state: &Value) {
        fs::write(self.registry.path(), serde_json::to_vec(state).unwrap()).unwrap();
    }
}

fn legacy_binding_path(bindings: &DirectoryBindings, stream: &str, session: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let wire = format!(
        "[{}, {}]",
        streams_temper::codec::ascii_json(stream),
        streams_temper::codec::ascii_json(session)
    );
    bindings
        .path(stream, None)
        .with_file_name(format!("{:x}.json", Sha256::digest(wire.as_bytes())))
}

#[test]
fn reads_create_nothing_and_host_scope_is_exact() {
    let f = Fixture::new();
    let root = f.repo("repo");
    assert!(f.registry.list().unwrap().is_empty());
    assert!(f.registry.find(&root).unwrap().is_none());
    assert!(
        f.registry
            .get("b9345696-ffbf-42d1-8255-6fdc9d2ab7d2")
            .is_err()
    );
    assert!(f.registry.resolve("repo", None, None).is_err());
    assert!(matches!(
        f.registry.get("missing"),
        Err(RuntimeError::NotFound(_))
    ));
    for invalid_id in ["", " ", "bad\x7f"] {
        assert!(matches!(
            f.registry.get(invalid_id),
            Err(RuntimeError::Invalid(_))
        ));
    }
    assert!(!f.registry.path().parent().unwrap().exists());
    let checkout = f.registry.register(&root, None).unwrap();
    let same = CheckoutRegistry::new(f.root.join("state"), "test-host")
        .unwrap()
        .with_inspector(fake_inspector);
    assert_eq!(same.get(&checkout.id).unwrap(), checkout);
    let other = CheckoutRegistry::new(f.root.join("state"), "other-host").unwrap();
    assert!(other.list().unwrap().is_empty());
    assert_ne!(same.path(), other.path());
    assert!(CheckoutRegistry::new(f.root.join("state"), "bad\x7f").is_err());
}

#[test]
fn aliases_reuse_uuid_and_urls_remain_exact_explicit_bindings() {
    let f = Fixture::new();
    let root = f.repo("root with trailing space ");
    let first = f.registry.register(&root, None).unwrap();
    uuid::Uuid::parse_str(&first.id).unwrap();
    assert!(first.repository_urls.is_empty());
    let alias = f.root.join("alias");
    symlink(&root, &alias).unwrap();
    assert_eq!(f.registry.register(&alias, None).unwrap(), first);
    assert_eq!(f.registry.find(&alias).unwrap(), Some(first.clone()));
    let checkout = f
        .registry
        .register(&root, Some("https://example/repo"))
        .unwrap();
    assert_eq!(checkout.id, first.id);
    assert_eq!(
        f.registry
            .register(&root, Some("https://example/repo"))
            .unwrap(),
        checkout
    );
    assert!(
        f.registry
            .resolve("https://example/repo", None, None)
            .is_err()
    );
    assert!(
        f.registry
            .set_default("https://example/repo.git", &first.id)
            .is_err()
    );
    f.registry
        .set_default("https://example/repo", &first.id)
        .unwrap();
    assert_eq!(
        f.registry
            .resolve("https://example/repo", None, None)
            .unwrap(),
        checkout
    );
}

#[test]
fn canonical_directory_preserves_nested_spelling_and_symlink_identity() {
    let f = Fixture::new();
    let outer = f.root.join("MiXeD-É");
    let inner = outer.join("Inner");
    fs::create_dir_all(&inner).unwrap();
    let alias = f.root.join("alias");
    symlink(&outer, &alias).unwrap();
    let identity = fs::metadata(&outer).unwrap();
    let stored_outer = fs::read_dir(&f.root)
        .unwrap()
        .map(|entry| entry.unwrap())
        .find(|entry| {
            let metadata = entry.metadata().unwrap();
            (metadata.dev(), metadata.ino()) == (identity.dev(), identity.ino())
        })
        .unwrap()
        .path();
    assert_eq!(
        canonical_directory(&alias.join("Inner")).unwrap(),
        stored_outer.join("Inner")
    );
    let long = inner.join("x".repeat(255));
    fs::create_dir(&long).unwrap();
    assert_eq!(
        canonical_directory(&long).unwrap(),
        stored_outer.join("Inner").join("x".repeat(255))
    );
    assert!(canonical_directory(&inner.join("missing")).is_err());
    fs::write(inner.join("file"), "").unwrap();
    assert!(canonical_directory(&inner.join("file")).is_err());
    assert_eq!(canonical_directory(Path::new("/")).unwrap(), Path::new("/"));
}

#[test]
fn case_aliases_use_actual_directory_entry_spelling_when_supported() {
    let f = Fixture::new();
    let root = f.repo("mixedcase");
    let alias = root.with_file_name("MIXEDCASE");
    if !alias.exists() {
        return;
    }
    let first = f.registry.register(&root, None).unwrap();
    symlink(f.root.join("missing"), f.root.join("broken-sibling")).unwrap();
    symlink(&root, f.root.join("resolving-sibling")).unwrap();
    assert_eq!(f.registry.register(&alias, None).unwrap(), first);
    assert_eq!(canonical_directory(&alias).unwrap(), root);
}

#[test]
fn explicit_move_preserves_identity_bindings_default_and_revalidates_selection() {
    let f = Fixture::new();
    let root = f.repo("first");
    fs::create_dir(root.join("src")).unwrap();
    let checkout = f.registry.register(&root, Some("repo")).unwrap();
    f.registry.set_default("repo", &checkout.id).unwrap();
    let bindings = f.bindings();
    bindings
        .remember(
            "stream",
            &root.join("src"),
            Some("session"),
            false,
            Some("repo"),
            None,
        )
        .unwrap();
    let moved = f.root.join("moved");
    fs::rename(&root, &moved).unwrap();
    assert!(f.registry.register(&moved, None).is_err());
    assert!(bindings.read("stream", Some("session")).is_err());
    let updated = f.registry.move_checkout(&checkout.id, &moved).unwrap();
    assert_eq!(updated.id, checkout.id);
    assert_eq!(updated.repository_urls, checkout.repository_urls);
    assert_eq!(f.registry.resolve("repo", None, None).unwrap(), updated);
    assert_eq!(
        bindings
            .read("stream", Some("session"))
            .unwrap()
            .unwrap()
            .cwd,
        moved.join("src")
    );
    let other = f.registry.register(&f.repo("other"), None).unwrap();
    assert!(
        f.registry
            .move_checkout(&checkout.id, &other.directory)
            .is_err()
    );
    let fresh = f.repo("fresh");
    f.registry.move_checkout(&checkout.id, &fresh).unwrap();
    assert!(moved.is_dir());
    assert!(bindings.read("stream", Some("session")).is_err());
}

#[test]
fn replaced_or_missing_selected_roots_never_use_healthy_default() {
    let f = Fixture::new();
    let root = f.repo("first");
    let selected = f.registry.register(&root, Some("repo")).unwrap();
    let fallback = f
        .registry
        .register(&f.repo("fallback"), Some("repo"))
        .unwrap();
    f.registry.set_default("repo", &fallback.id).unwrap();
    fs::rename(&root, f.root.join("old")).unwrap();
    assert!(
        f.registry
            .resolve("repo", Some(&selected.id), Some("repo"))
            .is_err()
    );
    f.repo("first");
    for error in [
        f.registry.get(&selected.id).unwrap_err(),
        f.registry.find(&root).unwrap_err(),
        f.registry.register(&root, None).unwrap_err(),
        f.registry.set_default("repo", &selected.id).unwrap_err(),
    ] {
        assert!(error.to_string().contains("replaced"), "{error}");
    }
    assert_eq!(f.registry.resolve("repo", None, None).unwrap(), fallback);
}

#[test]
fn documents_and_working_directories_reject_traversal_and_symlink_escapes() {
    let f = Fixture::new();
    let root = f.repo("checkout");
    let checkout = f.registry.register(&root, Some("repo")).unwrap();
    f.registry.set_default("repo", &checkout.id).unwrap();
    fs::write(root.join("doc.md"), "current uncommitted contents").unwrap();
    fs::write(f.root.join("outside.md"), "outside").unwrap();
    symlink(f.root.join("outside.md"), root.join("escape.md")).unwrap();
    symlink(&f.root, root.join("escape-dir")).unwrap();
    symlink(root.join("doc.md"), root.join("inside.md")).unwrap();
    assert_eq!(
        f.registry
            .document("repo", "inside.md", None, None)
            .unwrap()
            .1,
        root.join("doc.md")
    );
    for path in [
        "",
        ".",
        "..",
        "../outside.md",
        "/outside.md",
        "a/../doc.md",
        "a//b",
        "a/./b",
        "C:/doc.md",
        "a\\b",
        "doc.md/",
        "escape.md",
        "escape-dir/outside.md",
        "missing.md",
    ] {
        assert!(
            f.registry.document("repo", path, None, None).is_err(),
            "accepted {path:?}"
        );
    }
    for path in ["../", "/", "escape-dir", "doc.md", "a/../b"] {
        assert!(
            f.registry.directory(&checkout.id, path).is_err(),
            "accepted {path:?}"
        );
    }
    assert_eq!(f.registry.directory(&checkout.id, ".").unwrap(), root);
    let before = fs::read(f.registry.path()).unwrap();
    f.registry.document("repo", "doc.md", None, None).unwrap();
    assert_eq!(fs::read(f.registry.path()).unwrap(), before);
}

#[test]
fn resolved_documents_keep_one_snapshot_without_reinspecting_or_rereading_registry() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static INSPECTIONS: AtomicUsize = AtomicUsize::new(0);
    fn counting_inspector(path: &Path) -> Result<Option<RepositoryInspection>, RuntimeError> {
        INSPECTIONS.fetch_add(1, Ordering::SeqCst);
        fake_inspector(path)
    }
    let f = Fixture::new();
    let root = f.repo("checkout");
    let first = f.registry.register(&root, Some("repo")).unwrap();
    let other = f.registry.register(&f.repo("other"), Some("repo")).unwrap();
    f.registry.set_default("repo", &first.id).unwrap();
    fs::write(root.join("doc.md"), "current").unwrap();
    fs::write(f.root.join("outside.md"), "outside").unwrap();
    symlink(root.join("doc.md"), root.join("inside.md")).unwrap();
    symlink(f.root.join("outside.md"), root.join("escape.md")).unwrap();
    fs::create_dir(root.join("directory")).unwrap();
    let registry = f.registry.clone().with_inspector(counting_inspector);
    let resolved = registry.resolve("repo", None, None).unwrap();
    assert_eq!(INSPECTIONS.load(Ordering::SeqCst), 1);
    f.registry.set_default("repo", &other.id).unwrap();
    assert_eq!(resolved.document("doc.md").unwrap(), root.join("doc.md"));
    assert_eq!(resolved.document("inside.md").unwrap(), root.join("doc.md"));
    for path in ["escape.md", "missing.md", "directory", "../outside.md", "."] {
        assert!(resolved.document(path).is_err(), "accepted {path}");
    }
    fs::write(f.registry.path(), "no longer readable JSON").unwrap();
    assert_eq!(resolved.document("doc.md").unwrap(), root.join("doc.md"));
    assert_eq!(INSPECTIONS.load(Ordering::SeqCst), 1);
    fs::rename(&root, f.root.join("old-checkout")).unwrap();
    let replacement = f.repo("checkout");
    fs::write(replacement.join("doc.md"), "replacement").unwrap();
    let error = resolved.document("doc.md").unwrap_err();
    assert!(error.to_string().contains("replaced"), "{error}");
}

#[test]
fn canonical_directory_does_not_expand_literal_tilde() {
    if std::env::var_os("STREAMS_LITERAL_TILDE_CHILD").is_some() {
        let cwd = fs::canonicalize(".").unwrap();
        assert_eq!(canonical_directory(Path::new("~")).unwrap(), cwd.join("~"));
        assert_eq!(
            canonical_directory(Path::new("~/child")).unwrap(),
            cwd.join("~/child")
        );
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("~")).unwrap();
    fs::create_dir(temp.path().join("~").join("child")).unwrap();
    fs::create_dir(temp.path().join("home")).unwrap();
    // Isolate the cwd and HOME in a child, without mutating this test process's environment.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "canonical_directory_does_not_expand_literal_tilde",
        ])
        .current_dir(temp.path())
        .env("HOME", temp.path().join("home"))
        .env("STREAMS_LITERAL_TILDE_CHILD", "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn malformed_registry_is_never_absence_or_overwritten() {
    let f = Fixture::new();
    let root = f.repo("checkout");
    f.registry.register(&root, Some("repo")).unwrap();
    let mut invalid_host = f.state();
    invalid_host["host_id"] = json!("other-host");
    for bytes in [
        b"not JSON".to_vec(),
        serde_json::to_vec(&invalid_host).unwrap(),
    ] {
        fs::write(f.registry.path(), &bytes).unwrap();
        assert!(f.registry.list().is_err());
        assert!(f.registry.register(&root, None).is_err());
        assert_eq!(fs::read(f.registry.path()).unwrap(), bytes);
    }
}

#[test]
fn duplicate_live_physical_aliases_are_rejected_even_with_different_recorded_inodes() {
    let f = Fixture::new();
    let root = f.repo("root");
    f.registry.register(&root, None).unwrap();
    let alias = f.root.join("alias");
    symlink(root, &alias).unwrap();
    let mut state = f.state();
    let mut other = state["checkouts"][0].clone();
    other["id"] = json!(uuid::Uuid::new_v4().to_string());
    other["directory"] = json!(alias);
    other["inode"] = json!(1);
    state["checkouts"].as_array_mut().unwrap().push(other);
    f.overwrite(&state);
    assert!(f.registry.list().is_err());
}

#[test]
fn registry_state_lock_and_directory_binding_symlinks_are_refused() {
    let f = Fixture::new();
    let root = f.repo("repo");
    fs::create_dir_all(f.registry.path().parent().unwrap()).unwrap();
    let target = f.root.join("target");
    fs::write(&target, "sentinel").unwrap();
    symlink(&target, f.registry.path()).unwrap();
    assert!(f.registry.list().is_err());
    assert!(f.registry.register(&root, None).is_err());
    fs::remove_file(f.registry.path()).unwrap();
    let lock = f.registry.path().with_extension("lock");
    fs::remove_file(&lock).unwrap();
    symlink(&target, &lock).unwrap();
    assert!(f.registry.register(&root, None).is_err());
    let bindings = f.bindings();
    let path = bindings.path("stream", Some("session"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(&target, &path).unwrap();
    assert!(bindings.read("stream", Some("session")).is_err());
    assert!(
        bindings
            .remember("stream", &f.root, Some("session"), false, None, None)
            .is_err()
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "sentinel");
}

#[test]
fn nonregular_registry_and_lock_files_are_rejected_without_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let root = f.repo("repo");
    fs::create_dir_all(f.registry.path().parent().unwrap()).unwrap();
    fs::create_dir(f.registry.path()).unwrap();
    assert!(f.registry.list().is_err());
    assert!(f.registry.register(&root, None).is_err());
    fs::remove_dir(f.registry.path()).unwrap();
    let path = CString::new(f.registry.path().as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(f.registry.list().is_err());
    fs::remove_file(f.registry.path()).unwrap();
    let lock = f.registry.path().with_extension("lock");
    fs::remove_file(&lock).unwrap();
    let path = CString::new(lock.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
    assert!(f.registry.register(&root, None).is_err());
}

#[test]
fn nonregular_directory_bindings_and_binding_locks_are_rejected_without_blocking() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let bindings = f.bindings();
    let path = bindings.path("stream", None);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(bindings.read("stream", None).is_err());
    assert!(
        bindings
            .remember("stream", &f.root, None, false, None, None)
            .is_err()
    );
    fs::remove_dir(&path).unwrap();
    let fifo = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(bindings.read("stream", None).is_err());
    assert!(
        bindings
            .remember("stream", &f.root, None, false, None, None)
            .is_err()
    );
    fs::remove_file(&path).unwrap();
    let lock = path.with_extension("lock");
    fs::remove_file(&lock).unwrap();
    let fifo = CString::new(lock.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(
        bindings
            .remember("stream", &f.root, None, false, None, None)
            .is_err()
    );
    fs::remove_file(&lock).unwrap();
    let target = f.root.join("sentinel");
    fs::write(&target, "unchanged").unwrap();
    symlink(&target, &lock).unwrap();
    assert!(
        bindings
            .remember("stream", &f.root, None, false, None, None)
            .is_err()
    );
    assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");
    assert!(!path.exists());
}

#[test]
fn concurrent_registry_updates_preserve_every_registration() {
    let f = Fixture::new();
    let barrier = Arc::new(Barrier::new(8));
    let roots: Vec<_> = (0..8)
        .map(|index| f.repo(&format!("root-{index}")))
        .collect();
    let threads: Vec<_> = roots
        .into_iter()
        .map(|root| {
            let registry = f.registry.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                registry.register(&root, Some("repo")).unwrap()
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert_eq!(f.registry.list().unwrap().len(), 8);
}

#[test]
fn stream_and_session_directory_identity_and_if_missing_are_independent() {
    let f = Fixture::new();
    let bindings = f.bindings();
    let first = f.repo("first");
    let second = f.repo("second");
    let stream = bindings
        .remember("stream", &first, None, false, Some("repo"), None)
        .unwrap();
    assert!(bindings.read("stream", Some("session")).unwrap().is_none());
    let session = bindings
        .remember(
            "stream",
            &second,
            Some("session"),
            false,
            Some("repo"),
            None,
        )
        .unwrap();
    assert_ne!(stream.checkout, session.checkout);
    let path = bindings.path("stream", Some("session"));
    let before = fs::read(&path).unwrap();
    assert_eq!(
        bindings
            .remember("stream", &first, Some("session"), true, None, None)
            .unwrap(),
        session
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(bindings.read("stream", None).unwrap(), Some(stream));
    assert_eq!(
        bindings.read("other-stream", Some("session")).unwrap(),
        Some(session)
    );
    assert!(bindings.read("other-stream", None).unwrap().is_none());
    assert!(
        bindings
            .read("stream", Some("other-session"))
            .unwrap()
            .is_none()
    );
    let other = bindings
        .remember(
            "other-stream",
            &first,
            Some("other-session"),
            false,
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        bindings.read("stream", Some("other-session")).unwrap(),
        Some(other)
    );
    assert_eq!(
        bindings
            .read("other-stream", Some("session"))
            .unwrap()
            .unwrap()
            .cwd,
        second
    );
    bindings
        .remember("other-stream", &second, None, false, None, None)
        .unwrap();
    assert_eq!(bindings.read("stream", None).unwrap().unwrap().cwd, first);
}

#[test]
fn missing_binding_targets_can_be_explicitly_repaired_but_not_with_if_missing() {
    let f = Fixture::new();
    let bindings = f.bindings();
    let old = f.root.join("old");
    fs::create_dir(&old).unwrap();
    bindings
        .remember("stream", &old, Some("session"), false, None, None)
        .unwrap();
    fs::remove_dir(old).unwrap();
    let path = bindings.path("stream", Some("session"));
    let before = fs::read(&path).unwrap();
    assert!(bindings.read("stream", Some("session")).is_err());
    assert!(
        bindings
            .remember("stream", &f.root, Some("session"), true, None, None)
            .is_err()
    );
    assert_eq!(fs::read(&path).unwrap(), before);
    bindings
        .remember("stream", &f.root, Some("session"), false, None, None)
        .unwrap();
    assert_eq!(
        bindings
            .read("stream", Some("session"))
            .unwrap()
            .unwrap()
            .cwd,
        f.root
    );
}

#[test]
fn concurrent_if_missing_selection_keeps_the_first_locked_choice() {
    let f = Fixture::new();
    let first = f.repo("first");
    let second = f.repo("second");
    let bindings = f.bindings();
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [first, second]
        .into_iter()
        .enumerate()
        .map(|(index, cwd)| {
            let bindings = bindings.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                bindings
                    .remember(
                        &format!("stream-{index}"),
                        &cwd,
                        Some("session"),
                        true,
                        None,
                        None,
                    )
                    .unwrap()
            })
        })
        .collect();
    let results: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    assert_eq!(results[0], results[1]);
    assert_eq!(f.registry.list().unwrap().len(), 1);
}

#[test]
fn directory_selections_never_infer_checkout_identity_or_rewrite_on_read() {
    let f = Fixture::new();
    let root = f.repo("repo");
    let bindings = f.bindings();
    let path = bindings.path("stream", Some("session"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let current = json!({"version":3,"session_id":"session",
        "location":{"kind":"directory","cwd":root}});
    fs::write(&path, serde_json::to_vec(&current).unwrap()).unwrap();
    let before = fs::read(&path).unwrap();
    assert!(
        bindings
            .read("stream", Some("session"))
            .unwrap()
            .unwrap()
            .checkout
            .is_none()
    );
    assert!(!f.registry.path().exists());
    f.registry.register(&root, None).unwrap();
    let registry_before = fs::read(f.registry.path()).unwrap();
    let selected = bindings.read("stream", Some("session")).unwrap().unwrap();
    assert_eq!(selected.cwd, root);
    assert!(selected.checkout.is_none());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert_eq!(fs::read(f.registry.path()).unwrap(), registry_before);
}

#[test]
fn malformed_bindings_cannot_be_read_or_overwritten() {
    let f = Fixture::new();
    let bindings = f.bindings();
    let root = f.repo("repo");
    bindings
        .remember("stream", &root, Some("session"), false, None, None)
        .unwrap();
    let path = bindings.path("stream", Some("session"));
    let good: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut version = good.clone();
    version["version"] = json!(1);
    let mut escaping = good;
    escaping["location"]["within"] = json!("../escape");
    let duplicate = format!(
        r#"{{"version":3,"version":3,"session_id":"session","location":{{"kind":"directory","cwd":{}}}}}"#,
        serde_json::to_string(&f.root).unwrap()
    );
    for bytes in [
        serde_json::to_vec(&version).unwrap(),
        serde_json::to_vec(&escaping).unwrap(),
        duplicate.into_bytes(),
    ] {
        fs::write(&path, &bytes).unwrap();
        assert!(bindings.read("stream", Some("session")).is_err());
        assert!(
            bindings
                .remember("stream", &f.root, Some("session"), false, None, None)
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn malformed_current_binding_reports_the_missing_location_field() {
    let f = Fixture::new();
    let bindings = f.bindings();
    let path = bindings.path("stream", None);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, br#"{"version":2,"stream_id":"stream","session_id":null,"location":{"kind":"checkout","id":"saved-id","repository_url":null}}"#).unwrap();
    let error = bindings.read("stream", None).unwrap_err().to_string();
    assert!(error.contains("within"), "{error}");
    assert!(!error.contains("untagged"), "{error}");
}

#[test]
fn explicit_checkout_must_contain_cwd_and_have_requested_binding() {
    let f = Fixture::new();
    let bindings = f.bindings();
    let root = f.repo("first");
    let other = f.repo("other");
    let checkout = f.registry.register(&root, Some("repo")).unwrap();
    assert!(
        bindings
            .remember(
                "stream",
                &other,
                Some("session"),
                false,
                None,
                Some(&checkout.id)
            )
            .is_err()
    );
    assert!(
        bindings
            .remember(
                "stream",
                &root,
                Some("session"),
                false,
                Some("other-repo"),
                Some(&checkout.id)
            )
            .is_err()
    );
    assert!(
        bindings
            .remember(
                "stream",
                &f.root,
                Some("session"),
                false,
                Some("repo"),
                None
            )
            .is_err()
    );
    let binding = bindings
        .remember(
            "stream",
            &root,
            Some("session"),
            false,
            Some("repo"),
            Some(&checkout.id),
        )
        .unwrap();
    assert_eq!(
        binding.checkout.unwrap().repository_url.as_deref(),
        Some("repo")
    );
}

#[test]
fn python_wire_fixtures_remain_readable_without_rewriting_or_registering() {
    let f = Fixture::new();
    fs::create_dir_all(f.registry.path().parent().unwrap()).unwrap();
    let registry_fixture = include_bytes!("fixtures/runtime/registry-v1.json");
    fs::write(f.registry.path(), registry_fixture).unwrap();
    let checkout = f.registry.list().unwrap().pop().unwrap();
    assert_eq!(checkout.id, "b9345696-ffbf-42d1-8255-6fdc9d2ab7d2");
    assert_eq!(checkout.repository_urls, ["https://example.test/repo.git"]);
    assert_eq!(fs::read(f.registry.path()).unwrap(), registry_fixture);
    let bindings = f.bindings();
    let path = legacy_binding_path(&bindings, "stream", "session");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let root = f.repo("checkout");
    let metadata = fs::metadata(&root).unwrap();
    let mut state = f.state();
    state["checkouts"][0]["directory"] = json!(root);
    state["checkouts"][0]["device"] = json!(metadata.dev());
    state["checkouts"][0]["inode"] = json!(metadata.ino());
    f.overwrite(&state);
    fs::write(&path, include_bytes!("fixtures/runtime/directory-v2.json")).unwrap();
    assert_eq!(
        bindings
            .read("stream", Some("session"))
            .unwrap()
            .unwrap()
            .cwd,
        root
    );
    assert_eq!(
        fs::read(&path).unwrap(),
        include_bytes!("fixtures/runtime/directory-v2.json")
    );
}

#[test]
fn non_uuid_registry_ids_load_in_current_directory_bindings() {
    let f = Fixture::new();
    let root = f.repo("imported");
    let metadata = fs::metadata(&root).unwrap();
    let fixture = include_str!("fixtures/runtime/registry-v1.json")
        .replace("b9345696-ffbf-42d1-8255-6fdc9d2ab7d2", "legacy-checkout-1");
    let mut registry: Value = serde_json::from_str(&fixture).unwrap();
    registry["checkouts"][0]["directory"] = json!(root);
    registry["checkouts"][0]["device"] = json!(metadata.dev());
    registry["checkouts"][0]["inode"] = json!(metadata.ino());
    fs::create_dir_all(f.registry.path().parent().unwrap()).unwrap();
    f.overwrite(&registry);
    let before = fs::read(f.registry.path()).unwrap();
    let checkout = f.registry.get("legacy-checkout-1").unwrap();
    assert_eq!(checkout.id, "legacy-checkout-1");
    assert_eq!(
        f.registry.list().unwrap().as_slice(),
        std::slice::from_ref(&checkout)
    );
    assert_eq!(
        f.registry
            .resolve("https://example.test/repo.git", None, None)
            .unwrap(),
        checkout
    );
    assert!(matches!(
        f.registry.get("missing"),
        Err(RuntimeError::NotFound(_))
    ));
    let bindings = f.bindings();
    let path = legacy_binding_path(&bindings, "stream", "session");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let current = include_str!("fixtures/runtime/directory-v2.json")
        .replace("b9345696-ffbf-42d1-8255-6fdc9d2ab7d2", "legacy-checkout-1");
    fs::write(&path, &current).unwrap();
    let binding = bindings.read("stream", Some("session")).unwrap().unwrap();
    assert_eq!(binding.cwd, root);
    let bound = binding.checkout.unwrap();
    assert_eq!(bound.id, "legacy-checkout-1");
    assert_eq!(bound.relative_cwd, ".");
    assert!(bound.repository_url.is_none());
    assert_eq!(fs::read_to_string(&path).unwrap(), current);
    assert_eq!(fs::read(f.registry.path()).unwrap(), before);
    assert_eq!(f.registry.register(&root, None).unwrap(), checkout);
}

#[test]
fn invalid_inferred_relative_cwd_never_creates_or_changes_registry_state() {
    for invalid_name in ["bad\\child", "scheme:child"] {
        let f = Fixture::new();
        let root = f.repo("unregistered");
        let cwd = root.join(invalid_name);
        fs::create_dir(&cwd).unwrap();
        let bindings = f.bindings();
        assert!(
            bindings
                .remember("stream", &cwd, Some("session"), false, Some("repo"), None)
                .is_err()
        );
        assert!(!f.registry.path().exists());
        assert!(!bindings.path("stream", Some("session")).exists());
        let existing = f.repo("existing");
        f.registry
            .register(&existing, Some("existing-repo"))
            .unwrap();
        let before = fs::read(f.registry.path()).unwrap();
        assert!(
            bindings
                .remember("stream", &cwd, Some("session"), false, Some("repo"), None)
                .is_err()
        );
        assert_eq!(fs::read(f.registry.path()).unwrap(), before);
    }
}

#[test]
fn directory_hash_keeps_stream_tuple_compatibility_and_uses_session_identity() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new();
    let bindings = f.bindings();
    for (stream, session, wire) in [
        ("stream", Some("session"), "session"),
        ("雪😀", None, "[\"\\u96ea\\ud83d\\ude00\", null]"),
        ("quoted\"", Some("s\\"), "s\\"),
    ] {
        let expected = format!("{:x}.json", Sha256::digest(wire.as_bytes()));
        assert_eq!(
            bindings.path(stream, session).file_name().unwrap(),
            expected.as_str()
        );
    }
}

#[test]
fn legacy_session_selection_survives_membership_move_without_rewrite_and_promotes_on_update() {
    let f = Fixture::new();
    let bindings = f.bindings();
    let path = legacy_binding_path(&bindings, "old-stream", "session");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let bytes = serde_json::to_vec(&json!({
        "version": 2, "stream_id": "old-stream", "session_id": "session",
        "location": {"kind": "directory", "cwd": f.root}
    }))
    .unwrap();
    fs::write(&path, &bytes).unwrap();
    let before = bindings
        .read("old-stream", Some("session"))
        .unwrap()
        .unwrap();
    assert_eq!(
        bindings.read("new-stream", Some("session")).unwrap(),
        Some(before.clone())
    );
    assert_eq!(fs::read(&path).unwrap(), bytes);
    let canonical = bindings.path("new-stream", Some("session"));
    assert!(!canonical.exists());
    assert!(!canonical.parent().unwrap().exists());
    let updated = f.root.join("updated");
    fs::create_dir(&updated).unwrap();
    bindings
        .remember("new-stream", &updated, Some("session"), false, None, None)
        .unwrap();
    assert!(!path.exists());
    let saved: Value = serde_json::from_slice(&fs::read(&canonical).unwrap()).unwrap();
    assert_eq!(saved["version"], 3);
    assert!(saved.get("stream_id").is_none());
    assert_eq!(
        bindings
            .read("old-stream", Some("session"))
            .unwrap()
            .unwrap()
            .cwd,
        updated
    );

    // A process interrupted after rename must still expose the original v2 selection.
    fs::write(&canonical, &bytes).unwrap();
    assert_eq!(
        bindings.read("new-stream", Some("session")).unwrap(),
        Some(before)
    );
}

#[test]
fn ambiguous_legacy_sessions_fail_closed_but_canonical_selection_is_authoritative() {
    let f = Fixture::new();
    let bindings = f.bindings();
    for stream in ["first", "second"] {
        let path = legacy_binding_path(&bindings, stream, "session");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "version": 2, "stream_id": stream, "session_id": "session",
                "location": {"kind": "directory", "cwd": f.root}
            }))
            .unwrap(),
        )
        .unwrap();
    }
    assert!(bindings.read("third", Some("session")).is_err());
    assert!(
        bindings
            .remember("third", &f.root, Some("session"), false, None, None)
            .is_err()
    );
    let canonical = bindings.path("third", Some("session"));
    fs::create_dir_all(canonical.parent().unwrap()).unwrap();
    fs::write(
        &canonical,
        serde_json::to_vec(&json!({
            "version": 3, "session_id": "session",
            "location": {"kind": "directory", "cwd": f.root}
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        bindings
            .read("third", Some("session"))
            .unwrap()
            .unwrap()
            .cwd,
        f.root
    );
    fs::write(&canonical, "malformed").unwrap();
    assert!(bindings.read("third", Some("session")).is_err());
}

#[test]
fn canonical_session_special_files_and_legacy_exact_identity_are_not_absence() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let bindings = f.bindings();
    let canonical = bindings.path("source", Some("session"));
    fs::create_dir_all(canonical.parent().unwrap()).unwrap();
    let fifo = CString::new(canonical.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(bindings.read("destination", Some("session")).is_err());
    assert!(
        bindings
            .remember("destination", &f.root, Some("session"), false, None, None)
            .is_err()
    );
    fs::remove_file(&canonical).unwrap();
    let path = legacy_binding_path(&bindings, "source", "session");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "version": 2, "stream_id": "source", "session_id": "wrong-session",
            "location": {"kind": "directory", "cwd": f.root}
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(bindings.read("source", Some("session")).is_err());
    assert!(
        bindings
            .remember("source", &f.root, Some("session"), false, None, None)
            .is_err()
    );
}

#[test]
fn legacy_session_filename_must_match_its_stored_tuple() {
    let f = Fixture::new();
    let bindings = f.bindings();
    let path = legacy_binding_path(&bindings, "wrong-stream", "session");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &path,
        serde_json::to_vec(&json!({
            "version": 2, "stream_id": "actual-stream", "session_id": "session",
            "location": {"kind": "directory", "cwd": f.root}
        }))
        .unwrap(),
    )
    .unwrap();
    assert!(bindings.read("destination", Some("session")).is_err());
    assert!(!bindings.path("destination", Some("session")).exists());
}

#[test]
fn unreadable_legacy_identity_is_not_absence_after_a_move() {
    let f = Fixture::new();
    let bindings = f.bindings();
    let path = legacy_binding_path(&bindings, "source", "session");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"broken legacy record").unwrap();
    assert!(bindings.read("destination", Some("session")).is_err());
    assert!(
        bindings
            .remember("destination", &f.root, Some("session"), true, None, None)
            .is_err()
    );
    assert!(!bindings.path("destination", Some("session")).exists());
    assert_eq!(fs::read(&path).unwrap(), b"broken legacy record");
}

#[derive(Default)]
struct FakeRunner {
    calls: Mutex<Vec<ProcessRequest>>,
    root: Mutex<Option<PathBuf>>,
    fail: bool,
}
impl ProcessRunner for FakeRunner {
    fn run(&self, request: &ProcessRequest) -> Result<ProcessOutput, ProcessError> {
        self.calls.lock().unwrap().push(request.clone());
        if self.fail {
            return Err(ProcessError {
                message: "timeout".into(),
                code: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
            });
        }
        let root = self
            .root
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| request.cwd.clone().unwrap());
        Ok(ProcessOutput {
            status: std::process::ExitStatus::from_raw(0),
            stdout: format!("{}\n", root.display()).into_bytes(),
            stderr: Vec::new(),
        })
    }
}

#[test]
fn production_inspection_uses_bounded_readonly_git_and_jj_and_preserves_root_spaces() {
    let f = Fixture::new();
    let root = f.repo("trailing space ");
    fs::create_dir(root.join(".git")).unwrap();
    fs::create_dir(root.join("src")).unwrap();
    let runner = FakeRunner::default();
    assert_eq!(
        inspect_directory_with_runner(&root.join("src"), &runner).unwrap(),
        Some(RepositoryInspection {
            root: root.clone(),
            relative_cwd: "src".into()
        })
    );
    let git = runner.calls.lock().unwrap()[0].clone();
    assert_eq!(git.program, Path::new("git"));
    assert_eq!(git.timeout, 15.0);
    assert_eq!(git.allowed, [0]);
    assert_eq!(
        git.args,
        ["rev-parse", "--path-format=absolute", "--show-toplevel"]
    );
    assert!(
        git.env
            .keys()
            .filter(|key| key.to_string_lossy().starts_with("GIT_"))
            .all(|key| key == "GIT_OPTIONAL_LOCKS")
    );
    fs::create_dir(root.join(".jj")).unwrap();
    inspect_directory_with_runner(&root, &runner).unwrap();
    let jj = runner.calls.lock().unwrap().last().unwrap().clone();
    assert_eq!(jj.program, Path::new("jj"));
    assert_eq!(
        jj.args,
        [
            "--ignore-working-copy",
            "--at-operation=@",
            "workspace",
            "root"
        ]
    );
    let failed = FakeRunner {
        fail: true,
        ..Default::default()
    };
    assert!(inspect_directory_with_runner(&root, &failed).is_err());
}

#[test]
fn malformed_nearest_marker_and_mismatched_reported_root_do_not_fall_back() {
    let f = Fixture::new();
    let root = f.repo("root");
    fs::create_dir(root.join(".git")).unwrap();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join(".jj"), "invalid").unwrap();
    let runner = FakeRunner::default();
    assert!(inspect_directory_with_runner(&nested, &runner).is_err());
    assert!(runner.calls.lock().unwrap().is_empty());
    fs::remove_file(nested.join(".jj")).unwrap();
    symlink(root.join(".git"), nested.join(".git")).unwrap();
    assert!(inspect_directory_with_runner(&nested, &runner).is_err());
    fs::remove_file(nested.join(".git")).unwrap();
    fs::create_dir(nested.join(".git")).unwrap();
    *runner.root.lock().unwrap() = Some(root);
    assert!(inspect_directory_with_runner(&nested, &runner).is_err());
}

#[test]
fn post_replacement_uncertainty_is_preserved_in_runtime_errors() {
    let uncertain = RuntimeError::from(streams_temper::storage::StorageError::uncertain(
        "directory fsync failed after rename",
    ));
    assert!(uncertain.is_uncertain());
    assert!(!RuntimeError::Invalid("bad schema".into()).is_uncertain());
}
