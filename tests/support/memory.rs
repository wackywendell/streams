//! Independent test adapter: table-shaped records, opaque counters, and an object journal.
//! Reopening a cloned handle retains state; this does not model disk crash durability.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    thread::{self, ThreadId},
};

use serde_json::{Map, Value};
use streams_temper::{
    model::Record,
    storage::{Journal, Snapshot, StorageError, Store, Transaction},
};

#[derive(Debug, Default)]
struct Data {
    snapshot: Snapshot,
    generation: u64,
    journal: BTreeMap<String, Map<String, Value>>,
    owner: Option<ThreadId>,
}

fn check_scope(data: &Data) -> Result<(), StorageError> {
    if data.owner != Some(thread::current().id()) {
        return Err(StorageError::new("access outside owning transaction"));
    }
    Ok(())
}

struct ScopeReset<'a>(&'a Mutex<Data>);

impl Drop for ScopeReset<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().owner = None;
    }
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

#[derive(Debug, Clone)]
pub struct MemoryJournal(Arc<Mutex<Data>>);

impl Journal for MemoryJournal {
    fn read(&self, name: &str) -> Result<Option<Map<String, Value>>, StorageError> {
        let data = self.0.lock().unwrap();
        check_scope(&data)?;
        Ok(data.journal.get(name).cloned())
    }

    fn write(&self, name: &str, data: Map<String, Value>) -> Result<(), StorageError> {
        let mut persisted = self.0.lock().unwrap();
        check_scope(&persisted)?;
        persisted.journal.insert(name.into(), data);
        Ok(())
    }

    fn clear(&self, name: &str) -> Result<(), StorageError> {
        let mut data = self.0.lock().unwrap();
        check_scope(&data)?;
        data.journal.remove(name);
        Ok(())
    }

    fn names(&self) -> Result<Vec<String>, StorageError> {
        let data = self.0.lock().unwrap();
        check_scope(&data)?;
        Ok(data.journal.keys().cloned().collect())
    }
}

pub struct MemoryTransaction<'a>(&'a MemoryStore);

impl Transaction for MemoryTransaction<'_> {
    fn read(&self) -> Result<Snapshot, StorageError> {
        let data = self.0.data.lock().unwrap();
        check_scope(&data)?;
        Ok(data.snapshot.clone())
    }

    fn save(&self, record: &Record, expected: &Snapshot) -> Result<Snapshot, StorageError> {
        let mut data = self.0.data.lock().unwrap();
        check_scope(&data)?;
        if data.snapshot.revisions != expected.revisions {
            return Err(StorageError::revision_conflict("stale memory snapshot"));
        }
        let mut catalog = data
            .snapshot
            .catalog
            .replacing(record.clone())
            .map_err(|e| StorageError::new(e.to_string()))?;
        if data.snapshot.catalog.get(&record.key()).as_ref() == Some(record) {
            return Ok(data.snapshot.clone());
        }
        catalog.streams.sort_by(|a, b| a.id.cmp(&b.id));
        catalog
            .sessions
            .sort_by(|a, b| (&a.host_id, &a.id).cmp(&(&b.host_id, &b.id)));
        catalog.hosts.sort_by(|a, b| a.id.cmp(&b.id));
        data.generation += 1;
        let revision = format!("{}:{}", self.0.identity, data.generation);
        data.snapshot.catalog = catalog;
        data.snapshot.revisions.insert(record.key(), revision);
        Ok(data.snapshot.clone())
    }
}

impl Store for MemoryStore {
    type Transaction<'a> = MemoryTransaction<'a>;
    type Journal = MemoryJournal;

    fn with_transaction<T>(
        &self,
        f: impl FnOnce(&mut Self::Transaction<'_>) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        let _guard = self.scope.lock().unwrap_or_else(|e| e.into_inner());
        self.data.lock().unwrap().owner = Some(thread::current().id());
        let _reset = ScopeReset(&self.data);
        f(&mut MemoryTransaction(self))
    }

    fn journal(&self) -> Self::Journal {
        MemoryJournal(self.data.clone())
    }

    fn check_local(&self) -> Result<(), StorageError> {
        check_scope(&self.data.lock().unwrap())
    }
}
