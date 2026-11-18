use std::path::Path;

use serde::Serialize;
use serde_json::{Map, Value};
use streams::model::{Repository, Session, Stream};

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
    fn ordinary_stream_fields_preserve_references() {
        let mut current = current_stream();
        current.references.push(serde_json::from_value(json!({
            "name":"Plan", "purpose":"Guide work", "role":"brief", "state":"active",
            "target":{"kind":"document","repository_url":"git@example.test:tools","path":"plan.md"}
        })).unwrap());
        let references = current.references.clone();
        let input = StreamFields {
            goal: Some("Updated goal".into()),
            add_repository: strings(&["tools", "git@example.test:tools"]),
            ..Default::default()
        };
        let changes = stream(&current, &input, &mut no_read).unwrap();
        assert_eq!(
            Value::Object(changes),
            json!({
                "goal":"Updated goal", "repositories":{"tools":{"url":"git@example.test:tools"}}
            })
        );
        assert_eq!(current.references, references);
    }
}
