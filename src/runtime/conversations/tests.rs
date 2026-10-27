use super::*;

#[test]
fn first_line_harnesses_read_one_small_buffer_not_the_message_body() {
    struct Counted<'a> {
        bytes: &'a [u8],
        transferred: usize,
    }
    impl Read for Counted<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            let count = self.bytes.read(buffer)?;
            self.transferred += count;
            Ok(count)
        }
    }
    for (harness, header) in [
        (Harness::Pi, "{\"type\":\"session\",\"id\":\"target\"}\n"),
        (
            Harness::Codex,
            "{\"type\":\"session_meta\",\"payload\":{\"id\":\"target\"}}\n",
        ),
    ] {
        for body in [LIMIT, LIMIT * 16] {
            let bytes = format!("{header}{}", "x".repeat(body));
            let mut reader = Counted {
                bytes: bytes.as_bytes(),
                transferred: 0,
            };
            let prefix = read_identity_prefix(harness, &mut reader).unwrap();
            assert_eq!(prefix, header.as_bytes());
            assert_eq!(reader.transferred, 256);
            assert_eq!(identity_prefix(harness, &prefix).unwrap(), "target");
        }
        let oversized = vec![b'x'; LIMIT * 2];
        let mut reader = Counted {
            bytes: &oversized,
            transferred: 0,
        };
        let prefix = read_identity_prefix(harness, &mut reader).unwrap();
        assert_eq!(reader.transferred, LIMIT + 1);
        assert!(identity_prefix(harness, &prefix).is_err());
    }
    let bytes = b"{\"sessionId\":\"target\"}\n{\"sessionId\":\"other\"}\n";
    let prefix = read_identity_prefix(Harness::Claude, &bytes[..]).unwrap();
    assert_eq!(prefix, bytes);
    assert!(identity_prefix(Harness::Claude, &prefix).is_err());
}

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
