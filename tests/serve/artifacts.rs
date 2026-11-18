use super::*;

fn enroll(f: &Fixture, native: &str) -> (String, String) {
    let stream = f.ok(&["create", "Artifact work"], None)["stream"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let session = f.ok(
        &[
            "session",
            "attach",
            &stream,
            "--harness",
            "pi",
            "--session-id",
            native,
            "--title",
            "Output producer",
        ],
        None,
    )["session"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    (stream, session)
}

#[test]
fn cli_artifact_crud_reads_files_stdin_and_preserves_stdout_bytes() {
    let f = Fixture::new();
    let (stream, session) = enroll(&f, "pi-session");
    let written = f.ok(
        &[
            "artifact",
            "write",
            "assessment.md",
            "--session-id",
            &session,
            "--description",
            "Findings",
        ],
        Some("# Assessment\nno final newline"),
    );
    let reference = written["artifact"]["reference"].as_str().unwrap();
    let raw = f
        .command()
        .args(["artifact", "read", reference])
        .output()
        .unwrap();
    assert!(raw.status.success(), "{raw:?}");
    assert_eq!(raw.stdout, b"# Assessment\nno final newline");
    let listed = f.ok(&["artifact", "list", "--session-id", &session], None);
    assert_eq!(listed["artifacts"].as_array().unwrap().len(), 1);
    assert_eq!(
        f.ok(&["artifact", "list", "--stream-id", &stream], None),
        listed
    );
    let path = f.directory.path().join("source.md");
    fs::write(&path, "from file\n").unwrap();
    f.ok(
        &[
            "artifact",
            "write",
            "assessment.md",
            "--session-id",
            &session,
            "--description",
            "Updated",
            "--file",
            path.to_str().unwrap(),
        ],
        None,
    );
    assert_eq!(
        f.ok(&["artifact", "read", reference], None)["content"],
        "from file\n"
    );
    f.ok(
        &[
            "artifact",
            "write",
            "assessment.md",
            "--session-id",
            &session,
            "--description",
            "Metadata only",
            "--description-only",
        ],
        None,
    );
    assert_eq!(
        f.ok(&["artifact", "read", reference], None)["artifact"]["description"],
        "Metadata only"
    );
    f.ok(&["artifact", "delete", reference], None);
    assert_eq!(f.cli(&["artifact", "read", reference], None).0, 2);
    assert!(
        f.ok(&["artifact", "list", "--session-id", &session], None)["artifacts"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(f.cli(&["artifact", "list"], None).0, 2);
}

#[test]
fn artifact_view_passes_only_content_to_a_secure_read_only_pager() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let (_, session) = enroll(&f, "pi-session");
    let written = f.ok(
        &[
            "artifact",
            "write",
            "assessment.md",
            "--session-id",
            &session,
            "--description",
            "Findings",
        ],
        Some("assessment output\n"),
    );
    let bin = f.directory.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let pager = bin.join("less");
    fs::write(&pager, "#!/bin/sh\nset -eu\n[ \"$LESSSECURE\" = 1 ]\n[ -z \"$LESS\" ]\n[ -z \"${LESSOPEN+x}\" ]\n[ \"$#\" = 1 ]\n[ \"$1\" = -- ]\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done\nprintf 'viewer done\\n'\n").unwrap();
    fs::set_permissions(&pager, fs::Permissions::from_mode(0o700)).unwrap();
    let mut command = f.command();
    command
        .args([
            "artifact",
            "view",
            written["artifact"]["reference"].as_str().unwrap(),
        ])
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("LESS", "--log-file=not-allowed")
        .env("LESSOPEN", "not-allowed");
    let mut terminal = crate::support::pty::Pty::spawn(command, false);
    terminal.until(|terminal| terminal.screen().contains("viewer done"));
    assert!(terminal.screen().contains("assessment output"));
    assert!(terminal.finish().success());
}

#[test]
fn child_artifact_read_pages_use_utf8_boundaries_and_progress() {
    let f = Fixture::new();
    enroll(&f, "pi-session");
    let mut child = f.start();
    child.request(1, json!({"kind":"artifact","request":{"action":"write","name":"unicode.md","description":"Unicode","text":"é😀汉"}}));
    let written = child.response(1);
    let reference = written["outcome"]["result"]["artifact"]["reference"]
        .as_str()
        .unwrap();
    for (id, offset, expected, next) in [
        (2, 0, "é", Some(2)),
        (3, 2, "😀", Some(6)),
        (4, 6, "汉", None),
    ] {
        child.request(id, json!({"kind":"artifact","request":{"action":"read","reference":reference,"offset":offset,"limit":4}}));
        let page = child.response(id);
        assert_eq!(page["outcome"]["result"]["content"], expected);
        assert_eq!(page["outcome"]["result"]["next_offset"], json!(next));
        assert_eq!(page["outcome"]["result"]["total_bytes"], 9);
    }
    child.request(5, json!({"kind":"artifact","request":{"action":"read","reference":reference,"offset":3,"limit":4}}));
    assert_eq!(child.response(5)["outcome"]["status"], "error");
    let content = "x".repeat(40000);
    child.request(6, json!({"kind":"artifact","request":{"action":"write","name":"large.md","description":"Large output","text":content}}));
    let written = child.response(6);
    let reference = written["outcome"]["result"]["artifact"]["reference"]
        .as_str()
        .unwrap();
    child.request(
        7,
        json!({"kind":"artifact","request":{"action":"read","reference":reference}}),
    );
    let page = child.response(7);
    assert_eq!(
        page["outcome"]["result"]["content"].as_str().unwrap().len(),
        32768
    );
    assert_eq!(page["outcome"]["result"]["next_offset"], 32768);
    assert_eq!(page["outcome"]["result"]["total_bytes"], 40000);
    assert_eq!(
        f.ok(&["artifact", "read", reference], None)["content"],
        content
    );
    assert_eq!(child.finish(), (0, String::new()));
}

#[test]
fn child_artifact_requests_bind_writes_but_allow_cross_session_reads() {
    let f = Fixture::new();
    let (_stream, session) = enroll(&f, "pi-session");
    let mut child = f.start();
    child.request(1, json!({"kind":"artifact","request":{"action":"write","name":"assessment.md","description":"Findings","text":"useful output"}}));
    let written = child.response(1);
    assert_eq!(written["outcome"]["status"], "success", "{written}");
    let reference = written["outcome"]["result"]["artifact"]["reference"]
        .as_str()
        .unwrap();
    let mut other = Transport::start(f.command(), "other-session");
    assert_eq!(
        other.next(),
        json!({"type":"ready","protocol":2,"source":streams::child_protocol::SOURCE_DIGEST})
    );
    other.request(
        1,
        json!({"kind":"artifact","request":{"action":"read","reference":reference}}),
    );
    assert_eq!(
        other.response(1)["outcome"]["result"]["content"],
        "useful output"
    );
    other.request(2, json!({"kind":"artifact","request":{"action":"write","session_id":session,"name":"bad.md","description":"No","text":"wrong producer"}}));
    assert_eq!(other.response(2)["outcome"]["status"], "error");
    other.request(
        3,
        json!({"kind":"artifact","request":{"action":"delete","reference":reference}}),
    );
    assert_eq!(other.response(3)["outcome"]["status"], "error");
    child.request(
        2,
        json!({"kind":"artifact","request":{"action":"list_session"}}),
    );
    assert_eq!(
        child.response(2)["outcome"]["result"]["artifacts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    child.request(3, json!({"kind":"artifact","request":{"action":"write","name":"bad.md","description":"No","text":"one","file":"also"}}));
    assert_eq!(child.response(3)["outcome"]["status"], "error");
    child.request(
        4,
        json!({"kind":"artifact","request":{"action":"delete","reference":reference}}),
    );
    assert_eq!(child.response(4)["outcome"]["status"], "success");
    assert_eq!(child.finish(), (0, String::new()));
    assert_eq!(other.finish(), (0, String::new()));
}
