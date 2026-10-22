//! Independent test adapter: table-shaped records, opaque counters, and an object journal.
//! Reopening a cloned handle retains state; this does not model disk crash durability.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

use serde_json::{Map, Value};
use streams_temper::{
    model::{Catalog, Record},
    storage::{Journal, Snapshot, StorageError, Store, Transaction},
};

#[derive(Debug, Default)]
struct Data {
    snapshot: Snapshot,
    generation: u64,
    journal: BTreeMap<String, Map<String, Value>>,
}

#[derive(Debug, Clone)]
pub struct MemoryStore {
    data: Arc<Mutex<Data>>,
    scope: Arc<Mutex<()>>,
    identity: uuid::Uuid,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self {
            data: Arc::default(),
            scope: Arc::default(),
            identity: uuid::Uuid::new_v4(),
        }
    }
}

impl Journal for MemoryTransaction<'_> {
    fn read(&self, name: &str) -> Result<Option<Map<String, Value>>, StorageError> {
        let data = self.0.data.lock().unwrap();
        Ok(data.journal.get(name).cloned())
    }

    fn write(&self, name: &str, data: Map<String, Value>) -> Result<(), StorageError> {
        let mut persisted = self.0.data.lock().unwrap();
        persisted.journal.insert(name.into(), data);
        Ok(())
    }

    fn clear(&self, name: &str) -> Result<(), StorageError> {
        let mut data = self.0.data.lock().unwrap();
        data.journal.remove(name);
        Ok(())
    }

    fn names(&self) -> Result<Vec<String>, StorageError> {
        let data = self.0.data.lock().unwrap();
        Ok(data.journal.keys().cloned().collect())
    }
}

pub struct MemoryTransaction<'a>(&'a MemoryStore);

impl Transaction for MemoryTransaction<'_> {
    fn journal(&self) -> &dyn Journal {
        self
    }

    fn check_local(&self) -> Result<(), StorageError> {
        Ok(())
    }

    fn read(&self) -> Result<Snapshot, StorageError> {
        let data = self.0.data.lock().unwrap();
        Ok(data.snapshot.clone())
    }

    fn save(&self, record: &Record, expected: &Snapshot) -> Result<Snapshot, StorageError> {
        let mut data = self.0.data.lock().unwrap();
        if data.snapshot.revisions != expected.revisions {
            return Err(StorageError::revision_conflict("stale memory snapshot"));
        }
        let catalog = data
            .snapshot
            .catalog
            .replacing(record.clone())
            .map_err(|e| StorageError::new(e.to_string()))?;
        if data.snapshot.catalog.get(&record.key()).as_ref() == Some(record) {
            return Ok(data.snapshot.clone());
        }
        let mut streams = catalog.streams().to_vec();
        let mut sessions = catalog.sessions().to_vec();
        let mut hosts = catalog.hosts().to_vec();
        streams.sort_by(|a, b| a.id.cmp(&b.id));
        sessions.sort_by(|a, b| (&a.host_id, &a.id).cmp(&(&b.host_id, &b.id)));
        hosts.sort_by(|a, b| a.id.cmp(&b.id));
        let catalog =
            Catalog::new(streams, sessions, hosts).map_err(|e| StorageError::new(e.to_string()))?;
        data.generation += 1;
        let revision = format!("{}:{}", self.0.identity, data.generation);
        data.snapshot.catalog = catalog;
        data.snapshot.revisions.insert(record.key(), revision);
        Ok(data.snapshot.clone())
    }
}

impl Store for MemoryStore {
    type Transaction<'a> = MemoryTransaction<'a>;

    fn with_transaction<T, E: From<StorageError>>(
        &self,
        f: impl FnOnce(&mut Self::Transaction<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let _guard = self.scope.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut MemoryTransaction(self))
    }
}
