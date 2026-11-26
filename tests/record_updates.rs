//! Format-5 edit model, RFC 7386 merge, concurrency, and fake-store tests.

use std::{collections::BTreeMap, sync::Mutex};

use serde_json::json;
use streams::{
    application::{edit_session, edit_stream},
    model::{
        Goal, Record, ReferenceEntry, ReferenceKind, ReferenceRole, ReferenceStatus,
        RepositoryInfo, Session, SessionDirectory, Stamps, Stream, TextPair, Title,
    },
    operations::edits::EditRequest,
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordReader, RecordSave, RecordStore},
    },
};

type BeforeSaveHook = Box<dyn FnOnce(&mut BTreeMap<RecordAddress, Record>) + Send>;

/// In-memory fake `RecordStore` that can inject an outside write immediately before `save`.
struct FakeStore {
    records: Mutex<BTreeMap<RecordAddress, Record>>,
    inject_before_save: Mutex<Option<BeforeSaveHook>>,
}

impl FakeStore {
    fn new(records: Vec<Record>) -> Self {
        let mut map = BTreeMap::new();
        for mut record in records {
            record.set_stamps(Stamps {
                revision: 1,
                updated_at_ms: 1000,
            });
            map.insert(RecordAddress::for_record(&record), record);
        }
        Self {
            records: Mutex::new(map),
            inject_before_save: Mutex::new(None),
        }
    }

    fn inject_next_save(
        &self,
        f: impl FnOnce(&mut BTreeMap<RecordAddress, Record>) + Send + 'static,
    ) {
        *self.inject_before_save.lock().unwrap() = Some(Box::new(f));
    }

    fn get(&self, addr: &RecordAddress) -> Option<Record> {
        self.records.lock().unwrap().get(addr).cloned()
    }
}

struct FakeReader<'a>(&'a BTreeMap<RecordAddress, Record>);

impl RecordReader for FakeReader<'_> {
    fn read(&self, address: &RecordAddress) -> Result<Option<Record>, StorageError> {
        Ok(self.0.get(address).cloned())
    }
}

impl RecordStore for FakeStore {
    fn with_reader<T, E: From<StorageError>>(
        &self,
        read: impl FnOnce(&dyn RecordReader) -> Result<T, E>,
    ) -> Result<T, E> {
        let guard = self.records.lock().unwrap();
        read(&FakeReader(&guard))
    }

    fn save<T, E: From<StorageError>>(
        &self,
        target: &RecordAddress,
        apply: impl FnOnce(&dyn RecordReader, Option<&Record>) -> Result<RecordChange<T>, E>,
    ) -> Result<RecordSave<T>, E> {
        let mut guard = self.records.lock().unwrap();
        if let Some(inject) = self.inject_before_save.lock().unwrap().take() {
            inject(&mut guard);
        }
        let previous = guard.get(target).cloned();
        let RecordChange {
            mut record,
            outcome,
        } = apply(&FakeReader(&guard), previous.as_ref())?;
        record
            .validate()
            .map_err(|e| StorageError::new(e.to_string()))?;
        let stamps = previous
            .as_ref()
            .and_then(|r| r.stamps())
            .unwrap_or_default();
        record.set_stamps(stamps);
        if let Some(current) = previous
            && current == record
        {
            return Ok(RecordSave {
                current,
                outcome,
                written: false,
            });
        }
        record.set_stamps(Stamps {
            revision: stamps.revision + 1,
            updated_at_ms: stamps.updated_at_ms + 1,
        });
        guard.insert(target.clone(), record.clone());
        Ok(RecordSave {
            current: record,
            outcome,
            written: true,
        })
    }
}

fn fixture() -> (FakeStore, RecordAddress, RecordAddress) {
    let mut stream = Stream::new("host", "s1", Title::new("Stream", false), None).unwrap();
    stream.goal = Some(Goal::new("Original goal", Some("orig".into()), false));
    let s2 = Stream::new("host", "s2", Title::new("Second", false), None).unwrap();

    let mut session =
        Session::new("host", "pi", "sess1", "s1", Title::new("Session", false)).unwrap();
    session.recap = Some(TextPair::new("Drafting", Some("draft".into())));
    session.next = Some(TextPair::new("Finish tests", None));
    session.directory = Some(SessionDirectory {
        path: "/tmp/work".into(),
        repository: Some(RepositoryInfo {
            url: "git@github.com:o/r.git".into(),
            within: None,
            branch: Some("main".into()),
        }),
    });
    session.references.insert(
        "https://github.com/o/r/pull/43".into(),
        ReferenceEntry {
            kind: ReferenceKind::Pr,
            name: "PR 43".into(),
            purpose: "Implements it".into(),
            role: ReferenceRole::Deliverable,
            status: ReferenceStatus::Active,
            manual: false,
        },
    );

    let stream_addr = RecordAddress::for_stream(&stream);
    let session_addr = RecordAddress::for_session(&session);
    let store = FakeStore::new(vec![
        Record::Stream(stream),
        Record::Stream(s2),
        Record::Session(session),
    ]);
    (store, stream_addr, session_addr)
}

#[test]
fn edit_without_before_overwrites_and_requires_complete_objects() {
    let (store, stream_addr, session_addr) = fixture();

    // Missing `manual` without `before` fails the whole request and writes nothing.
    let incomplete = EditRequest::parse(&json!({
        "changes": { "title": { "text": "New" } }
    }))
    .unwrap();
    assert!(edit_stream(&store, &stream_addr, &incomplete).is_err());

    // Complete overwrite succeeds.
    let complete = EditRequest::parse(&json!({
        "changes": {
            "title": { "text": "Renamed", "manual": true },
            "goal": null
        }
    }))
    .unwrap();
    let res = edit_stream(&store, &stream_addr, &complete).unwrap();
    assert!(res.is_complete());
    assert!(res.written);
    let Record::Stream(after) = store.get(&stream_addr).unwrap() else {
        unreachable!()
    };
    assert_eq!(after.title, Title::new("Renamed", true));
    assert_eq!(after.goal, None);

    // Partial reference without `before` fails as a whole.
    let partial_ref = EditRequest::parse(&json!({
        "changes": {
            "references": {
                "https://github.com/o/r/pull/43": { "status": "fulfilled" }
            }
        }
    }))
    .unwrap();
    assert!(edit_session(&store, &session_addr, &partial_ref).is_err());
}

#[test]
fn edit_with_before_merges_rfc7386_preserving_explicit_nulls() {
    let (store, _stream_addr, session_addr) = fixture();

    let req = EditRequest::parse(&json!({
        "changes": {
            "recap": { "short": null },
            "next": null,
            "directory": { "repository": { "branch": null } },
            "references": {
                "https://github.com/o/r/pull/43": { "status": "fulfilled" },
                "docs/plan.md": {
                    "kind": "document",
                    "name": "Plan",
                    "purpose": "The plan",
                    "role": "brief",
                    "status": "active",
                    "manual": false
                }
            }
        },
        "before": {
            "recap": { "text": "Drafting", "short": "draft" },
            "next": { "text": "Finish tests", "short": null },
            "directory": {
                "path": "/tmp/work",
                "repository": {
                    "url": "git@github.com:o/r.git",
                    "within": null,
                    "branch": "main"
                }
            },
            "references": {
                "https://github.com/o/r/pull/43": {
                    "kind": "pr",
                    "name": "PR 43",
                    "purpose": "Implements it",
                    "role": "deliverable",
                    "status": "active",
                    "manual": false
                },
                "docs/plan.md": null
            }
        }
    }))
    .unwrap();

    let res = edit_session(&store, &session_addr, &req).unwrap();
    assert!(res.is_complete());
    assert!(res.written);

    let Record::Session(after) = store.get(&session_addr).unwrap() else {
        unreachable!()
    };
    assert_eq!(after.recap, Some(TextPair::new("Drafting", None)));
    assert_eq!(after.next, None);
    let repo = after
        .directory
        .as_ref()
        .unwrap()
        .repository
        .as_ref()
        .unwrap();
    assert_eq!(repo.url, "git@github.com:o/r.git");
    assert_eq!(repo.branch, None);
    assert_eq!(
        after.references["https://github.com/o/r/pull/43"].status,
        ReferenceStatus::Fulfilled
    );
    assert_eq!(after.references["docs/plan.md"].name, "Plan");

    // Clearing directory.repository to null keeps directory.path.
    let clear_repo = EditRequest::parse(&json!({
        "changes": { "directory": { "repository": null } },
        "before": { "directory": serde_json::to_value(&after.directory).unwrap() }
    }))
    .unwrap();
    let res = edit_session(&store, &session_addr, &clear_repo).unwrap();
    assert!(res.is_complete());
    let Record::Session(after2) = store.get(&session_addr).unwrap() else {
        unreachable!()
    };
    assert_eq!(after2.directory.as_ref().unwrap().path, "/tmp/work");
    assert_eq!(after2.directory.as_ref().unwrap().repository, None);

    // Setting only branch when before.directory.repository is null fails (merged repository incomplete).
    let bad_branch = EditRequest::parse(&json!({
        "changes": { "directory": { "repository": { "branch": "feat" } } },
        "before": { "directory": { "path": "/tmp/work", "repository": null } }
    }))
    .unwrap();
    assert!(edit_session(&store, &session_addr, &bad_branch).is_err());
}

#[test]
fn edit_rejects_mismatched_before_keys_readonly_computed_unknown_and_null_references() {
    let (store, stream_addr, session_addr) = fixture();

    // Mismatched top-level keys.
    let mismatch = EditRequest::parse(&json!({
        "changes": { "title": { "text": "X" } },
        "before": { "title": { "text": "Stream", "manual": false }, "goal": null }
    }))
    .unwrap();
    assert!(edit_stream(&store, &stream_addr, &mismatch).is_err());

    // Mismatched reference keys.
    let ref_mismatch = EditRequest::parse(&json!({
        "changes": { "references": { "https://github.com/o/r/pull/43": { "status": "fulfilled" } } },
        "before": { "references": {} }
    }))
    .unwrap();
    assert!(edit_session(&store, &session_addr, &ref_mismatch).is_err());

    // Read-only, computed, and unknown fields all fail without writing.
    for field in ["id", "host_id", "revision", "status", "sessions", "bogus"] {
        let req = EditRequest::parse(&json!({
            "changes": { (field): "x" }
        }))
        .unwrap();
        assert!(edit_stream(&store, &stream_addr, &req).is_err(), "{field}");
    }
    for field in [
        "id",
        "host_id",
        "harness",
        "artifacts",
        "document_files",
        "bogus",
    ] {
        let req = EditRequest::parse(&json!({
            "changes": { (field): "x" }
        }))
        .unwrap();
        assert!(
            edit_session(&store, &session_addr, &req).is_err(),
            "{field}"
        );
    }

    // Null reference entry fails.
    let null_ref = EditRequest::parse(&json!({
        "changes": { "references": { "https://github.com/o/r/pull/43": null } }
    }))
    .unwrap();
    assert!(edit_session(&store, &session_addr, &null_ref).is_err());
}

#[test]
fn concurrent_edit_writes_unchanged_units_and_reports_not_written_with_current_values() {
    let (store, _stream_addr, session_addr) = fixture();

    // Inject a concurrent person edit to `title` and the PR reference right before the report saves.
    let target = session_addr.clone();
    store.inject_next_save(move |records| {
        let Record::Session(s) = records.get_mut(&target).unwrap() else {
            unreachable!()
        };
        s.title = Title::new("Human Title", true);
        let pr = s
            .references
            .get_mut("https://github.com/o/r/pull/43")
            .unwrap();
        pr.name = "Human PR Name".into();
        pr.manual = true;
    });

    let report = EditRequest::parse(&json!({
        "changes": {
            "title": { "text": "Auto Title" },
            "recap": { "text": "In review", "short": "review" },
            "references": {
                "https://github.com/o/r/pull/43": { "status": "fulfilled" },
                "https://github.com/o/r/issues/9": {
                    "kind": "issue",
                    "name": "Issue 9",
                    "purpose": "Dependency",
                    "role": "dependency",
                    "status": "active",
                    "manual": false
                }
            }
        },
        "before": {
            "title": { "text": "Session", "manual": false },
            "recap": { "text": "Drafting", "short": "draft" },
            "references": {
                "https://github.com/o/r/pull/43": {
                    "kind": "pr",
                    "name": "PR 43",
                    "purpose": "Implements it",
                    "role": "deliverable",
                    "status": "active",
                    "manual": false
                },
                "https://github.com/o/r/issues/9": null
            }
        }
    }))
    .unwrap();

    let res = edit_session(&store, &session_addr, &report).unwrap();
    assert!(!res.is_complete());
    assert!(res.written);

    // `recap` and `issues/9` landed; `title` and `pull/43` were not written and report current saved values.
    assert!(res.not_written.get("recap").is_none());
    assert_eq!(
        res.not_written["title"],
        json!({ "text": "Human Title", "manual": true })
    );
    assert!(
        res.not_written["references"]
            .get("https://github.com/o/r/issues/9")
            .is_none()
    );
    assert_eq!(
        res.not_written["references"]["https://github.com/o/r/pull/43"]["name"],
        "Human PR Name"
    );

    let Record::Session(after) = store.get(&session_addr).unwrap() else {
        unreachable!()
    };
    assert_eq!(after.title, Title::new("Human Title", true));
    assert_eq!(
        after.recap,
        Some(TextPair::new("In review", Some("review".into())))
    );
    assert!(
        after
            .references
            .contains_key("https://github.com/o/r/issues/9")
    );

    // Replaying the exact same report returns the exact same `not_written` and writes nothing.
    let replay = edit_session(&store, &session_addr, &report).unwrap();
    assert_eq!(replay.not_written, res.not_written);
    assert!(!replay.written);
}

#[test]
fn dropped_reference_blocks_report_re_add_and_restores_preserving_metadata() {
    let (store, _stream_addr, session_addr) = fixture();

    // Person drops the PR reference.
    let drop_req = EditRequest::parse(&json!({
        "changes": { "references": { "https://github.com/o/r/pull/43": { "status": "dropped", "manual": true } } },
        "before": {
            "references": {
                "https://github.com/o/r/pull/43": {
                    "kind": "pr",
                    "name": "PR 43",
                    "purpose": "Implements it",
                    "role": "deliverable",
                    "status": "active",
                    "manual": false
                }
            }
        }
    }))
    .unwrap();
    assert!(
        edit_session(&store, &session_addr, &drop_req)
            .unwrap()
            .is_complete()
    );

    // Report tries to add it back with `before: null` -> refused in `not_written`.
    let re_add = EditRequest::parse(&json!({
        "changes": {
            "references": {
                "https://github.com/o/r/pull/43": {
                    "kind": "pr",
                    "name": "PR 43",
                    "purpose": "Implements it",
                    "role": "deliverable",
                    "status": "active",
                    "manual": false
                }
            }
        },
        "before": { "references": { "https://github.com/o/r/pull/43": null } }
    }))
    .unwrap();
    let res = edit_session(&store, &session_addr, &re_add).unwrap();
    assert!(!res.is_complete());
    assert!(!res.written);
    assert_eq!(
        res.not_written["references"]["https://github.com/o/r/pull/43"]["status"],
        "dropped"
    );

    // Restoring with `before` keeps saved name/purpose/role.
    let restore = EditRequest::parse(&json!({
        "changes": { "references": { "https://github.com/o/r/pull/43": { "status": "active" } } },
        "before": {
            "references": {
                "https://github.com/o/r/pull/43": {
                    "kind": "pr",
                    "name": "PR 43",
                    "purpose": "Implements it",
                    "role": "deliverable",
                    "status": "dropped",
                    "manual": true
                }
            }
        }
    }))
    .unwrap();
    let restored = edit_session(&store, &session_addr, &restore).unwrap();
    assert!(restored.is_complete());
    let Record::Session(after) = store.get(&session_addr).unwrap() else {
        unreachable!()
    };
    assert_eq!(
        after.references["https://github.com/o/r/pull/43"].status,
        ReferenceStatus::Active
    );
    assert_eq!(
        after.references["https://github.com/o/r/pull/43"].name,
        "PR 43"
    );
}

#[test]
fn stream_id_edit_needs_an_existing_same_host_stream() {
    let (store, _stream_addr, session_addr) = fixture();

    let bad_move = EditRequest::parse(&json!({
        "changes": { "stream_id": "missing" },
        "before": { "stream_id": "s1" }
    }))
    .unwrap();
    assert!(edit_session(&store, &session_addr, &bad_move).is_err());

    let good_move = EditRequest::parse(&json!({
        "changes": { "stream_id": "s2" },
        "before": { "stream_id": "s1" }
    }))
    .unwrap();
    let res = edit_session(&store, &session_addr, &good_move).unwrap();
    assert!(res.is_complete());
    assert!(res.written);
    let Record::Session(after) = store.get(&session_addr).unwrap() else {
        unreachable!()
    };
    assert_eq!(after.stream_id, "s2");
}

#[test]
fn same_value_written_concurrently_omits_field_from_not_written() {
    let (store, stream_addr, _session_addr) = fixture();
    let target = stream_addr.clone();
    store.inject_next_save(move |records| {
        let Record::Stream(s) = records.get_mut(&target).unwrap() else {
            unreachable!()
        };
        s.title = Title::new("Agreed", false);
    });
    let req = EditRequest::parse(&json!({
        "changes": { "title": { "text": "Agreed" } },
        "before": { "title": { "text": "Stream", "manual": false } }
    }))
    .unwrap();
    let res = edit_stream(&store, &stream_addr, &req).unwrap();
    assert!(res.is_complete());
    assert!(!res.written);
}
