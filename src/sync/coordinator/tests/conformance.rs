use super::*;
use crate as catalog_api;
use crate::sync::CatalogBackend;

#[path = "../../../../tests/support/catalog_laws.rs"]
mod catalog_laws;

// Fresh handles share repository observations and the journal, not disk crash durability.
struct MemoryCatalog(Rc<MemoryRepository>);

impl CatalogBackend for MemoryCatalog {
    fn snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.0.run(|core| core.snapshot()).map_err(repair)
    }

    fn authority(&self, stream_id: &str) -> Result<Option<Authority>, RepairRequired> {
        self.0.run(|core| core.authority(stream_id)).map_err(repair)
    }

    fn save(
        &self,
        operation: &Operation,
        origin: Option<&Origin>,
    ) -> Result<SyncResult, ValidationError> {
        self.0.run(|core| core.save(operation, origin))
    }

    fn read(&self, refresh: bool) -> Result<ReadResult, RepairRequired> {
        self.0.run(|core| core.read(refresh)).map_err(repair)
    }

    fn sync(&self, event: SyncEvent) -> SyncResult {
        self.0.run(|core| core.sync(event))
    }
}

fn with_repository(law: impl Fn(&dyn Fn() -> Box<dyn CatalogBackend>)) {
    let repository = Rc::new(MemoryRepository::new(
        Rc::default(),
        Rc::default(),
        "host",
        false,
    ));
    law(&|| Box::new(MemoryCatalog(Rc::clone(&repository))));
}

#[test]
fn identical_retries_and_creation_authority_survive_reopening() {
    with_repository(catalog_laws::identical_retries_and_creation_authority_survive_reopening);
}

#[test]
fn captured_bases_and_manual_fields_are_not_refreshed_on_retry() {
    with_repository(catalog_laws::captured_bases_and_manual_fields_are_not_refreshed_on_retry);
}

#[test]
fn producer_partitions_and_ownership_tokens_are_enforced() {
    with_repository(catalog_laws::producer_partitions_and_ownership_tokens_are_enforced);
}

#[test]
fn changed_cross_record_inputs_retire_generated_output_and_conflict_manual_intent() {
    with_repository(
        catalog_laws::changed_cross_record_inputs_retire_generated_output_and_conflict_manual_intent,
    );
}
