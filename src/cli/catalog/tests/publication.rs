use super::*;
use streams_temper::{
    model::ValidationError,
    operations::Authority,
    storage::Snapshot,
    sync::{
        ReadResult,
        envelope::Origin,
        outcomes::{self, LocalStatus, SharingStatus},
        state::RepairRequired,
    },
};

/// Keep real local save/read behavior; script only the transport's reported outcome.
struct Offline<'a>(&'a LocalCatalog<memory::MemoryStore>);

impl CatalogBackend for Offline<'_> {
    fn snapshot(&self) -> std::result::Result<Snapshot, RepairRequired> {
        self.0.snapshot()
    }
    fn authority(&self, id: &str) -> std::result::Result<Option<Authority>, RepairRequired> {
        self.0.authority(id)
    }
    fn save(
        &self,
        operation: &Operation,
        origin: Option<&Origin>,
    ) -> std::result::Result<outcomes::Result, ValidationError> {
        let mut saved = self.0.save(operation, origin)?;
        assert!(saved.accepted(), "fixture expects an accepted local save");
        saved.local = LocalStatus::Committed;
        saved.sharing = SharingStatus::Pending;
        Ok(saved)
    }
    fn read(&self, refresh: bool) -> std::result::Result<ReadResult, RepairRequired> {
        self.0.read(refresh)
    }
    fn sync(&self, _: SyncEvent) -> outcomes::Result {
        outcomes::Result::new(SharingStatus::Failed, LocalStatus::Unchanged)
            .with_detail("transport unavailable")
    }
}

#[test]
fn accepted_local_write_retains_publication_warning_without_requiring_repair() {
    let f = Fixture::new();
    let stream = f.create("Work", None, Status::Active);
    let id = stream["id"].as_str().unwrap();
    let offline = Offline(&f.backend);
    let output = update(
        &offline,
        |catalog| f.select(catalog, id),
        Some("Saved offline".into()),
        &StreamFields::default(),
        |_| panic!("unexpected text read"),
        || f.operation_id(),
    )
    .unwrap();
    assert_eq!(output.code, 0, "{}", output.value);
    let result = output.value;
    assert_eq!(result["save"]["local"], "committed");
    assert_eq!(result["save"]["domain"]["outcome"], "changes");
    assert_eq!(result["publication"]["sharing"], "failed");
    assert_eq!(result["publication"]["local"], "unchanged");
    assert_eq!(result["publication"]["detail"], "transport unavailable");
    assert_eq!(
        ok(show(&offline, false, |catalog| f.select(catalog, id)))["stream"],
        result["stream"]
    );
    let synced = sync(&offline, SyncEvent::Explicit).unwrap();
    assert_eq!(synced.code, 2, "{}", synced.value);
    assert_eq!(synced.value["sharing"], "failed");
}
