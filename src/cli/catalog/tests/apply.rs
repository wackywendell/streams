use std::sync::{Arc, Mutex};

use streams::{
    model::TitleDefaults,
    storage::{
        StorageError,
        records::{
            RecordAddress, RecordChange, RecordRead, RecordReader, RecordRequest, RecordSave,
            RecordStore,
        },
    },
    yaml_store::records::{RecordIoEvent, YamlRecordStore, relative_record_path},
};

use super::*;

fn request(before: Value) -> Value {
    json!({
        "version":2,"operation_id":"captured-update","catalog_source":"logical-source",
        "actor":"user","kind":"stream","before":before,"changes":{"goal":"Captured"},
        "title_default":"automatic","inputs":[],
    })
}

#[test]
fn captured_apply_is_target_local_and_retains_the_original_decision_without_publication() {
    for unrelated in [0, 64] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let events = Arc::new(Mutex::new(Vec::new()));
        let observed = events.clone();
        let store = YamlRecordStore::new(root, "test-host")
            .unwrap()
            .with_io_observer(move |event| observed.lock().unwrap().push(event));
        store.initialize().unwrap();
        let record = Record::Stream(
            Stream::new("effort", "Work", Some("Original".into()), "test-host").unwrap(),
        );
        let address = RecordAddress::for_record(&record);
        let path = root.join(relative_record_path(&address).unwrap());
        store
            .save::<StorageError>(
                &RecordRequest::new(address.clone(), "create", &json!({})).unwrap(),
                |_, _| {
                    Ok(RecordChange {
                        record: record.clone(),
                        outcome: json!({"outcome":"changes"}).as_object().unwrap().clone(),
                    })
                },
            )
            .unwrap();
        // A broken transport configuration cannot block an ordinary local save.
        std::fs::write(root.join(".catalog-local/transport.json"), "not JSON").unwrap();
        let others = root.join("hosts/other/streams");
        std::fs::create_dir_all(&others).unwrap();
        for index in 0..unrelated {
            std::fs::write(others.join(format!("broken-{index}.yaml")), "malformed").unwrap();
        }
        events.lock().unwrap().clear();
        let captured = request(record_to_data(&record));
        let first = ok(apply(
            &store,
            &captured.to_string(),
            "logical-source",
            TitleDefaults::default(),
        ));
        assert_eq!(first["operation_id"], "captured-update");
        assert_eq!(first["save"]["domain"]["outcome"], "changes");
        assert_eq!(first["save"]["local"], "persisted");
        assert_eq!(first["stream"]["goal"], "Captured");
        assert!(first.get("publication").is_none());
        assert!(first["save"].get("sharing").is_none());
        assert_eq!(
            events
                .lock()
                .unwrap()
                .iter()
                .filter(|event| event.is_access())
                .cloned()
                .collect::<Vec<_>>(),
            vec![
                RecordIoEvent::Lock(root.join(".catalog.lock")),
                RecordIoEvent::Read(root.join(".catalog-local/records.json")),
                RecordIoEvent::Read(path.clone()),
            ]
        );
        let accepted = std::fs::read(&path).unwrap();
        let retry = ok(apply(
            &store,
            &captured.to_string(),
            "logical-source",
            TitleDefaults::default(),
        ));
        assert_eq!(retry["save"]["domain"], first["save"]["domain"]);
        assert_eq!(retry["save"]["local"], "unchanged");
        assert_eq!(retry["stream"], first["stream"]);
        assert_eq!(std::fs::read(&path).unwrap(), accepted);
        let mut wrong_source = captured.clone();
        wrong_source["catalog_source"] = json!("wrong");
        assert!(
            apply(
                &store,
                &wrong_source.to_string(),
                "logical-source",
                TitleDefaults::default()
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), accepted);
        let mut repositories = captured.clone();
        repositories["changes"] =
            json!({"repositories":{"app":{"url":"https://example.test/app.git"}}});
        assert!(
            apply(
                &store,
                &repositories.to_string(),
                "logical-source",
                TitleDefaults::default()
            )
            .is_err(),
            "the direct human repository edit must not widen the agent wire"
        );
        assert_eq!(std::fs::read(&path).unwrap(), accepted);
        let mut stale = captured;
        stale["operation_id"] = json!("new-stale-intent");
        stale["changes"] = json!({"goal":"Different"});
        let refused = apply(
            &store,
            &stale.to_string(),
            "logical-source",
            TitleDefaults::default(),
        )
        .unwrap();
        assert_eq!(refused.code, 2);
        assert_eq!(refused.value["save"]["domain"]["outcome"], "conflict");
        assert_eq!(refused.value["stream"], first["stream"]);
    }
}

struct FailedWrite(bool);
impl RecordStore for FailedWrite {
    fn with_reader<T, E: From<StorageError>>(
        &self,
        _: impl FnOnce(&dyn RecordReader) -> std::result::Result<T, E>,
    ) -> std::result::Result<T, E> {
        panic!("apply must not read again after an uncertain save");
    }
    fn save<E: From<StorageError>>(
        &self,
        _: &RecordRequest,
        _: impl FnOnce(&dyn RecordReader, Option<&RecordRead>) -> std::result::Result<RecordChange, E>,
    ) -> std::result::Result<RecordSave, E> {
        let error = if self.0 {
            StorageError::uncertain("replacement flush failed")
        } else {
            StorageError::new("replacement flush failed")
        };
        Err(error.into())
    }
}

#[test]
fn apply_retains_post_replacement_uncertainty_but_not_a_definite_failure_as_a_save() {
    let record = Record::Stream(
        Stream::new("effort", "Work", Some("Original".into()), "test-host").unwrap(),
    );
    let captured = request(record_to_data(&record)).to_string();
    let uncertain = apply(
        &FailedWrite(true),
        &captured,
        "logical-source",
        TitleDefaults::default(),
    )
    .unwrap();
    assert_eq!(uncertain.code, 2);
    assert_eq!(uncertain.value["operation_id"], "captured-update");
    assert_eq!(uncertain.value["save"]["local"], "uncertain");
    assert!(uncertain.value["save"]["domain"].is_null());
    assert!(uncertain.value.get("stream").is_none());
    assert!(
        apply(
            &FailedWrite(false),
            &captured,
            "logical-source",
            TitleDefaults::default()
        )
        .is_err()
    );
}
