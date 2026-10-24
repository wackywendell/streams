use super::super::Envelope;
use super::*;
use crate::{
    codec::record_to_data,
    model::{Session, Stream},
};

fn bytes(record: &Record) -> Vec<u8> {
    serde_json::to_vec_pretty(&Envelope {
        version: 2,
        revision: uuid::Uuid::new_v4().to_string(),
        kind: record.kind(),
        record: record_to_data(record),
        receipts: BTreeMap::new(),
    })
    .unwrap()
}
fn tree() -> BTreeMap<PathBuf, Vec<u8>> {
    let mut tree = BTreeMap::new();
    for host in ["host", "other"] {
        for record in [
            Record::Stream(Stream::new("same-id", host, None, host).unwrap()),
            Record::Session(
                Session::new(
                    Session::id_for(host, "pi", "native"),
                    host,
                    "same-id",
                    "pi",
                    "native",
                    "Work",
                )
                .unwrap(),
            ),
        ] {
            let address = RecordAddress::for_record(&record);
            tree.insert(relative_record_path(&address).unwrap(), bytes(&record));
        }
    }
    tree
}

#[test]
fn canonical_paths_decode_exact_host_qualified_identities() {
    for kind in [RecordKind::Stream, RecordKind::Session, RecordKind::Host] {
        let host = "høst/encoded";
        let id = if kind == RecordKind::Host {
            host
        } else {
            "ID /😀"
        };
        let address = RecordAddress::new(host, kind, id).unwrap();
        assert_eq!(
            address_from_relative_path(&relative_record_path(&address).unwrap()).unwrap(),
            address
        );
    }
    for path in [
        "/hosts/host/streams/a.yaml",
        "hosts/host/streams/../a.yaml",
        "hosts/host/streams/./a.yaml",
        "hosts//host/streams/a.yaml",
        "hosts/host/streams/%4a.yaml",
        "hosts/host/streams/UPPER.yaml",
        "hosts/host/streams/a.json",
        "streams/a.yaml",
        "hosts/host/a.yaml",
    ] {
        assert!(
            address_from_relative_path(Path::new(path)).is_err(),
            "{path}"
        );
    }
}

#[test]
fn tree_validation_allows_equal_stream_ids_on_different_hosts_but_requires_local_relationships() {
    let mut tree = tree();
    assert_eq!(decode_record_tree(&tree).unwrap().len(), 4);
    tree.remove(
        &relative_record_path(&RecordAddress::new("host", RecordKind::Stream, "same-id").unwrap())
            .unwrap(),
    );
    assert!(
        decode_record_tree(&tree)
            .unwrap_err()
            .to_string()
            .contains("same-host Stream is missing")
    );
}

#[test]
fn explicit_discovery_uses_the_same_codec_and_does_not_read_legacy_history() {
    let directory = tempfile::tempdir().unwrap();
    let store = YamlRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    let files = tree();
    for (path, bytes) in &files {
        let path = directory.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    fs::create_dir_all(directory.path().join(".catalog-sync")).unwrap();
    fs::write(
        directory.path().join(".catalog-sync/operations"),
        "not replayable",
    )
    .unwrap();
    let records = store.inventory().unwrap();
    assert_eq!(records.len(), 4);
    assert_eq!(records, decode_record_tree(&files).unwrap());
}

#[test]
fn explicit_discovery_rejects_symlinks_unknown_directories_and_malformed_records() {
    use std::os::unix::fs::symlink;
    for failure in ["symlink", "directory", "malformed"] {
        let directory = tempfile::tempdir().unwrap();
        let store = YamlRecordStore::new(directory.path(), "host").unwrap();
        store.initialize().unwrap();
        let target = directory.path().join("hosts/host/streams/a.yaml");
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        match failure {
            "symlink" => symlink(directory.path().join("missing"), &target).unwrap(),
            "directory" => {
                fs::create_dir_all(directory.path().join("hosts/host/unexpected")).unwrap()
            }
            _ => fs::write(&target, "not a record").unwrap(),
        }
        assert!(store.inventory().is_err(), "{failure}");
    }
}
