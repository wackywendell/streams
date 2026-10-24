//! One offline replacement boundary. Ordinary catalog actions never call this installer.
use super::{PreparedCatalog, prepare};
use crate::{
    config::{Config, Strategy},
    file_io::{check_catalog_root, existing_write_lock},
    runtime::{
        CheckoutRegistry, DirectoryBindings,
        locations::PreparedDirectoryCutover,
        native_state::NativeState,
        state::{PreparedRelationshipCutover, RuntimeState},
    },
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRequest, RecordStore},
    },
    sync::{policy::Policy, records::RecordSync},
    yaml_store::records::YamlRecordStore,
};
use serde::Serialize;
use serde_json::json;
use std::{
    collections::BTreeSet,
    fs::{self, File},
    path::{Path, PathBuf},
};

#[derive(Debug, Serialize)]
pub struct InstalledCatalog {
    pub root: PathBuf,
    /// The whole legacy catalog, including its Git history and private bookkeeping.
    pub archived_catalog: PathBuf,
    pub migrated_records: usize,
    pub dropped_foreign_records: usize,
}
struct InstallInputs<'a> {
    catalog: &'a PreparedCatalog,
    runtime: Option<(PreparedDirectoryCutover, PreparedRelationshipCutover)>,
}
fn invalid(error: impl std::fmt::Display) -> StorageError {
    StorageError::new(error.to_string())
}

/// All catalog writers and native workers must be stopped before this explicit command.
/// The archive directory must not exist; its `catalog` child receives the old root unchanged.
pub fn run(
    config: &Config,
    native_state: &NativeState,
    archive_directory: &Path,
    strategy: Strategy,
    remote: Option<&str>,
    branch: &str,
) -> super::super::Result<InstalledCatalog> {
    let lock = config.lock()?;
    let settings = lock.settings()?;
    let host = lock.existing_host_identity()?;
    let cancelled = std::sync::atomic::AtomicBool::new(false);
    let _native = native_state.lock(0.0, &cancelled)?;
    let inspected = super::super::inspect(config)?;
    let prepared = prepare(&inspected.inspection, host.as_str())?;
    let state = RuntimeState::new(
        config.runtime_state_dir_for(&settings.catalog_root)?,
        host.as_str(),
    )?;
    let bindings = DirectoryBindings::new(
        state.root(),
        CheckoutRegistry::new(config.checkout_state_dir(), host.as_str())?,
    );
    let streams: BTreeSet<_> = prepared
        .records
        .iter()
        .filter_map(|record| match record {
            crate::model::Record::Stream(stream) => Some(stream.id.clone()),
            _ => None,
        })
        .collect();
    let directories = bindings.prepare_cutover(&prepared.session_ids, &streams)?;
    let relationships = state.prepare_cutover(&prepared.session_ids, &streams)?;
    let inputs = InstallInputs {
        catalog: &prepared,
        runtime: Some((directories, relationships)),
    };
    Ok(install(
        &settings.catalog_root,
        archive_directory,
        &inputs,
        strategy,
        remote,
        branch,
        settings.sync,
    )?)
}

fn install(
    root: &Path,
    archive_directory: &Path,
    inputs: &InstallInputs<'_>,
    strategy: Strategy,
    remote: Option<&str>,
    branch: &str,
    policy: Policy,
) -> Result<InstalledCatalog, StorageError> {
    let prepared = inputs.catalog;
    check_catalog_root(root)?;
    match fs::symlink_metadata(root.join(".catalog-local/records.json")) {
        Ok(_) => {
            return Err(invalid(
                "catalog already has a record-format marker; offline cutover accepts only legacy catalogs",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(invalid(error)),
    }
    let root = fs::canonicalize(root).map_err(invalid)?;
    let parent = root
        .parent()
        .ok_or_else(|| invalid("catalog must have a parent directory"))?;
    let absolute = if archive_directory.is_absolute() {
        archive_directory.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(invalid)?
            .join(archive_directory)
    };
    let archive_directory = absolute.as_path();
    let archive_parent = archive_directory
        .parent()
        .ok_or_else(|| invalid("archive directory must have a parent"))?;
    let archive_parent = fs::canonicalize(archive_parent).map_err(invalid)?;
    let name = archive_directory
        .file_name()
        .ok_or_else(|| invalid("archive directory must have a name"))?;
    let archive_directory = archive_parent.join(name);
    if archive_directory.starts_with(&root) || fs::symlink_metadata(&archive_directory).is_ok() {
        return Err(invalid(
            "archive directory must be absent and outside the catalog",
        ));
    }
    let _lock = existing_write_lock(&root.join(".catalog.lock"))?;
    // Build and verify everything, including a fresh transport baseline, before touching old data.
    let candidate = tempfile::Builder::new()
        .prefix(".streams-record-cutover-")
        .tempdir_in(parent)
        .map_err(invalid)?;
    let records = YamlRecordStore::new(candidate.path(), &prepared.host)?;
    records.initialize()?;
    for record in &prepared.records {
        let request = RecordRequest::new(
            RecordAddress::for_record(record),
            "offline-cutover",
            &json!({"record":record}),
        )?;
        records.save::<StorageError>(&request, |_, _| {
            Ok(RecordChange {
                record: record.clone(),
                outcome: Default::default(),
            })
        })?;
    }
    RecordSync::initialize(
        candidate.path(),
        &prepared.host,
        strategy,
        remote.map(str::to_owned),
        branch,
        policy,
    )?;
    // create_dir reserves this user-named destination without replacing even an empty directory.
    fs::create_dir(&archive_directory).map_err(invalid)?;
    let archived_catalog = archive_directory.join("catalog");
    if let Err(error) = fs::rename(&root, &archived_catalog) {
        let _ = fs::remove_dir(&archive_directory);
        return Err(invalid(error));
    }
    if let Some((directories, relationships)) = &inputs.runtime {
        directories.install(&archive_directory).and_then(|()| relationships.install(&archive_directory)).map_err(|error| StorageError::uncertain(format!("old catalog is preserved at {}; runtime translation did not complete; keep workers stopped and inspect the archive: {error}", archived_catalog.display())))?;
    }
    if let Err(error) = fs::rename(candidate.path(), &root) {
        return Err(StorageError::uncertain(format!(
            "old catalog is preserved at {}; replacement did not complete: {error}",
            archived_catalog.display()
        )));
    }
    for directory in [
        parent,
        archive_directory.as_path(),
        archive_parent.as_path(),
    ] {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| {
                StorageError::uncertain(format!(
                    "catalog replacement completed but directory durability is uncertain: {error}"
                ))
            })?;
    }
    Ok(InstalledCatalog {
        root,
        archived_catalog,
        migrated_records: prepared.records.len(),
        dropped_foreign_records: prepared.dropped_foreign_records,
    })
}

#[cfg(test)]
mod tests;
