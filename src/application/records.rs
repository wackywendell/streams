//! Host/source admission for the record-local application path.

use std::path::Path;

use crate::{
    config::{Config, HostIdentity, Settings, Strategy, source_id},
    file_store::records::{FileRecordStore, RecordIoEvent},
    model::{Record, RecordKind, Session, Stream},
    runtime::{CheckoutRegistry, DirectoryBindings, conversations::Harness, state::RuntimeState},
    storage::records::{RecordAddress, RecordInventory, RecordListing, RecordRead, RecordStore},
    sync::records::RecordSync,
};

use super::Result;

/// An admitted producer and exact-record store, with no snapshot, history or Git access.
pub struct OpenedRecords {
    records: FileRecordStore,
    host: HostIdentity,
    source_id: String,
    settings: Settings,
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
    /// This host's checkout registry.
    pub fn registry(&self, config: &Config) -> Result<CheckoutRegistry> {
        Ok(CheckoutRegistry::new(
            config.checkout_state_dir(),
            self.host.as_str(),
        )?)
    }
    /// This host's runtime state directory for this catalog.
    pub fn runtime_state(&self, config: &Config) -> Result<RuntimeState> {
        Ok(RuntimeState::new(
            config.runtime_state_dir_for(self.root())?,
            self.host.as_str(),
        )?)
    }
    /// This host's directory bindings for this catalog.
    pub fn locations(&self, config: &Config) -> Result<DirectoryBindings> {
        let state = self.runtime_state(config)?;
        Ok(DirectoryBindings::new(state.root(), self.registry(config)?))
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
    /// Explicit human discovery/inspection only; ordinary commands use `records()`.
    pub fn inventory(&self) -> Result<Vec<RecordRead>> {
        Ok(self.records.inventory()?)
    }
    /// Every host's readable records for display, naming the ones that cannot be decoded.
    pub fn listing(&self) -> Result<RecordListing> {
        Ok(self.records.listing()?)
    }
    /// Discard whichever records of a conversation's card cannot be decoded, so a reset can
    /// rebuild them. The card is the conversation's Session, found by its native identity, and
    /// the Stream that Session names. An unreadable Session is removed: the Stream it named is
    /// unknown, so the conversation enrolls again. An unreadable Stream is replaced by a new
    /// Stream with the same ID, so every member keeps its membership; it takes the Session's
    /// title until the reset infers its own.
    pub fn discard_unreadable_card(
        &self,
        harness: Harness,
        native_id: &str,
    ) -> Result<DiscardedCard> {
        let host = self.host.as_str();
        let session_address = RecordAddress::new(
            host,
            RecordKind::Session,
            Session::id_for(host, harness.as_ref(), native_id),
        )?;
        if let Some(session) = self.records.discard_unreadable(&session_address, None)? {
            return Ok(DiscardedCard {
                session: Some(session),
                stream: None,
            });
        }
        let session = self
            .records
            .with_reader(|reader| reader.read(&session_address))?;
        let Some(RecordRead {
            record: Record::Session(session),
            ..
        }) = session
        else {
            return Ok(DiscardedCard::default());
        };
        let stream_address = RecordAddress::new(host, RecordKind::Stream, &session.stream_id)?;
        let fresh = Stream::new(&session.stream_id, &session.title, None, host)?;
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
    let records = FileRecordStore::new(&settings.catalog_root, host.as_str())?;
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
        settings,
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
    let mut records = FileRecordStore::new(&settings.catalog_root, host.as_str())?;
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
        settings,
    })
}
