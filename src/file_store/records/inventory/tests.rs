use super::*;
use crate::{
    codec::record_to_data,
    model::{Session, Stream},
};

fn bytes(record: &Record) -> Vec<u8> {
    serde_json::to_vec_pretty(&record_to_data(record)).unwrap()
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
        "/hosts/host/streams/a.json",
        "hosts/host/streams/../a.json",
        "hosts/host/streams/./a.json",
        "hosts//host/streams/a.json",
        "hosts/host/streams/%4a.json",
        "hosts/host/streams/UPPER.json",
        "hosts/host/streams/a.yaml",
        "streams/a.json",
        "hosts/host/a.json",
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
    let store = FileRecordStore::new(directory.path(), "host").unwrap();
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
        let store = FileRecordStore::new(directory.path(), "host").unwrap();
        store.initialize().unwrap();
        let target = directory.path().join("hosts/host/streams/a.json");
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

#[test]
fn listings_name_undecodable_records_and_still_fail_on_unexpected_layout() {
    use std::os::unix::fs::symlink;
    let directory = tempfile::tempdir().unwrap();
    let store = FileRecordStore::new(directory.path(), "host").unwrap();
    store.initialize().unwrap();
    for (path, bytes) in &tree() {
        let path = directory.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let broken = Path::new("hosts/host/streams/broken.json");
    fs::write(directory.path().join(broken), "not a record").unwrap();

    let listing = store.listing().unwrap();
    assert_eq!(listing.records.len(), 4);
    assert_eq!(listing.unreadable.len(), 1);
    assert_eq!(listing.unreadable[0].kind, RecordKind::Stream);
    assert_eq!(listing.unreadable[0].path, broken);
    let streams = store.host_streams("host").unwrap();
    assert_eq!(streams.records.len(), 1);
    assert_eq!(streams.unreadable, listing.unreadable);
    assert_eq!(
        streams.complete().unwrap_err().to_string(),
        listing.unreadable[0].to_string()
    );
    assert!(store.host_sessions("host").unwrap().unreadable.is_empty());

    symlink(
        directory.path().join("missing"),
        directory.path().join("hosts/host/streams/link.json"),
    )
    .unwrap();
    assert!(store.listing().is_err());
    assert!(store.host_streams("host").is_err());
}
