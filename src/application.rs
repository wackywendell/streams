//! Host-bound backend selection. Catalog operations use the backend contract thereafter.

use std::fs;
use std::path::{Path, PathBuf};

use crate::config::{Config, ConfigError, HostIdentity, Marker, Strategy, source_id};
use crate::file_io::{check_catalog_root, check_empty_catalog_root};
use crate::model::{ValidationError, check_text};
use crate::storage::{StorageError, Transaction};
use crate::sync::state::{RepairRequired, State};
use crate::sync::{CatalogBackend, adoption::adopt_local, git::GitCatalog, local::LocalCatalog};
use crate::yaml_store::YamlStore;

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

fn empty_root(root: &Path) -> Result<()> {
    let io = |e: std::io::Error| StorageError::new(e.to_string());
    check_empty_catalog_root(root)?;
    let state = root.join(".catalog-sync");
    if state.exists() && fs::read_dir(state).map_err(io)?.next().is_some() {
        return Err(invalid("initialization requires empty catalog bookkeeping"));
    }
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

/// Frozen inspection data has no backend write or native effect capability.
pub struct InspectedCatalog {
    pub root: PathBuf,
    pub host: HostIdentity,
    pub source: String,
    pub inspection: crate::sync::Inspection,
}

pub fn inspect(config: &Config) -> Result<InspectedCatalog> {
    let (root, host, backend) = open_backend(config, true)?;
    let inspection = backend.inspect()?;
    Ok(InspectedCatalog {
        source: source_id(&root, &host)?,
        root,
        host,
        inspection,
    })
}

/// Open existing state only; missing producer identity is never repaired by generation.
pub fn open(config: &Config) -> Result<OpenedCatalog> {
    let (root, host, backend) = open_backend(config, false)?;
    opened(root, host, backend)
}

fn open_backend(
    config: &Config,
    read_only: bool,
) -> Result<(PathBuf, HostIdentity, Box<dyn CatalogBackend>)> {
    let settings = config.load()?;
    let root = settings.catalog_root;
    check_catalog_root(&root)?;
    if !Marker::exists(&root)? {
        return Err(invalid("catalog is not initialized; run streams init"));
    }
    let store = YamlStore::new(&root);
    let read_marker = |tx: &mut crate::yaml_store::YamlTransaction| -> Result<_> {
        Ok(Marker::read(tx.journal())?)
    };
    let marker = if read_only {
        store.with_inspection(read_marker)?
    } else {
        store.with_transaction(read_marker)?
    }
    .ok_or_else(|| invalid("catalog is not initialized; run streams init"))?;
    let host = config.host_identity()?;
    marker.check_host(&host)?;
    let backend: Box<dyn CatalogBackend> = match marker.strategy {
        Strategy::None => Box::new(LocalCatalog::new(store, host.as_str())?),
        Strategy::Git => Box::new(GitCatalog::new(&root, host.as_str(), settings.sync)?),
    };
    Ok((root, host, backend))
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
    check_catalog_root(&root)?;
    let initialized = Marker::exists(&root)?;
    if adopt && !initialized {
        return Err(invalid(
            "adoption requires an initialized local-only catalog",
        ));
    }
    let store = YamlStore::new(&root);
    let marker = store.with_transaction(|tx| -> Result<_> { Ok(Marker::read(tx.journal())?) })?;
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
        store.with_transaction(|tx| -> Result<()> {
            check_catalog_root(&root)?;
            let current = Marker::read(tx.journal())?;
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
                Marker {
                    strategy,
                    host: host.clone(),
                }
                .write(&State::from_journal(tx.journal()))?;
            }
            Ok(())
        })?;
        let backend = Box::new(LocalCatalog::new(store, host.as_str())?);
        return opened(root, host, backend);
    }
    let backend = GitCatalog::new(&root, host.as_str(), settings.sync)?;
    let backend = if adopt {
        adopt_local(&backend, branch)?;
        backend
    } else {
        if marker.is_none() {
            store.with_transaction(|_| empty_root(&root))?;
        } else {
            backend.snapshot()?;
        }
        let backend = backend.initialize(remote.map(str::to_owned), branch)?;
        if marker.is_none() {
            backend.snapshot()?;
            store.with_transaction(|tx| -> Result<()> {
                Marker {
                    strategy,
                    host: host.clone(),
                }
                .write(&State::from_journal(tx.journal()))?;
                Ok(())
            })?;
        }
        backend
    };
    opened(root, host, Box::new(backend))
}
