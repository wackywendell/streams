//! Locked local-source evidence, independent of Git candidate construction.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::storage::{Snapshot, Transaction};
use crate::sync::local;
use crate::sync::state::{RepairRequired, State};
use crate::yaml_store::YamlStore;

use super::repair;

type Files = BTreeMap<String, Vec<u8>>;
type Bookkeeping = BTreeMap<String, Option<String>>;

pub(super) struct Source {
    pub(super) snapshot: Snapshot,
    pub(super) files: Files,
    pub(super) bookkeeping: Bookkeeping,
}

impl Source {
    pub(super) fn capture(root: &Path, tx: &impl Transaction) -> Result<Self, RepairRequired> {
        let state = State::from_journal(tx.journal());
        local::ready(tx, &state)?;
        let snapshot = tx.read().map_err(repair)?;
        let files = record_files(root, &snapshot)?;
        let bookkeeping = bookkeeping(root, &state)?;
        Ok(Self {
            snapshot,
            files,
            bookkeeping,
        })
    }

    pub(super) fn check_unchanged(
        &self,
        root: &Path,
        tx: &impl Transaction,
    ) -> Result<(), RepairRequired> {
        let state = State::from_journal(tx.journal());
        local::ready(tx, &state)?;
        if tx.read().map_err(repair)? != self.snapshot
            || record_files(root, &self.snapshot)? != self.files
        {
            return Err(repair(
                "catalog records changed during adoption preparation",
            ));
        }
        if bookkeeping(root, &state)? != self.bookkeeping {
            return Err(repair(
                "catalog bookkeeping changed during adoption preparation",
            ));
        }
        Ok(())
    }
}

fn record_files(root: &Path, snapshot: &Snapshot) -> Result<Files, RepairRequired> {
    let mut files = BTreeMap::new();
    for record in snapshot.catalog.records() {
        let path = YamlStore::new(root).record_path(&record);
        files.insert(
            path.to_str()
                .ok_or_else(|| repair("record path must be UTF-8"))?
                .into(),
            fs::read(root.join(path)).map_err(repair)?,
        );
    }
    Ok(files)
}

fn bookkeeping(root: &Path, state: &State<'_>) -> Result<Bookkeeping, RepairRequired> {
    let mut before = BTreeMap::new();
    for name in ["cli.json", "local.json", "receipts.json"] {
        // Validate the journal object before retaining its exact original text.
        let present = state.read(name)?.is_some();
        before.insert(
            name.into(),
            if present {
                Some(fs::read_to_string(root.join(".catalog-sync").join(name)).map_err(repair)?)
            } else {
                None
            },
        );
    }
    Ok(before)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codec::record_to_data;
    use crate::model::{Record, Session, Stream};
    use crate::yaml_store::relative_record_path;
    use serde_json::json;

    fn seed(root: &Path) {
        let record = Record::Stream(Stream::new("work", "Original", None, "host").unwrap());
        let data = json!({"version": 1, "revision": "fixture", "kind": record.key().kind(), "record": record_to_data(&record)});
        fs::create_dir_all(root.join("streams")).unwrap();
        fs::create_dir_all(root.join(".catalog-sync")).unwrap();
        // These tests observe existing files; durable save behavior has its own suite.
        fs::write(
            root.join(relative_record_path(&record)),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
    }

    fn write_bookkeeping(root: &Path, name: &str) {
        let data = match name {
            "local.json" => json!({"version": 1, "creations": {}}),
            "receipts.json" => json!({"version": 1, "operations": {}}),
            "cli.json" => json!({"version": 1, "host": "host", "strategy": "none"}),
            _ => unreachable!(),
        };
        fs::write(
            root.join(".catalog-sync").join(name),
            serde_json::to_vec(&data).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn unchanged_observation_preserves_exact_source() {
        let dir = tempfile::tempdir().unwrap();
        YamlStore::new(dir.path())
            .with_transaction::<_, RepairRequired>(|tx| {
                seed(dir.path());
                for name in ["local.json", "cli.json", "receipts.json"] {
                    write_bookkeeping(dir.path(), name);
                }
                let source = Source::capture(dir.path(), tx)?;
                assert_eq!(source.snapshot, tx.read()?);
                assert_eq!(
                    source.files["streams/work.yaml"],
                    fs::read(dir.path().join("streams/work.yaml")).unwrap()
                );
                for (name, contents) in &source.bookkeeping {
                    assert_eq!(
                        contents.as_ref().unwrap(),
                        &fs::read_to_string(dir.path().join(".catalog-sync").join(name)).unwrap()
                    );
                }
                source.check_unchanged(dir.path(), tx)
            })
            .unwrap();
    }

    #[test]
    fn legacy_session_capture_preserves_existing_path_and_exact_bytes() {
        let dir = tempfile::tempdir().unwrap();
        YamlStore::new(dir.path())
            .with_transaction::<_, RepairRequired>(|tx| {
                seed(dir.path());
                let record = Record::Session(
                    Session::new("session", "host", "work", "pi", "native", "Session").unwrap(),
                );
                let path = "sessions/work/host/session.yaml";
                fs::create_dir_all(dir.path().join("sessions/work/host")).unwrap();
                let mut bytes = serde_json::to_vec_pretty(&json!({
                    "version": 1, "revision": "legacy", "kind": "session",
                    "record": record_to_data(&record)
                }))
                .unwrap();
                bytes.extend_from_slice(b"\n# preserve this legacy formatting\n");
                fs::write(dir.path().join(path), &bytes).unwrap();
                let source = Source::capture(dir.path(), tx)?;
                assert_eq!(source.files[path], bytes);
                assert!(!source.files.contains_key("conversations/host/session.yaml"));
                source.check_unchanged(dir.path(), tx)?;
                assert_eq!(fs::read(dir.path().join(path)).unwrap(), bytes);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn raw_record_byte_drift_rejects_unchanged_decoded_catalog() {
        let dir = tempfile::tempdir().unwrap();
        YamlStore::new(dir.path())
            .with_transaction::<_, RepairRequired>(|tx| {
                seed(dir.path());
                let source = Source::capture(dir.path(), tx)?;
                let path = dir.path().join("streams/work.yaml");
                let mut bytes = fs::read(&path).unwrap();
                bytes.extend_from_slice(b"\n# changed formatting\n");
                fs::write(path, bytes).unwrap();
                assert_eq!(tx.read()?.catalog, source.snapshot.catalog);
                assert_eq!(
                    source
                        .check_unchanged(dir.path(), tx)
                        .unwrap_err()
                        .to_string(),
                    "catalog records changed during adoption preparation"
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn bookkeeping_exact_contents_and_presence_drift() {
        for name in ["local.json", "cli.json", "receipts.json"] {
            for change in ["contents", "appeared", "disappeared"] {
                let dir = tempfile::tempdir().unwrap();
                YamlStore::new(dir.path())
                    .with_transaction::<_, RepairRequired>(|tx| {
                        seed(dir.path());
                        if change != "appeared" {
                            write_bookkeeping(dir.path(), name);
                        }
                        let source = Source::capture(dir.path(), tx)?;
                        source.check_unchanged(dir.path(), tx)?;
                        let path = dir.path().join(".catalog-sync").join(name);
                        match change {
                            "contents" => {
                                let before = tx.journal().read(name)?;
                                let mut bytes = fs::read(&path).unwrap();
                                bytes.push(b'\n');
                                fs::write(path, bytes).unwrap();
                                assert_eq!(tx.journal().read(name)?, before);
                            }
                            "appeared" => write_bookkeeping(dir.path(), name),
                            "disappeared" => fs::remove_file(path).unwrap(),
                            _ => unreachable!(),
                        }
                        assert_eq!(
                            source
                                .check_unchanged(dir.path(), tx)
                                .unwrap_err()
                                .to_string(),
                            "catalog bookkeeping changed during adoption preparation",
                            "{name}: {change}"
                        );
                        Ok(())
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn corruption_fails_closed_at_capture_and_recheck() {
        for name in [
            "streams/work.yaml",
            ".catalog-sync/local.json",
            ".catalog-sync/cli.json",
            ".catalog-sync/receipts.json",
        ] {
            for corrupt in ["not valid data", "{\"version\":999}"] {
                let dir = tempfile::tempdir().unwrap();
                YamlStore::new(dir.path())
                    .with_transaction::<_, RepairRequired>(|tx| {
                        seed(dir.path());
                        let source = Source::capture(dir.path(), tx)?;
                        fs::write(dir.path().join(name), corrupt).unwrap();
                        assert!(
                            Source::capture(dir.path(), tx).is_err(),
                            "{name}: {corrupt}"
                        );
                        assert!(
                            source.check_unchanged(dir.path(), tx).is_err(),
                            "{name}: {corrupt}"
                        );
                        Ok(())
                    })
                    .unwrap();
            }
        }
    }
}
