#[cfg(test)]
mod tests;

use serde::Serialize;
use serde_json::json;
use streams::{
    application,
    model::{
        Reference, ReferenceState, ReferenceTarget, ReferenceValue, RepositoryDocument,
        ResourceKey, Stream,
    },
    operations::references::ReferenceChange,
};
use unicode_general_category::{GeneralCategory, get_general_category};

use super::{
    App, CliError, Output, Result,
    args::{ReferenceCommand, ReferenceFields, ReferenceKind, ReferenceSelector, Target},
    catalog,
};

impl App {
    pub(super) fn reference_cli(&self, command: ReferenceCommand) -> Result<Output> {
        let selected = match &command {
            ReferenceCommand::List { stream }
            | ReferenceCommand::Add { stream, .. }
            | ReferenceCommand::Edit { stream, .. }
            | ReferenceCommand::Drop { stream, .. }
            | ReferenceCommand::Restore { stream, .. }
            | ReferenceCommand::Delete { stream, .. } => stream,
        };
        let opened = application::open_records(&self.config)?;
        let stream = self.record_target(
            &opened,
            &Target {
                reference: Some(selected.clone()),
                current: false,
                frontend: None,
            },
        )?;
        if matches!(command, ReferenceCommand::List { .. }) {
            return Ok(list(&stream));
        }
        let settings = self.config.load()?;
        if settings.catalog_root != opened.root() {
            return Err(CliError::message(
                "catalog root changed during reference edit",
            ));
        }
        let operation_id = uuid::Uuid::new_v4().to_string();
        let change = mutation(&stream, &command, &operation_id)?;
        catalog::finish_save(
            &operation_id,
            application::save_user_references(
                opened.records(),
                &operation_id,
                &stream,
                vec![change],
                settings.naming,
            ),
        )
    }
}

fn key(selector: &ReferenceSelector) -> Result<ResourceKey> {
    let key = match (&selector.url, &selector.repository_url, &selector.path) {
        (Some(url), None, None) => ResourceKey::Url { url: url.clone() },
        (None, Some(repository_url), Some(path)) => ResourceKey::Document {
            repository_url: repository_url.clone(),
            path: path.clone(),
        },
        _ => {
            return Err(CliError::message(
                "supply --url or both --repository-url and --path",
            ));
        }
    };
    key.validate()?;
    Ok(key)
}

fn kind(target: &ReferenceTarget) -> ReferenceKind {
    match target {
        ReferenceTarget::Document(_) => ReferenceKind::Document,
        ReferenceTarget::Pr { .. } => ReferenceKind::Pr,
        ReferenceTarget::Issue { .. } => ReferenceKind::Issue,
        ReferenceTarget::Link { .. } => ReferenceKind::Link,
    }
}

fn target(
    kind: ReferenceKind,
    key: &ResourceKey,
    previous: Option<&ReferenceTarget>,
) -> Result<ReferenceTarget> {
    match (kind, key) {
        (
            ReferenceKind::Document,
            ResourceKey::Document {
                repository_url,
                path,
            },
        ) => {
            let mut document = match previous {
                Some(ReferenceTarget::Document(document)) => document.clone(),
                _ => RepositoryDocument {
                    repository_url: repository_url.clone(),
                    path: path.clone(),
                    r#ref: None,
                    commit: None,
                    source_session_id: None,
                },
            };
            document.repository_url = repository_url.clone();
            document.path = path.clone();
            Ok(ReferenceTarget::Document(document))
        }
        (ReferenceKind::Pr, ResourceKey::Url { url }) => {
            Ok(ReferenceTarget::Pr { url: url.clone() })
        }
        (ReferenceKind::Issue, ResourceKey::Url { url }) => {
            Ok(ReferenceTarget::Issue { url: url.clone() })
        }
        (ReferenceKind::Link, ResourceKey::Url { url }) => {
            Ok(ReferenceTarget::Link { url: url.clone() })
        }
        _ => Err(CliError::message(
            "kind and target family must match; changing families requires --kind and a complete target",
        )),
    }
}

fn has_correction(fields: &ReferenceFields) -> bool {
    fields.target_url.is_some()
        || fields.target_repository_url.is_some()
        || fields.target_path.is_some()
}

fn has_recovery(fields: &ReferenceFields) -> bool {
    fields.r#ref.is_some()
        || fields.commit.is_some()
        || fields.source_session.is_some()
        || fields.clear_ref
        || fields.clear_commit
        || fields.clear_source_session
}

fn recovery(target: &mut ReferenceTarget, fields: &ReferenceFields) -> Result<()> {
    let ReferenceTarget::Document(document) = target else {
        return if has_recovery(fields) {
            Err(CliError::message(
                "recovery flags require a document target",
            ))
        } else {
            Ok(())
        };
    };
    for (saved, supplied, clear) in [
        (&mut document.r#ref, &fields.r#ref, fields.clear_ref),
        (&mut document.commit, &fields.commit, fields.clear_commit),
        (
            &mut document.source_session_id,
            &fields.source_session,
            fields.clear_source_session,
        ),
    ] {
        if supplied.is_some() && clear {
            return Err(CliError::message(
                "cannot set and clear the same recovery field",
            ));
        }
        if clear {
            *saved = None;
        } else if let Some(value) = supplied {
            *saved = Some(value.clone());
        }
    }
    Ok(())
}

fn add_value(selector: &ReferenceSelector, fields: &ReferenceFields) -> Result<ReferenceValue> {
    if has_correction(fields)
        || fields.clear_ref
        || fields.clear_commit
        || fields.clear_source_session
    {
        return Err(CliError::message(
            "add accepts no correction or recovery-clear flags",
        ));
    }
    let kind = fields
        .kind
        .ok_or_else(|| CliError::message("add requires --kind"))?;
    let mut value = ReferenceValue {
        name: fields
            .name
            .clone()
            .ok_or_else(|| CliError::message("add requires --name"))?,
        purpose: fields
            .purpose
            .clone()
            .ok_or_else(|| CliError::message("add requires --purpose"))?,
        role: fields
            .role
            .ok_or_else(|| CliError::message("add requires --role"))?,
        state: fields.state.unwrap_or(ReferenceState::Active),
        target: target(kind, &key(selector)?, None)?,
    };
    recovery(&mut value.target, fields)?;
    value.validate()?;
    Ok(value)
}

fn edit_value(reference: &Reference, fields: &ReferenceFields) -> Result<ReferenceValue> {
    if fields.name.is_none()
        && fields.purpose.is_none()
        && fields.kind.is_none()
        && fields.role.is_none()
        && fields.state.is_none()
        && !has_correction(fields)
        && !has_recovery(fields)
    {
        return Err(CliError::message(
            "edit requires at least one mutation flag",
        ));
    }
    if reference.state == ReferenceState::Dropped
        && fields
            .state
            .is_some_and(|state| state != ReferenceState::Dropped)
    {
        return Err(CliError::message(
            "use restore to make a Dropped reference visible",
        ));
    }
    let corrected = match (
        &fields.target_url,
        &fields.target_repository_url,
        &fields.target_path,
    ) {
        (None, None, None) => reference.target.key(),
        (Some(url), None, None) => ResourceKey::Url { url: url.clone() },
        (None, Some(repository_url), Some(path)) => ResourceKey::Document {
            repository_url: repository_url.clone(),
            path: path.clone(),
        },
        _ => {
            return Err(CliError::message(
                "correction requires --target-url or both --target-repository-url and --target-path",
            ));
        }
    };
    let mut value = reference.value();
    value.target = target(
        fields.kind.unwrap_or_else(|| kind(&reference.target)),
        &corrected,
        Some(&reference.target),
    )?;
    if let Some(name) = &fields.name {
        value.name = name.clone();
    }
    if let Some(purpose) = &fields.purpose {
        value.purpose = purpose.clone();
    }
    if let Some(role) = fields.role {
        value.role = role;
    }
    if let Some(state) = fields.state {
        value.state = state;
    }
    recovery(&mut value.target, fields)?;
    value.validate()?;
    Ok(value)
}

fn mutation(
    stream: &Stream,
    command: &ReferenceCommand,
    operation_id: &str,
) -> Result<ReferenceChange> {
    let (key, desired) = match command {
        ReferenceCommand::Add { target, fields, .. } => {
            let desired = add_value(target, fields)?;
            let key = desired.target.key();
            if stream
                .references
                .iter()
                .any(|reference| reference.target.key() == key)
            {
                return Err(CliError::message(format!(
                    "reference already exists: {}",
                    safe_reference_json(&key)
                )));
            }
            (key, desired)
        }
        ReferenceCommand::Edit { selector, .. }
        | ReferenceCommand::Drop { selector, .. }
        | ReferenceCommand::Restore { selector, .. }
        | ReferenceCommand::Delete { selector, .. } => {
            let key = key(selector)?;
            let reference = stream
                .references
                .iter()
                .find(|reference| reference.target.key() == key)
                .ok_or_else(|| {
                    CliError::message(format!(
                        "reference not found: {}",
                        safe_reference_json(&key)
                    ))
                })?;
            let mut desired = reference.value();
            match command {
                ReferenceCommand::Edit { fields, .. } => desired = edit_value(reference, fields)?,
                ReferenceCommand::Drop { .. } => desired.state = ReferenceState::Dropped,
                ReferenceCommand::Restore { state, .. } => {
                    if *state == ReferenceState::Dropped {
                        return Err(CliError::message(
                            "restore requires --state active or fulfilled",
                        ));
                    }
                    desired.state = *state;
                }
                ReferenceCommand::Delete { confirm, .. } => {
                    if !confirm {
                        return Err(CliError::message("delete requires --confirm"));
                    }
                    return Ok(ReferenceChange::UserDelete { key });
                }
                _ => unreachable!("selected mutation"),
            }
            (key, desired)
        }
        ReferenceCommand::List { .. } => return Err(CliError::message("list has no mutation")),
    };
    Ok(ReferenceChange::UserSet {
        key,
        desired,
        next_revision: operation_id.into(),
    })
}

fn unsafe_character(c: char) -> bool {
    matches!(
        get_general_category(c),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    )
}

pub(super) fn safe_reference_json<T: Serialize>(value: &T) -> String {
    let json = serde_json::to_string(value).expect("reference values are JSON-compatible");
    let mut output = String::new();
    for c in json.chars() {
        if unsafe_character(c) {
            for unit in c.encode_utf16(&mut [0; 2]) {
                output.push_str(&format!("\\u{unit:04x}"));
            }
        } else {
            output.push(c);
        }
    }
    output
}

pub(super) fn reference_text(value: &str) -> String {
    if value.chars().any(unsafe_character) {
        safe_reference_json(&value)
    } else {
        value.into()
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn selector_text(key: &ResourceKey) -> String {
    match key {
        ResourceKey::Url { url } => format!("--url={}", shell_quote(url)),
        ResourceKey::Document {
            repository_url,
            path,
        } if !repository_url.chars().any(unsafe_character)
            && !path.chars().any(unsafe_character) =>
        {
            format!(
                "--repository-url={} --path={}",
                shell_quote(repository_url),
                shell_quote(path)
            )
        }
        _ => format!("selector-json: {}", safe_reference_json(key)),
    }
}

fn list(stream: &Stream) -> Output {
    let plain = if stream.references.is_empty() {
        "No references".into()
    } else {
        stream
            .references
            .iter()
            .map(|reference| {
                let kind = match reference.target {
                    ReferenceTarget::Document(_) => "Document",
                    ReferenceTarget::Pr { .. } => "PR",
                    ReferenceTarget::Issue { .. } => "Issue",
                    ReferenceTarget::Link { .. } => "Link",
                };
                format!(
                    "{}\n  {kind} · {:?} · {:?}\n  {}\n  {}",
                    reference_text(&reference.name),
                    reference.role,
                    reference.state,
                    reference_text(&reference.purpose),
                    selector_text(&reference.target.key())
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    Output::plain(
        json!({"owner_host_id":stream.owner_host_id,"stream_id":stream.id,"references":stream.references}),
        plain,
    )
}
