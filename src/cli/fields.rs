use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};
use streams::model::{Document, Link, Repository, Session, Stream};

use super::CliError;
use super::args::{SessionFields, StreamFields};

pub(super) fn stream(
    current: &Stream,
    input: &StreamFields,
    read: &mut impl FnMut(&Path) -> Result<String, CliError>,
) -> Result<Map<String, Value>, CliError> {
    let mut changes = Map::new();
    text(
        &mut changes,
        "goal",
        &input.goal,
        input.goal_file.as_deref(),
        input.clear_goal,
        read,
    )?;
    optional(&mut changes, "goal_mode", input.goal_mode)?;
    optional(&mut changes, "title_mode", input.title_mode)?;
    optional(&mut changes, "status", input.status)?;

    if !input.add_repository.is_empty() || !input.remove_repository.is_empty() {
        let mut repositories = current.repositories.clone();
        for alias in &input.remove_repository {
            repositories.remove(alias);
        }
        for arguments in input.add_repository.chunks_exact(2) {
            let alias = &arguments[0];
            let repository = Repository::new(&arguments[1])?;
            if repositories
                .get(alias)
                .is_some_and(|old| old != &repository)
            {
                return Err(CliError::message(format!(
                    "repository {alias:?} exists; remove and add it explicitly to replace it"
                )));
            }
            repositories.insert(alias.clone(), repository);
        }
        changes.insert("repositories".into(), serde_json::to_value(repositories)?);
    }

    let mut documents = current.documents.clone();
    if !input.add_document.is_empty() || !input.remove_document.is_empty() {
        documents.retain(|item| {
            !input
                .remove_document
                .chunks_exact(2)
                .any(|arguments| item.identity() == (arguments[0].as_str(), arguments[1].as_str()))
        });
        for arguments in input.add_document.chunks_exact(4) {
            let item = Document::new(
                &arguments[0],
                &arguments[1],
                &arguments[2],
                &arguments[3],
                None,
            )?;
            match documents
                .iter()
                .find(|old| old.identity() == item.identity())
            {
                Some(old) if old != &item => {
                    return Err(CliError::message(
                        "document exists with different contents; remove and add it explicitly",
                    ));
                }
                Some(_) => {}
                None => documents.push(item),
            }
        }
    }

    links(
        &mut changes,
        "issues",
        "issue",
        &current.issues,
        &input.add_issue,
        &input.remove_issue,
    )?;
    links(
        &mut changes,
        "prs",
        "pr",
        &current.prs,
        &input.add_pr,
        &input.remove_pr,
    )?;

    type ProvenanceField = fn(&mut Document) -> &mut Option<String>;
    let provenance: [(&[String], ProvenanceField); 2] = [
        (&input.document_commit, |document| &mut document.commit),
        (&input.document_session, |document| {
            &mut document.source_session_id
        }),
    ];
    for (updates, field) in provenance {
        for arguments in updates.chunks_exact(3) {
            let repository = &arguments[0];
            let path = &arguments[1];
            let item = documents
                .iter_mut()
                .find(|item| item.identity() == (repository.as_str(), path.as_str()))
                .ok_or_else(|| {
                    CliError::message(format!(
                        "no document matches repository/path: {repository}:{path}"
                    ))
                })?;
            *field(item) = Some(arguments[2].clone());
            item.validate()?;
        }
    }
    if [
        &input.add_document,
        &input.remove_document,
        &input.document_commit,
        &input.document_session,
    ]
    .iter()
    .any(|values| !values.is_empty())
    {
        changes.insert("documents".into(), serde_json::to_value(&documents)?);
    }
    Ok(changes)
}

pub(super) fn session(
    _current: &Session,
    input: &SessionFields,
    read: &mut impl FnMut(&Path) -> Result<String, CliError>,
) -> Result<Map<String, Value>, CliError> {
    let fields = [
        (
            "recap",
            &input.recap,
            input.recap_file.as_deref(),
            input.clear_recap,
        ),
        (
            "recap_short",
            &input.recap_short,
            input.recap_short_file.as_deref(),
            input.clear_recap_short,
        ),
        (
            "state",
            &input.state,
            input.state_file.as_deref(),
            input.clear_state,
        ),
        (
            "state_short",
            &input.state_short,
            input.state_short_file.as_deref(),
            input.clear_state_short,
        ),
        (
            "remaining",
            &input.remaining,
            input.remaining_file.as_deref(),
            input.clear_remaining,
        ),
        (
            "remaining_short",
            &input.remaining_short,
            input.remaining_short_file.as_deref(),
            input.clear_remaining_short,
        ),
    ];
    if fields
        .iter()
        .filter_map(|(_, _, file, _)| *file)
        .filter(|path| *path == Path::new("-"))
        .count()
        > 1
    {
        return Err(CliError::message(
            "only one field can read stdin in a command",
        ));
    }
    let mut changes = Map::new();
    for (name, value, file, clear) in fields {
        text(&mut changes, name, value, file, false, read)?;
        if clear {
            changes.insert(
                name.into(),
                if name.ends_with("_short") {
                    Value::Null
                } else {
                    Value::String(String::new())
                },
            );
        }
    }
    optional(&mut changes, "title_mode", input.title_mode)?;
    optional(&mut changes, "status", input.status)?;
    Ok(changes)
}

fn text(
    changes: &mut Map<String, Value>,
    name: &str,
    value: &Option<String>,
    file: Option<&Path>,
    clear: bool,
    read: &mut impl FnMut(&Path) -> Result<String, CliError>,
) -> Result<(), CliError> {
    if let Some(file) = file {
        changes.insert(name.into(), Value::String(read(file)?));
    } else if let Some(value) = value {
        changes.insert(name.into(), Value::String(value.clone()));
    }
    if clear {
        changes.insert(name.into(), Value::Null);
    }
    Ok(())
}

fn optional(
    changes: &mut Map<String, Value>,
    name: &str,
    value: Option<impl Serialize>,
) -> Result<(), CliError> {
    if let Some(value) = value {
        changes.insert(name.into(), serde_json::to_value(value)?);
    }
    Ok(())
}

fn links(
    changes: &mut Map<String, Value>,
    field: &str,
    singular: &str,
    current: &[Link],
    additions: &[String],
    removals: &[String],
) -> Result<(), CliError> {
    if additions.is_empty() && removals.is_empty() {
        return Ok(());
    }
    let mut values: Vec<Link> = current
        .iter()
        .filter(|item| !removals.contains(&item.url))
        .cloned()
        .collect();
    for arguments in additions.chunks_exact(2) {
        let item = Link::new(&arguments[0], &arguments[1])?;
        match values.iter().find(|old| old.url == item.url) {
            Some(old) if old != &item => {
                return Err(CliError::message(format!(
                    "{singular} exists with different contents; remove and add it explicitly"
                )));
            }
            Some(_) => {}
            None => values.push(item),
        }
    }
    changes.insert(field.into(), serde_json::to_value(values)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use streams::model::{NamingMode, Status, TitleMode};

    fn current_stream() -> Stream {
        Stream::new("stream", "Title", Some("Goal".into()), "host").unwrap()
    }

    fn current_session() -> Session {
        Session::new("session", "host", "stream", "pi", "native", "Title").unwrap()
    }

    fn no_read(_: &Path) -> Result<String, CliError> {
        panic!("unexpected text read")
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn omitted_fields_produce_no_changes() {
        assert!(
            stream(&current_stream(), &StreamFields::default(), &mut no_read)
                .unwrap()
                .is_empty()
        );
        assert!(
            session(&current_session(), &SessionFields::default(), &mut no_read)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn scalars_preserve_text_and_explicit_authority() {
        let input = StreamFields {
            goal: Some("\n  user text  \n".into()),
            goal_mode: Some(NamingMode::Automatic),
            title_mode: Some(TitleMode::Manual),
            status: Some(Status::Waiting),
            ..Default::default()
        };
        let changes = stream(&current_stream(), &input, &mut no_read).unwrap();
        assert_eq!(
            Value::Object(changes),
            json!({"goal": "\n  user text  \n", "goal_mode": "automatic", "title_mode": "manual", "status": "waiting"})
        );
        assert_eq!(
            stream(
                &current_stream(),
                &StreamFields {
                    clear_goal: true,
                    ..Default::default()
                },
                &mut no_read
            )
            .unwrap()["goal"],
            Value::Null
        );
    }

    #[test]
    fn reads_are_delegated_without_path_or_text_normalization() {
        let input = StreamFields {
            goal_file: Some("~/goal".into()),
            ..Default::default()
        };
        let mut read = |path: &Path| {
            assert_eq!(path, Path::new("~/goal"));
            Ok("\nfile text\n".into())
        };
        assert_eq!(
            stream(&current_stream(), &input, &mut read).unwrap()["goal"],
            "\nfile text\n"
        );
    }

    #[test]
    fn session_stdin_conflict_is_rejected_before_any_read() {
        let input = SessionFields {
            state_file: Some("-".into()),
            remaining_file: Some("-".into()),
            ..Default::default()
        };
        let error = session(&current_session(), &input, &mut no_read).unwrap_err();
        assert!(error.to_string().contains("only one field can read stdin"));
    }

    #[test]
    fn recap_and_short_forms_preserve_input_and_clear_to_their_wire_types() {
        let input = SessionFields {
            recap_file: Some("recap".into()),
            recap_short: Some("Scope".into()),
            state_short: Some("Now".into()),
            remaining_short: Some("Next".into()),
            ..Default::default()
        };
        let changes = session(&current_session(), &input, &mut |path| {
            assert_eq!(path, Path::new("recap"));
            Ok("Scope and achievements\n".into())
        })
        .unwrap();
        assert_eq!(
            Value::Object(changes),
            json!({"recap":"Scope and achievements\n","recap_short":"Scope","state_short":"Now","remaining_short":"Next"})
        );
        let input = SessionFields {
            clear_recap: true,
            clear_recap_short: true,
            clear_state: true,
            clear_state_short: true,
            clear_remaining: true,
            clear_remaining_short: true,
            ..Default::default()
        };
        assert_eq!(
            Value::Object(session(&current_session(), &input, &mut no_read).unwrap()),
            json!({"recap":"","recap_short":null,"state":"","state_short":null,"remaining":"","remaining_short":null})
        );
        let input = SessionFields {
            recap_file: Some("-".into()),
            remaining_short_file: Some("-".into()),
            ..Default::default()
        };
        assert!(session(&current_session(), &input, &mut no_read).is_err());
    }

    #[test]
    fn session_scalars_preserve_text_and_explicit_authority() {
        let input = SessionFields {
            state: Some("\n  state  \n".into()),
            remaining_file: Some("-".into()),
            title_mode: Some(TitleMode::Manual),
            status: Some(Status::Blocked),
            ..Default::default()
        };
        let changes = session(&current_session(), &input, &mut |path| {
            assert_eq!(path, Path::new("-"));
            Ok("remaining\n".into())
        })
        .unwrap();
        assert_eq!(
            Value::Object(changes),
            json!({"state": "\n  state  \n", "remaining": "remaining\n", "title_mode": "manual", "status": "blocked"})
        );
    }

    #[test]
    fn repository_replacement_requires_remove_and_preserves_others() {
        let mut current = current_stream();
        current
            .repositories
            .insert("repo".into(), Repository::new("old").unwrap());
        current
            .repositories
            .insert("keep".into(), Repository::new("unchanged").unwrap());
        let mut input = StreamFields {
            add_repository: strings(&["repo", "new"]),
            ..Default::default()
        };
        assert!(stream(&current, &input, &mut no_read).is_err());
        input.remove_repository = strings(&["repo", "absent"]);
        let changes = stream(&current, &input, &mut no_read).unwrap();
        assert_eq!(
            changes["repositories"],
            json!({"repo": {"url": "new"}, "keep": {"url": "unchanged"}})
        );
        input.add_repository = strings(&["repo", "new", "repo", "new"]);
        assert!(stream(&current, &input, &mut no_read).is_ok());
        input.add_repository = strings(&["repo", "new", "repo", "different"]);
        assert!(stream(&current, &input, &mut no_read).is_err());
        assert_eq!(current.repositories["repo"].url, "old");
    }

    #[test]
    fn link_removals_precede_additions_and_duplicates_preserve_order() {
        let mut current = current_stream();
        current.issues = vec![
            Link::new("one", "old").unwrap(),
            Link::new("two", "keep").unwrap(),
        ];
        let mut input = StreamFields {
            add_issue: strings(&["one", "new"]),
            ..Default::default()
        };
        assert!(stream(&current, &input, &mut no_read).is_err());
        input.remove_issue = strings(&["one", "absent"]);
        input.add_issue = strings(&["one", "new", "one", "new", "three", "added"]);
        input.add_pr = strings(&["pr", "description", "pr", "description"]);
        let changes = stream(&current, &input, &mut no_read).unwrap();
        assert_eq!(
            changes["issues"],
            json!([{"url": "two", "description": "keep"}, {"url": "one", "description": "new"}, {"url": "three", "description": "added"}])
        );
        assert_eq!(
            changes["prs"],
            json!([{"url": "pr", "description": "description"}])
        );
    }

    #[test]
    fn document_identity_and_provenance_follow_collection_order() {
        let mut current = current_stream();
        current.documents =
            vec![Document::new("repo", "main", "doc.md", "old", Some("commit".into())).unwrap()];
        let mut input = StreamFields {
            add_document: strings(&["repo", "main", "doc.md", "old"]),
            ..Default::default()
        };
        // Even otherwise identical additions may not silently discard provenance.
        assert!(stream(&current, &input, &mut no_read).is_err());
        input.remove_document = strings(&["repo", "doc.md"]);
        input.add_document = strings(&[
            "repo", "branch", "doc.md", "new", "repo", "branch", "doc.md", "new",
        ]);
        input.document_commit = strings(&[
            "repo",
            "doc.md",
            "first-sha",
            "repo",
            "doc.md",
            "latest-sha",
        ]);
        input.document_session = strings(&[
            "repo",
            "doc.md",
            "first-session",
            "repo",
            "doc.md",
            "source-session",
        ]);
        let changes = stream(&current, &input, &mut no_read).unwrap();
        assert_eq!(
            changes["documents"],
            json!([{"repository": "repo", "ref": "branch", "path": "doc.md", "description": "new", "commit": "latest-sha", "source_session_id": "source-session"}])
        );
        assert_eq!(current.documents[0].commit.as_deref(), Some("commit"));
    }

    #[test]
    fn provenance_requires_known_document_and_valid_commit() {
        let mut input = StreamFields {
            document_commit: strings(&["repo", "missing", "commit"]),
            ..Default::default()
        };
        let error = stream(&current_stream(), &input, &mut no_read).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no document matches repository/path: repo:missing")
        );
        input.add_document = strings(&["repo", "main", "missing", ""]);
        input.document_commit[2] = "".into();
        assert!(stream(&current_stream(), &input, &mut no_read).is_err());
    }

    #[test]
    fn new_provenance_requires_exact_existing_identity_and_valid_text() {
        let mut current = current_stream();
        current
            .documents
            .push(Document::new("repo", "main", "doc", "", None).unwrap());
        for field in ["commit", "source_session_id"] {
            for (repository, path, value, valid) in [
                ("repo", "doc", "known", true),
                ("other", "doc", "known", false),
                ("repo", "other", "known", false),
                ("repo", "doc", "", false),
                ("repo", "doc", " \n", false),
                ("repo", "doc", "bad\u{0}text", false),
            ] {
                let mut input = StreamFields::default();
                let updates = strings(&[repository, path, value]);
                if field == "commit" {
                    input.document_commit = updates;
                } else {
                    input.document_session = updates;
                }
                let result = stream(&current, &input, &mut no_read);
                assert_eq!(
                    result.is_ok(),
                    valid,
                    "{field}: {repository}:{path} {value:?}"
                );
                if valid {
                    let changes = result.unwrap();
                    assert_eq!(changes["documents"][0][field], "known");
                    assert_eq!(changes["documents"][0]["ref"], "main");
                }
                assert_eq!(current.documents[0].commit, None);
                assert_eq!(current.documents[0].source_session_id, None);
            }
        }
    }

    #[test]
    fn unchanged_provenance_survives_other_document_changes() {
        let mut current = current_stream();
        let mut original = Document::new(
            "repo",
            "main",
            "original",
            "keep",
            Some("latest-sha".into()),
        )
        .unwrap();
        original.source_session_id = Some("source-session".into());
        current.documents.push(original.clone());
        let input = StreamFields {
            add_document: strings(&["repo", "other", "new", "added"]),
            ..Default::default()
        };
        let changes = stream(&current, &input, &mut no_read).unwrap();
        assert_eq!(
            changes["documents"][0],
            serde_json::to_value(original).unwrap()
        );
    }
}
