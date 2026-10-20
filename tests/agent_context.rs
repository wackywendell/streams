use std::{cell::Cell, collections::BTreeMap};

use streams_temper::{
    agent::{ContextRequest, context},
    model::{Catalog, Session, Stream, ValidationError},
    operations::{Authority, Operation},
    runtime::conversations::Harness,
    storage::Snapshot,
    sync::{
        CatalogBackend, ReadResult, envelope::Origin, outcomes, policy::SyncEvent,
        state::RepairRequired,
    },
};

struct Backend {
    catalog: Catalog,
    snapshots: Cell<usize>,
}
impl Backend {
    fn new() -> Self {
        let streams =
            ["work", "other"].map(|id| Stream::new(id, "Not an identity", None, "host").unwrap());
        let sessions = vec![
            Session::new("local", "host", "work", "pi", "native", "Local").unwrap(),
            Session::new(
                "remote",
                "other-host",
                "work",
                "pi",
                "remote-native",
                "Remote",
            )
            .unwrap(),
        ];
        Self {
            catalog: Catalog::new(streams.into(), sessions, vec![]).unwrap(),
            snapshots: Cell::new(0),
        }
    }
}
impl CatalogBackend for Backend {
    fn snapshot(&self) -> Result<Snapshot, RepairRequired> {
        self.snapshots.set(self.snapshots.get() + 1);
        Ok(Snapshot {
            catalog: self.catalog.clone(),
            revisions: BTreeMap::new(),
        })
    }
    fn authority(&self, stream_id: &str) -> Result<Option<Authority>, RepairRequired> {
        assert_eq!(stream_id, "work");
        Ok(Some(Authority::new("host", "original-creation").unwrap()))
    }
    fn save(&self, _: &Operation, _: Option<&Origin>) -> Result<outcomes::Result, ValidationError> {
        panic!("context must not persist anything")
    }
    fn read(&self, _: bool) -> Result<ReadResult, RepairRequired> {
        panic!("context must not fetch/project a read")
    }
    fn sync(&self, _: SyncEvent) -> outcomes::Result {
        panic!("context must not publish")
    }
}
fn request(native_id: &str) -> ContextRequest<'_> {
    ContextRequest {
        harness: Harness::Pi,
        native_id,
        stream_id: None,
        environment_stream_id: None,
        workspace_stream_id: None,
    }
}

#[test]
fn enrolled_context_preserves_local_records_authority_and_all_hosts_sessions() {
    let backend = Backend::new();
    let before = backend.catalog.clone();
    let result = context(&backend, "host", "physical-source", request("native")).unwrap();
    assert_eq!(result.stream.as_ref().unwrap().id, "work");
    assert_eq!(result.session.as_ref().unwrap().id, "local");
    assert_eq!(result.sessions, before.sessions());
    assert_eq!(
        result.authority.as_ref().unwrap().token,
        "original-creation"
    );
    assert_eq!(backend.snapshots.get(), 1);
    assert_eq!(backend.catalog, before);
    let wire = serde_json::to_value(result).unwrap();
    assert_eq!(wire.as_object().unwrap().len(), 6);
    assert_eq!(wire["host_id"], "host");
    assert_eq!(wire["catalog_source"], "physical-source");
    assert_eq!(wire["stream"]["owner"]["host_id"], "host");
}

#[test]
fn explicit_association_never_attaches_an_unenrolled_conversation() {
    let backend = Backend::new();
    let mut captured = request("not-enrolled");
    captured.stream_id = Some("work");
    captured.environment_stream_id = Some("work");
    captured.workspace_stream_id = Some("work");
    let result = context(&backend, "host", "source", captured).unwrap();
    assert_eq!(result.stream.unwrap().id, "work");
    assert!(result.session.is_none());
    assert_eq!(result.sessions.len(), 2);
    assert!(result.authority.is_some());
}

#[test]
fn conflicting_identity_sources_fail_instead_of_moving_or_guessing() {
    let backend = Backend::new();
    for index in 0..3 {
        let mut captured = request("native");
        match index {
            0 => captured.stream_id = Some("other"),
            1 => captured.environment_stream_id = Some("other"),
            _ => captured.workspace_stream_id = Some("other"),
        }
        assert!(
            context(&backend, "host", "source", captured)
                .unwrap_err()
                .to_string()
                .contains("explicit move or fork")
        );
    }
    let mut captured = request("not-enrolled");
    captured.stream_id = Some("work");
    captured.workspace_stream_id = Some("other");
    assert!(context(&backend, "host", "source", captured).is_err());
}

#[test]
fn exact_identity_is_host_and_harness_scoped_without_title_inference() {
    let backend = Backend::new();
    for (host, harness, native) in [
        ("host", Harness::Pi, "Not an identity"),
        ("host", Harness::Pi, "remote-native"),
        ("host", Harness::Codex, "native"),
        ("other-host", Harness::Pi, "native"),
    ] {
        let mut captured = request(native);
        captured.harness = harness;
        let result = context(&backend, host, "source", captured).unwrap();
        assert!(result.session.is_none() && result.stream.is_none());
        assert!(result.sessions.is_empty() && result.authority.is_none());
        let wire = serde_json::to_value(result).unwrap();
        assert!(
            wire["session"].is_null() && wire["stream"].is_null() && wire["authority"].is_null()
        );
    }
}

#[test]
fn unknown_and_malformed_associations_do_not_refresh_remote_records() {
    let backend = Backend::new();
    for id in ["unknown", "", " ", "work\n"] {
        let mut captured = request("not-enrolled");
        captured.stream_id = Some(id);
        assert!(context(&backend, "host", "source", captured).is_err());
    }
    assert!(context(&backend, "host", "source", request(" ")).is_err());
}
