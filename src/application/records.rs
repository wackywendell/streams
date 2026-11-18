//! Host/source admission for the record-local application path.

use std::path::Path;

use crate::{
    config::{Config, HostIdentity, Strategy, source_id},
    storage::records::{RecordAddress, RecordInventory, RecordRead, RecordStore},
    sync::records::RecordSync,
    yaml_store::records::{RecordIoEvent, YamlRecordStore},
};

use super::Result;

/// An admitted producer and exact-record store, with no snapshot, history or Git access.
pub struct OpenedRecords {
    records: YamlRecordStore,
    host: HostIdentity,
    source_id: String,
}

impl OpenedRecords {
    pub fn root(&self) -> &Path {
        self.records.root()
    }
    pub fn host(&self) -> &HostIdentity {
        &self.host
    }
    pub fn source_id(&self) -> &str {
        &self.source_id
    }
    pub fn records(&self) -> &impl RecordStore {
        &self.records
    }
    pub fn artifacts(&self) -> &impl crate::artifacts::ArtifactStore {
        &self.records
    }
    /// Inspect only this admitted host's direct Session partition.
    pub fn host_sessions(&self) -> Result<Vec<RecordRead>> {
        Ok(self.records.host_sessions(self.host.as_str())?)
    }
    /// Inspect only this admitted host's direct Stream partition.
    pub fn host_streams(&self) -> Result<Vec<RecordRead>> {
        Ok(self.records.host_streams(self.host.as_str())?)
    }
    /// Last-updated timestamps in Unix ms from the active catalog backend.
    pub fn last_updated_ms(
        &self,
        addresses: &[RecordAddress],
    ) -> Result<std::collections::BTreeMap<RecordAddress, u64>> {
        if self.root().join(".catalog-local/transport.json").exists() {
            let sync = RecordSync::open(self.root(), self.host.as_str())?;
            Ok(sync.last_updated_ms(addresses)?)
        } else {
            self.local_last_updated_ms(addresses)
        }
    }
    /// Local file mtime timestamps without admitting transport configuration.
    pub fn local_last_updated_ms(
        &self,
        addresses: &[RecordAddress],
    ) -> Result<std::collections::BTreeMap<RecordAddress, u64>> {
        Ok(self.records.last_updated_ms(addresses)?)
    }
    /// Explicit human discovery/inspection only; ordinary commands use `records()`.
    pub fn inventory(&self) -> Result<Vec<RecordRead>> {
        Ok(self.records.inventory()?)
    }
}

/// Initialize only an empty catalog. Existing data is neither interpreted nor adopted.
pub fn initialize_records(
    config: &Config,
    strategy: Strategy,
    remote: Option<&str>,
    branch: &str,
) -> Result<OpenedRecords> {
    let mut lock = config.lock()?;
    let settings = lock.settings()?;
    let host = lock.host_identity_for_new_catalog()?;
    let records = YamlRecordStore::new(&settings.catalog_root, host.as_str())?;
    records.initialize()?;
    RecordSync::initialize(
        records.root(),
        host.as_str(),
        strategy,
        remote.map(str::to_owned),
        branch,
    )?;
    let source_id =
        records.with_reader(|_| -> Result<String> { Ok(source_id(records.root(), &host)?) })?;
    Ok(OpenedRecords {
        records,
        host,
        source_id,
    })
}

/// Legacy or incomplete roots refuse here; no operation probes or adopts their layout.
pub fn open_records(config: &Config) -> Result<OpenedRecords> {
    open(config, None::<fn(RecordIoEvent)>)
}

/// Observe actual adapter I/O, including the format check performed during opening.
/// Configuration/producer reads and source-directory metadata are outside this adapter trace.
pub fn open_records_observed(
    config: &Config,
    observe: impl Fn(RecordIoEvent) + Send + Sync + 'static,
) -> Result<OpenedRecords> {
    open(config, Some(observe))
}

fn open(
    config: &Config,
    observe: Option<impl Fn(RecordIoEvent) + Send + Sync + 'static>,
) -> Result<OpenedRecords> {
    let settings = config.load()?;
    let host = config.host_identity()?;
    let mut records = YamlRecordStore::new(&settings.catalog_root, host.as_str())?;
    if let Some(observe) = observe {
        records = records.with_io_observer(observe);
    }
    // Check source identity under the same shared lock as the completed-format gate.
    let source_id =
        records.with_reader(|_| -> Result<String> { Ok(source_id(records.root(), &host)?) })?;
    Ok(OpenedRecords {
        records,
        host,
        source_id,
    })
}
