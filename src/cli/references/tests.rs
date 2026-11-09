use super::*;
use crate::cli::args::{Cli, Command};
use clap::Parser;
use serde_json::json;

fn parse(arguments: &[&str]) -> ReferenceCommand {
    let parsed = Cli::try_parse_from(
        ["streams", "reference"]
            .into_iter()
            .chain(arguments.iter().copied()),
    )
    .unwrap();
    let Command::Reference { command } = parsed.command else {
        panic!("reference command")
    };
    command
}

fn current(document: bool) -> Stream {
    let mut stream = Stream::new("work", "Work", None, "host").unwrap();
    let target = if document {
        json!({"kind":"document","repository_url":"repo","path":"plan.md","ref":"topic","commit":"commit","source_session_id":"source"})
    } else {
        json!({"kind":"pr","url":"https://example.test/pr/1"})
    };
    stream.references.push(
        serde_json::from_value(json!({
            "name":"Plan","purpose":"Guide work","role":"brief","state":"active",
            "target":target,"revision":"old"
        }))
        .unwrap(),
    );
    stream
}

fn desired(stream: &Stream, arguments: &[&str]) -> ReferenceValue {
    let ReferenceChange::UserSet {
        desired,
        next_revision,
        ..
    } = mutation(stream, &parse(arguments), "new").unwrap()
    else {
        panic!("set")
    };
    assert_eq!(next_revision, "new");
    desired
}

#[test]
fn add_requires_complete_values_and_defaults_to_active() {
    let stream = Stream::new("work", "Work", None, "host").unwrap();
    for kind in ["pr", "issue", "link"] {
        let args = [
            "add",
            "work",
            "--kind",
            kind,
            "--url",
            "https://example.test/item",
            "--name",
            "Name",
            "--purpose",
            "Purpose",
            "--role",
            "deliverable",
        ];
        let value = desired(&stream, &args);
        assert_eq!(value.state, ReferenceState::Active);
        assert_eq!(format!("{:?}", value.role), "Deliverable");
        for state in ["active", "fulfilled", "dropped"] {
            let mut explicit = args.to_vec();
            explicit.extend(["--state", state]);
            assert_eq!(desired(&stream, &explicit).state.to_string(), state);
        }
        for flag in ["--kind", "--name", "--purpose", "--role"] {
            let index = args.iter().position(|argument| *argument == flag).unwrap();
            let mut missing = args.to_vec();
            missing.drain(index..index + 2);
            assert!(
                mutation(&stream, &parse(&missing), "new").is_err(),
                "{flag}"
            );
        }
    }
    let value = desired(
        &stream,
        &[
            "add",
            "work",
            "--kind",
            "document",
            "--repository-url",
            "repo",
            "--path",
            "doc",
            "--name",
            "Name",
            "--purpose",
            "Purpose",
            "--role",
            "brief",
            "--ref",
            "topic",
            "--commit",
            "commit",
            "--source-session",
            "source",
        ],
    );
    let ReferenceTarget::Document(document) = value.target else {
        panic!("document")
    };
    assert_eq!(document.r#ref.as_deref(), Some("topic"));
    assert_eq!(document.commit.as_deref(), Some("commit"));
    assert_eq!(document.source_session_id.as_deref(), Some("source"));
}

#[test]
fn add_refuses_existing_identity_and_unsupported_flags() {
    let stream = current(false);
    let command = parse(&[
        "add",
        "work",
        "--kind",
        "issue",
        "--url",
        "https://example.test/pr/1",
        "--name",
        "Name",
        "--purpose",
        "Purpose",
        "--role",
        "brief",
    ]);
    assert!(
        mutation(&stream, &command, "new")
            .unwrap_err()
            .to_string()
            .contains("already exists")
    );
    let empty = Stream::new("work", "Work", None, "host").unwrap();
    for extra in [
        vec!["--target-url", "https://example.test/new"],
        vec!["--clear-ref"],
        vec!["--ref", "topic"],
    ] {
        let mut args = vec![
            "add",
            "work",
            "--kind",
            "link",
            "--url",
            "https://example.test/new",
            "--name",
            "Name",
            "--purpose",
            "Purpose",
            "--role",
            "brief",
        ];
        args.extend(extra);
        assert!(mutation(&empty, &parse(&args), "new").is_err());
    }
}

#[test]
fn edit_preserves_metadata_and_document_correction_copies_recovery() {
    let stream = current(true);
    let value = desired(
        &stream,
        &[
            "edit",
            "work",
            "--repository-url",
            "repo",
            "--path",
            "plan.md",
            "--name",
            "Updated",
        ],
    );
    let mut expected = stream.references[0].value();
    expected.name = "Updated".into();
    assert_eq!(value, expected);
    let value = desired(
        &stream,
        &[
            "edit",
            "work",
            "--repository-url",
            "repo",
            "--path",
            "plan.md",
            "--target-repository-url",
            "other",
            "--target-path",
            "accepted.md",
            "--purpose",
            "Accepted",
            "--role",
            "deliverable",
            "--state",
            "fulfilled",
        ],
    );
    let ReferenceTarget::Document(document) = value.target else {
        panic!("document")
    };
    assert_eq!(document.repository_url, "other");
    assert_eq!(document.path, "accepted.md");
    assert_eq!(document.r#ref.as_deref(), Some("topic"));
    assert_eq!(document.commit.as_deref(), Some("commit"));
    assert_eq!(document.source_session_id.as_deref(), Some("source"));
    assert_eq!(value.purpose, "Accepted");
    assert_eq!(value.state, ReferenceState::Fulfilled);
    let no_op = desired(
        &stream,
        &[
            "edit",
            "work",
            "--repository-url",
            "repo",
            "--path",
            "plan.md",
            "--name",
            "Plan",
        ],
    );
    assert_eq!(no_op, stream.references[0].value());
    assert!(
        mutation(
            &stream,
            &parse(&[
                "edit",
                "work",
                "--repository-url",
                "repo",
                "--path",
                "plan.md"
            ]),
            "new"
        )
        .is_err()
    );
}

#[test]
fn edit_family_changes_require_explicit_kind_and_complete_targets() {
    let stream = current(true);
    for flags in [
        vec!["--kind", "link"],
        vec!["--target-url", "https://example.test/item"],
    ] {
        let mut args = vec![
            "edit",
            "work",
            "--repository-url",
            "repo",
            "--path",
            "plan.md",
        ];
        args.extend(flags);
        assert!(mutation(&stream, &parse(&args), "new").is_err());
    }
    let value = desired(
        &stream,
        &[
            "edit",
            "work",
            "--repository-url",
            "repo",
            "--path",
            "plan.md",
            "--kind",
            "link",
            "--target-url",
            "https://example.test/item",
        ],
    );
    assert_eq!(
        value.target,
        ReferenceTarget::Link {
            url: "https://example.test/item".into()
        }
    );
    let stream = current(false);
    let value = desired(
        &stream,
        &[
            "edit",
            "work",
            "--url",
            "https://example.test/pr/1",
            "--kind",
            "issue",
        ],
    );
    assert_eq!(
        value.target,
        ReferenceTarget::Issue {
            url: "https://example.test/pr/1".into()
        }
    );
    let value = desired(
        &stream,
        &[
            "edit",
            "work",
            "--url",
            "https://example.test/pr/1",
            "--kind",
            "document",
            "--target-repository-url",
            "repo",
            "--target-path",
            "doc",
        ],
    );
    let ReferenceTarget::Document(document) = value.target else {
        panic!("document")
    };
    assert_eq!(document.r#ref, None);
    assert_eq!(document.commit, None);
    assert_eq!(document.source_session_id, None);
}

#[test]
fn recovery_flags_set_clear_and_reject_web_targets() {
    let stream = current(true);
    let value = desired(
        &stream,
        &[
            "edit",
            "work",
            "--repository-url",
            "repo",
            "--path",
            "plan.md",
            "--clear-ref",
            "--clear-commit",
            "--clear-source-session",
        ],
    );
    let ReferenceTarget::Document(document) = value.target else {
        panic!("document")
    };
    assert_eq!(document.r#ref, None);
    assert_eq!(document.commit, None);
    assert_eq!(document.source_session_id, None);
    for flag in ["--clear-ref", "--clear-commit", "--clear-source-session"] {
        assert!(
            mutation(
                &current(false),
                &parse(&["edit", "work", "--url", "https://example.test/pr/1", flag]),
                "new"
            )
            .is_err()
        );
    }
    for flag in ["--ref", "--commit", "--source-session"] {
        assert!(
            mutation(
                &stream,
                &parse(&[
                    "edit",
                    "work",
                    "--repository-url",
                    "repo",
                    "--path",
                    "plan.md",
                    flag,
                    ""
                ]),
                "new"
            )
            .is_err()
        );
        assert!(
            mutation(
                &current(false),
                &parse(&[
                    "edit",
                    "work",
                    "--url",
                    "https://example.test/pr/1",
                    flag,
                    "known"
                ]),
                "new"
            )
            .is_err()
        );
    }
}

#[test]
fn dropped_edits_require_explicit_restore_and_delete_confirmation() {
    let mut stream = current(false);
    stream.references[0].state = ReferenceState::Dropped;
    assert!(
        mutation(
            &stream,
            &parse(&[
                "edit",
                "work",
                "--url",
                "https://example.test/pr/1",
                "--state",
                "active"
            ]),
            "new"
        )
        .is_err()
    );
    assert_eq!(
        desired(
            &stream,
            &[
                "edit",
                "work",
                "--url",
                "https://example.test/pr/1",
                "--name",
                "Updated"
            ]
        )
        .state,
        ReferenceState::Dropped
    );
    for state in ["active", "fulfilled"] {
        assert_eq!(
            desired(
                &stream,
                &[
                    "restore",
                    "work",
                    "--url",
                    "https://example.test/pr/1",
                    "--state",
                    state
                ]
            )
            .state
            .to_string(),
            state
        );
    }
    assert!(
        mutation(
            &stream,
            &parse(&[
                "restore",
                "work",
                "--url",
                "https://example.test/pr/1",
                "--state",
                "dropped"
            ]),
            "new"
        )
        .is_err()
    );
    assert!(
        mutation(
            &stream,
            &parse(&["delete", "work", "--url", "https://example.test/pr/1"]),
            "new"
        )
        .is_err()
    );
    assert!(matches!(
        mutation(
            &stream,
            &parse(&[
                "delete",
                "work",
                "--url",
                "https://example.test/pr/1",
                "--confirm"
            ]),
            "new"
        )
        .unwrap(),
        ReferenceChange::UserDelete { .. }
    ));
    assert_eq!(
        desired(
            &current(false),
            &["drop", "work", "--url", "https://example.test/pr/1"]
        )
        .state,
        ReferenceState::Dropped
    );
}

#[test]
fn human_output_escapes_unsafe_scalars_and_json_roundtrips() {
    for value in [
        "safe",
        "name\nline",
        "\u{1b}[31m",
        "\u{202e}",
        "\u{2028}",
        "\u{2029}",
        "\u{e0001}",
    ] {
        let encoded = safe_reference_json(&value);
        assert!(!encoded.chars().any(unsafe_character));
        assert_eq!(serde_json::from_str::<String>(&encoded).unwrap(), value);
        if value.chars().any(unsafe_character) {
            assert_eq!(reference_text(value), encoded)
        } else {
            assert_eq!(reference_text(value), value)
        }
    }
    assert_eq!(safe_reference_json(&"\u{e0001}"), "\"\\udb40\\udc01\"");
    let mut stream = current(true);
    stream.references[0].name = "name\u{1b}\n\u{202e}".into();
    if let ReferenceTarget::Document(document) = &mut stream.references[0].target {
        document.path = "line\nfile".into();
    }
    let output = list(&stream);
    let plain = output.plain.unwrap();
    assert!(plain.contains("selector-json: {\"kind\":\"document\",\"repository_url\":\"repo\",\"path\":\"line\\nfile\"}"));
    assert!(!plain.chars().any(|c| c != '\n' && unsafe_character(c)));
    assert_eq!(
        output.value["references"][0]["name"],
        "name\u{1b}\n\u{202e}"
    );
    let empty = Stream::new("work", "Work", None, "host").unwrap();
    assert_eq!(list(&empty).plain.as_deref(), Some("No references"));
}

#[test]
fn shell_selectors_roundtrip_apostrophes_option_markers_and_equals() {
    for (repository_url, path) in [
        ("git@example.test:a'b", "docs/a=b'c"),
        ("--", "--"),
        ("-repo=x", "-file=y"),
    ] {
        let key = ResourceKey::Document {
            repository_url: repository_url.into(),
            path: path.into(),
        };
        let script = format!("set -- {}; printf '%s\\0' \"$@\"", selector_text(&key));
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert!(output.status.success());
        let words: Vec<_> = output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|word| !word.is_empty())
            .map(|word| std::str::from_utf8(word).unwrap())
            .collect();
        assert_eq!(
            words,
            [
                format!("--repository-url={repository_url}"),
                format!("--path={path}")
            ]
        );
        let parsed = Cli::try_parse_from(
            ["streams", "reference", "drop", "work"]
                .into_iter()
                .chain(words.iter().copied()),
        )
        .unwrap();
        let Command::Reference {
            command: ReferenceCommand::Drop { selector, .. },
        } = parsed.command
        else {
            panic!("drop")
        };
        assert_eq!(super::key(&selector).unwrap(), key);
    }
}
