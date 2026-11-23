//! Host/source admission for the record-local application path.

use std::path::{Path, PathBuf};

use crate::{
    config::{Config, HostIdentity, Settings, Strategy, source_id},
    file_store::records::{FileRecordStore, RecordIoEvent},
    model::{Host, Record, Stream, Title},
    runtime::state::RuntimeState,
    storage::records::{RecordAddress, RecordChange, RecordInventory, RecordListing, RecordStore},
    sync::{self, RemoteUpdate},
};

use super::Result;

/// An admitted producer and exact-record store, with no snapshot, history or Git access.
pub struct OpenedRecords {
    records: FileRecordStore,
    host: HostIdentity,
    source_id: String,
    settings: Settings,
    runtime_state_dir: PathBuf,
}

impl OpenedRecords {
    pub fn root(&self) -> &Path {
        self.records.root()
    }
    pub fn host(&self) -> &HostIdentity {
        &self.host
    }
    /// The settings loaded when this catalog was opened.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    /// This host's runtime state directory for this catalog.
    pub fn runtime_state(&self) -> Result<RuntimeState> {
        Ok(RuntimeState::new(
            &self.runtime_state_dir,
            self.host.as_str(),
        )?)
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
    pub fn host_sessions(&self) -> Result<RecordListing> {
        Ok(self.records.host_sessions(self.host.as_str())?)
    }
    /// Inspect only this admitted host's direct Stream partition.
    pub fn host_streams(&self) -> Result<RecordListing> {
        Ok(self.records.host_streams(self.host.as_str())?)
    }
    /// Inspect a named host's direct Session partition.
    pub fn sessions_for_host(&self, host: &str) -> Result<RecordListing> {
        Ok(self.records.host_sessions(host)?)
    }
    /// Inspect a named host's direct Stream partition.
    pub fn streams_for_host(&self, host: &str) -> Result<RecordListing> {
        Ok(self.records.host_streams(host)?)
    }
    /// Explicit human discovery/inspection only; ordinary commands use `records()`.
    pub fn inventory(&self) -> Result<Vec<Record>> {
        Ok(self.records.inventory()?)
    }
    /// Every host's readable records for display, naming the ones that cannot be decoded.
    pub fn listing(&self) -> Result<RecordListing> {
        Ok(self.records.listing()?)
    }
    /// Delete an empty local Stream record owned by this host.
    pub fn delete_stream(&self, target: &RecordAddress) -> Result<()> {
        if target.host() != self.host.as_str() {
            return Err(super::ApplicationError::invalid(format!(
                "cannot delete Stream {} owned by host {:?} from host {:?}",
                target.full_key(),
                target.host(),
                self.host.as_str()
            )));
        }
        let deleted = self.records.delete_empty_stream(target)?;
        if !deleted {
            return Err(super::ApplicationError::not_found(format!(
                "no Stream {}",
                target.full_key()
            )));
        }
        Ok(())
    }
    /// Discard whichever records of a conversation's card cannot be decoded, so a reset can
    /// rebuild them.
    pub fn discard_unreadable_card(&self, harness: &str, id: &str) -> Result<DiscardedCard> {
        let host = self.host.as_str();
        let session_address = RecordAddress::session(host, harness, id)?;
        if let Some(session) = self.records.discard_unreadable(&session_address, None)? {
            return Ok(DiscardedCard {
                session: Some(session),
                stream: None,
            });
        }
        let session = self
            .records
            .with_reader(|reader| reader.read(&session_address))?;
        let Some(Record::Session(session)) = session else {
            return Ok(DiscardedCard::default());
        };
        let stream_address = RecordAddress::stream(host, &session.stream_id)?;
        let fresh = Stream::new(
            host,
            &session.stream_id,
            Title::new(&session.title.text, false),
            None,
        )?;
        Ok(DiscardedCard {
            session: None,
            stream: self
                .records
                .discard_unreadable(&stream_address, Some(Record::Stream(fresh)))?,
        })
    }
}

/// The decode error of each card record a reset discarded; `None` where it was readable.
#[derive(Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct DiscardedCard {
    pub session: Option<String>,
    pub stream: Option<String>,
}

/// Return the current machine's OS hostname trimmed for a default host label, falling back
/// to `"host"` when unavailable or invalid.
pub fn default_host_label() -> String {
    let mut buf = [0u8; 256];
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    if ok {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if let Ok(raw) = std::str::from_utf8(&buf[..len]) {
            let trimmed = raw.trim();
            if Host::new("host", trimmed).is_ok() {
                return trimmed.to_owned();
            }
        }
    }
    "host".to_owned()
}

/// Read one host's saved label from `hosts/<host_id>/host.json` without scanning the catalog.
pub fn read_host_label(opened: &OpenedRecords, host_id: &str) -> Result<Option<String>> {
    let address = RecordAddress::host_record(host_id)?;
    let record = opened
        .records()
        .with_reader::<_, super::ApplicationError>(|reader| Ok(reader.read(&address)?))?;
    Ok(match record {
        Some(Record::Host(host)) => Some(host.label),
        _ => None,
    })
}

/// Set or update this host's label in `hosts/<local-host>/host.json`.
pub fn set_host_label(opened: &OpenedRecords, label: &str) -> Result<(Host, bool)> {
    let host = Host::new(opened.host().as_str(), label)?;
    let address = RecordAddress::host_record(&host.id)?;
    let save =
        opened
            .records()
            .save::<_, super::ApplicationError>(&address, |_reader, _before| {
                Ok(RecordChange {
                    record: Record::Host(host.clone()),
                    outcome: (),
                })
            })?;
    Ok((host, save.written))
}

/// Initialize only an empty catalog. Existing data is neither interpreted nor adopted.
pub fn initialize_records(
    config: &Config,
    strategy: Strategy,
    remote: Option<&str>,
    branch: &str,
) -> Result<OpenedRecords> {
    initialize_records_with_label(config, strategy, remote, branch, None)
}

/// Initialize an empty catalog and write `hosts/<host>/host.json` with `label` (or OS hostname).
pub fn initialize_records_with_label(
    config: &Config,
    strategy: Strategy,
    remote: Option<&str>,
    branch: &str,
    label: Option<&str>,
) -> Result<OpenedRecords> {
    let resolved_label = match label {
        Some(value) => {
            Host::new("host", value)?;
            value.to_owned()
        }
        None => default_host_label(),
    };
    let mut lock = config.lock()?;
    let settings = lock.settings()?;
    let host = lock.host_identity_for_new_catalog()?;
    let records = FileRecordStore::new(&settings.catalog_root, host.as_str())?;
    records.initialize()?;
    let host_record = Host::new(host.as_str(), &resolved_label)?;
    let host_address = RecordAddress::host_record(host.as_str())?;
    records.save::<_, super::ApplicationError>(&host_address, |_reader, _before| {
        Ok(RecordChange {
            record: Record::Host(host_record.clone()),
            outcome: (),
        })
    })?;
    sync::initialize(
        records.root(),
        host.as_str(),
        strategy,
        remote.map(str::to_owned),
        branch,
    )?;
    let source_id =
        records.with_reader(|_| -> Result<String> { Ok(source_id(records.root(), &host)?) })?;
    let runtime_state_dir = config.runtime_state_dir_for(records.root())?;
    Ok(OpenedRecords {
        records,
        host,
        source_id,
        settings,
        runtime_state_dir,
    })
}

/// Reconfigure the sync transport on an already-initialized catalog.
pub fn configure_records(
    config: &Config,
    strategy: Strategy,
    remote: RemoteUpdate,
    branch: Option<&str>,
) -> Result<(OpenedRecords, Strategy, Option<String>, Option<String>)> {
    let opened = open_records(config)?;
    let transport = sync::configure(
        opened.root(),
        opened.host().as_str(),
        strategy,
        remote,
        branch,
    )?;
    Ok((
        opened,
        transport.strategy,
        transport.remote,
        transport.branch,
    ))
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
    let mut records = FileRecordStore::new(&settings.catalog_root, host.as_str())?;
    if let Some(observe) = observe {
        records = records.with_io_observer(observe);
    }
    let source_id =
        records.with_reader(|_| -> Result<String> { Ok(source_id(records.root(), &host)?) })?;
    let runtime_state_dir = config.runtime_state_dir_for(records.root())?;
    Ok(OpenedRecords {
        records,
        host,
        source_id,
        settings,
        runtime_state_dir,
    })
}
