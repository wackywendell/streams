use crate::support;

#[path = "support/catalog_laws.rs"]
mod catalog_laws;

use catalog_laws::creation;
use std::path::Path;
use streams_temper as catalog_api;
use streams_temper::{
    storage::Store,
    sync::{
        CatalogBackend,
        git::GitCatalog,
        local::LocalCatalog,
        outcomes::{LocalStatus, SharingStatus},
        policy::{Policy, SyncEvent},
        state::RepairRequired,
    },
    yaml_store::YamlStore,
};
use support::memory::MemoryStore;

fn local_git(root: &Path) -> GitCatalog {
    let mut backend = GitCatalog::new(root, "host", Policy::default()).unwrap();
    support::git::install_hook(&mut backend, |_, _, _, _| None);
    backend
}

fn each_memory_backend(law: impl Fn(&dyn Fn() -> Box<dyn CatalogBackend>)) {
    let memory = MemoryStore::default();
    law(&|| Box::new(LocalCatalog::new(memory.clone(), "host").unwrap()));
}

// The law takes only the application interface and a reopen factory, not concrete adapters.
fn each_backend(law: impl Fn(&dyn Fn() -> Box<dyn CatalogBackend>)) {
    each_local_backend(&law);
    let dir = tempfile::tempdir().unwrap();
    local_git(dir.path()).initialize(None, "main").unwrap();
    law(&|| Box::new(local_git(dir.path())));
}

fn each_local_backend(law: impl Fn(&dyn Fn() -> Box<dyn CatalogBackend>)) {
    let dir = tempfile::tempdir().unwrap();
    law(&|| Box::new(LocalCatalog::new(YamlStore::new(dir.path()), "host").unwrap()));
    let memory = MemoryStore::default();
    law(&|| Box::new(LocalCatalog::new(memory.clone(), "host").unwrap()));
}

#[test]
fn identical_retries_and_creation_authority_survive_reopening() {
    each_backend(catalog_laws::identical_retries_and_creation_authority_survive_reopening);
}

#[test]
fn captured_bases_and_manual_fields_are_not_refreshed_on_retry() {
    each_memory_backend(catalog_laws::captured_bases_and_manual_fields_are_not_refreshed_on_retry);
}

#[test]
fn producer_partitions_and_ownership_tokens_are_enforced() {
    each_memory_backend(catalog_laws::producer_partitions_and_ownership_tokens_are_enforced);
}

#[test]
fn changed_cross_record_inputs_retire_generated_output_and_conflict_manual_intent() {
    each_memory_backend(catalog_laws::changed_cross_record_inputs_retire_generated_output_and_conflict_manual_intent);
}

#[test]
fn local_only_read_refresh_and_sync_do_not_invent_publication() {
    each_local_backend(|open| {
        let backend = open();
        backend.save(&creation(), None).unwrap();
        let saved = backend.snapshot().unwrap();
        for refresh in [false, true] {
            let view = backend.read(refresh).unwrap();
            assert_eq!(view.catalog, saved.catalog);
            assert_eq!(view.local, saved);
            assert!(view.confirmed.is_none());
            assert_eq!(backend.snapshot().unwrap(), saved);
        }
        for event in [SyncEvent::Start, SyncEvent::Explicit, SyncEvent::Shutdown] {
            let receipt = backend.sync(event);
            assert_eq!(receipt.sharing, SharingStatus::LocalOnly);
            assert_eq!(receipt.local, LocalStatus::Unchanged);
            assert_eq!(backend.snapshot().unwrap(), saved);
        }
    });
}

fn failed_clear_fences_reopened_store<S: Store + Clone>(store: S) {
    let mut backend = LocalCatalog::new(store.clone(), "host").unwrap();
    backend.set_state_hook(std::sync::Arc::new(std::sync::Mutex::new(
        |operation: &str, name: &str| {
            if operation == "clear" && name == "save.json" {
                Err(RepairRequired::new("injected failed clear"))
            } else {
                Ok(())
            }
        },
    )));
    assert!(backend.save(&creation(), None).unwrap().requires_repair());
    let reopened = LocalCatalog::new(store, "host").unwrap();
    assert!(reopened.snapshot().is_err());
    assert!(reopened.save(&creation(), None).unwrap().requires_repair());
}

#[test]
fn recovery_journal_is_not_tied_to_filesystem_paths() {
    let dir = tempfile::tempdir().unwrap();
    failed_clear_fences_reopened_store(YamlStore::new(dir.path()));
    failed_clear_fences_reopened_store(MemoryStore::default());
}
