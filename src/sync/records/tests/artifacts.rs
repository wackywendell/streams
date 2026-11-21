use super::*;
use crate::{
    application::artifacts::{
        WriteArtifact, delete_artifact, list_session_artifacts, read_artifact, write_artifact,
    },
    artifacts::ArtifactKey,
};

#[test]
fn artifact_content_and_session_metadata_sync_to_a_read_only_foreign_replica() {
    let directory = tempfile::tempdir().unwrap();
    let remote = directory.path().join("remote");
    bare(&remote);
    let a = directory.path().join("a");
    let b = directory.path().join("b");
    let a_records = setup(&a, "a-host");
    let b_records = setup(&b, "b-host");
    let a_sync = transport(&a, "a-host", Some(&remote));
    let b_sync = transport(&b, "b-host", Some(&remote));
    let key = ArtifactKey {
        host: "a-host".into(),
        harness: "pi".into(),
        session: "native".into(),
        name: "assessment.md".into(),
    };
    write_artifact(
        &a_records,
        "a-host",
        &WriteArtifact {
            harness: "pi".into(),
            session_id: "native".into(),
            name: key.name.clone(),
            description: "Findings".into(),
            content: Some("useful output\n".into()),
        },
    )
    .unwrap();
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(b_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(
        read_artifact(&b_records, &key).unwrap().content,
        "useful output\n"
    );
    assert_eq!(
        list_session_artifacts(&b_records, "a-host", "pi", "native")
            .unwrap()
            .len(),
        1
    );
    assert!(delete_artifact(&b_records, &key).is_err());
    let path = crate::file_store::records::relative_artifact_path(&key).unwrap();
    assert_eq!(fs::read(b.join(path)).unwrap(), b"useful output\n");
    delete_artifact(&a_records, &key).unwrap();
    assert_eq!(a_sync.sync().sharing, SharingStatus::Confirmed);
    assert_eq!(b_sync.sync().sharing, SharingStatus::Confirmed);
    assert!(read_artifact(&b_records, &key).is_err());
}
