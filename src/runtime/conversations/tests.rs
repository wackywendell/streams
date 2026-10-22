use super::*;

#[test]
fn discovery_counts_all_jsonl_files_across_directories_before_matching_hints() {
    let mut discovery = Discovery::new("target");
    for index in 0..DISCOVERY_LIMIT {
        assert!(
            !discovery
                .candidate(Path::new("/first/not-json.txt"))
                .unwrap()
        );
        assert!(
            !discovery
                .candidate(&PathBuf::from(format!("/first/unrelated-{index}.jsonl")))
                .unwrap()
        );
    }
    let error = discovery
        .candidate(Path::new("/second/target.jsonl"))
        .unwrap_err();
    assert!(error.to_string().contains("discovery limit"), "{error}");
}

#[test]
fn discovery_requires_exact_identity_and_deduplicates_verified_canonical_paths() {
    assert!(matches!(
        Discovery::new("target").finish(),
        Err(RuntimeError::NotFound(_))
    ));
    let mut discovery = Discovery::new("target");
    assert!(
        discovery
            .candidate(Path::new("/target-hint.jsonl"))
            .unwrap()
    );
    discovery.record("not-target", "/wrong.jsonl".into());
    assert!(discovery.matches.is_empty());
    discovery.record("target", "/canonical.jsonl".into());
    discovery.record("target", "/canonical.jsonl".into());
    assert_eq!(discovery.finish().unwrap(), Path::new("/canonical.jsonl"));
    let mut ambiguous = Discovery::new("target");
    ambiguous.record("target", "/second.jsonl".into());
    ambiguous.record("target", "/first.jsonl".into());
    match ambiguous.finish().unwrap_err() {
        RuntimeError::Ambiguous { candidates, .. } => {
            assert_eq!(candidates, ["/first.jsonl", "/second.jsonl"])
        }
        error => panic!("{error}"),
    }
}
