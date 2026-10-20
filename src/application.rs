//! Host-bound backend selection. Catalog operations use the backend contract thereafter.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::config::{Config, ConfigError, HostIdentity, Marker, Strategy, source_id};
use crate::model::{ValidationError, check_text};
use crate::storage::{StorageError, Store};
use crate::sync::state::{RepairRequired, State};
use crate::sync::{CatalogBackend, adoption::adopt_local, git::GitCatalog, local::LocalCatalog};
use crate::yaml_store::{YamlStore, ensure_directory};

#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Validation(#[from] ValidationError),
    #[error(transparent)]
    Repair(#[from] RepairRequired),
}

pub type Result<T> = std::result::Result<T, ApplicationError>;

/// Private identity and physical source remain bound to the selected backend.
pub struct OpenedCatalog {
    root: PathBuf,
    host: HostIdentity,
    backend: Box<dyn CatalogBackend>,
}

impl OpenedCatalog {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn host(&self) -> &HostIdentity {
        &self.host
    }
    pub fn backend(&self) -> &dyn CatalogBackend {
        self.backend.as_ref()
    }
    pub fn source_id(&self) -> Result<String> {
        Ok(source_id(&self.root, &self.host)?)
    }
}

fn invalid(message: &str) -> ApplicationError {
    ConfigError::Invalid(message.into()).into()
}

fn check_root(root: &Path) -> Result<()> {
    ensure_directory(root, false)?;
    for name in [".catalog-sync", ".catalog.lock", ".catalog-staging"] {
        if root.join(name).is_symlink() {
            return Err(invalid("symlinked catalog machinery"));
        }
    }
    ensure_directory(&root.join(".catalog-sync"), false)?;
    Ok(())
}

fn marker_exists(root: &Path) -> Result<bool> {
    match fs::symlink_metadata(root.join(".catalog-sync/cli.json")) {
        Ok(metadata) if metadata.is_file() => Ok(true),
        Ok(_) => Err(invalid(
            "catalog initialization marker must be a regular file",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(StorageError::new(error.to_string()).into()),
    }
}

fn empty_root(root: &Path) -> Result<()> {
    let io = |e: std::io::Error| StorageError::new(e.to_string());
    for entry in fs::read_dir(root).map_err(io)? {
        let name = entry.map_err(io)?.file_name();
        if name != ".catalog.lock" && name != ".catalog-sync" {
            return Err(invalid(
                "initialization requires a new empty catalog directory",
            ));
        }
    }
    let state = root.join(".catalog-sync");
    if state.exists() && fs::read_dir(state).map_err(io)?.next().is_some() {
        return Err(invalid("initialization requires empty catalog bookkeeping"));
    }
    Ok(())
}

fn write_marker(store: &YamlStore, strategy: Strategy, host: &HostIdentity) -> Result<()> {
    State::from_journal(store.journal()).write(
        "cli.json",
        json!({"strategy":strategy,"host":host.as_str()})
            .as_object()
            .expect("object")
            .clone(),
    )?;
    Ok(())
}

fn opened(
    root: PathBuf,
    host: HostIdentity,
    backend: Box<dyn CatalogBackend>,
) -> Result<OpenedCatalog> {
    backend.snapshot()?;
    Ok(OpenedCatalog {
        root,
        host,
        backend,
    })
}

/// Open existing state only; missing producer identity is never repaired by generation.
pub fn open(config: &Config) -> Result<OpenedCatalog> {
    let settings = config.load()?;
    let root = settings.catalog_root;
    check_root(&root)?;
    if !marker_exists(&root)? {
        return Err(invalid("catalog is not initialized; run streams init"));
    }
    let store = YamlStore::new(&root);
    let marker = store
        .with_transaction(|_| Ok(Marker::read(&store.journal())))??
        .ok_or_else(|| invalid("catalog is not initialized; run streams init"))?;
    let host = config.host_identity()?;
    marker.check_host(&host)?;
    let backend: Box<dyn CatalogBackend> = match marker.strategy {
        Strategy::None => Box::new(LocalCatalog::new(store, host.as_str())?),
        Strategy::Git => Box::new(GitCatalog::new(&root, host.as_str(), settings.sync)?),
    };
    opened(root, host, backend)
}

/// Initialize under the config lock; only explicit adoption may change an existing strategy.
pub fn initialize(
    config: &Config,
    strategy: Strategy,
    remote: Option<&str>,
    branch: &str,
    adopt: bool,
) -> Result<OpenedCatalog> {
    check_text(branch, "branch", false)?;
    if let Some(remote) = remote {
        check_text(remote, "Git remote", false)?;
    }
    if remote.is_some() && (strategy == Strategy::None || adopt) {
        return Err(invalid(
            "Git remote is forbidden for local-only initialization or adoption",
        ));
    }
    if adopt && strategy != Strategy::Git {
        return Err(invalid("adoption requires the Git strategy"));
    }
    let mut lock = config.lock()?;
    // Loading before the lock could select a different root from host creation.
    let settings = lock.settings()?;
    let root = settings.catalog_root;
    check_root(&root)?;
    let initialized = marker_exists(&root)?;
    if adopt && !initialized {
        return Err(invalid(
            "adoption requires an initialized local-only catalog",
        ));
    }
    let store = YamlStore::new(&root);
    let marker = store.with_transaction(|_| Ok(Marker::read(&store.journal())))??;
    let host = if marker.is_some() {
        lock.existing_host_identity()?
    } else {
        if adopt {
            return Err(invalid(
                "adoption requires an initialized local-only catalog",
            ));
        }
        lock.host_identity_for_new_catalog()?
    };
    if let Some(marker) = &marker {
        marker.check_host(&host)?;
        if marker.strategy != strategy && !(adopt && marker.strategy == Strategy::None) {
            return Err(invalid(
                "catalog sync strategy does not match; explicit adoption is required",
            ));
        }
    }
    if strategy == Strategy::None {
        store.with_transaction(|tx| {
            Ok((|| -> Result<()> {
                check_root(&root)?;
                let current = Marker::read(&store.journal())?;
                match &current {
                    None => empty_root(&root)?,
                    Some(marker) => {
                        marker.check_host(&host)?;
                        if marker.strategy != Strategy::None {
                            return Err(invalid("catalog sync strategy does not match"));
                        }
                    }
                }
                tx.read()?;
                if current.is_none() {
                    write_marker(&store, strategy, &host)?;
                }
                Ok(())
            })())
        })??;
        let backend = Box::new(LocalCatalog::new(store, host.as_str())?);
        return opened(root, host, backend);
    }
    let backend = GitCatalog::new(&root, host.as_str(), settings.sync)?;
    let backend = if adopt {
        adopt_local(&backend, branch)?;
        backend
    } else {
        if marker.is_none() {
            store.with_transaction(|_| Ok(empty_root(&root)))??;
        } else {
            backend.snapshot()?;
        }
        let backend = backend.initialize(remote.map(str::to_owned), branch)?;
        if marker.is_none() {
            backend.snapshot()?;
            store.with_transaction(|_| Ok(write_marker(&store, strategy, &host)))??;
        }
        backend
    };
    opened(root, host, Box::new(backend))
}
