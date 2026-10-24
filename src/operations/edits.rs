//! Pure captured edits. Conversation work and effort prose have different input guards.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    Actor, Reconciliation, apply_changes, copy_field, field_eq, field_policy,
    filter_automatic_changes, merge_additive, record_field_names, text_group_guards,
};
use crate::model::{NamingMode, Record, Session, Stream, TitleMode, ValidationError, check_text};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionEdit {
    operation_id: String,
    before: Session,
    after: Session,
    actor: Actor,
    title_default: NamingMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamEdit {
    operation_id: String,
    before: Stream,
    after: Stream,
    actor: Actor,
    captured_session: Option<Session>,
    title_default: NamingMode,
}

impl SessionEdit {
    pub fn new(
        operation_id: &str,
        before: &Session,
        changes: &Map<String, Value>,
        actor: Actor,
        title_default: NamingMode,
    ) -> Result<Self, ValidationError> {
        let Record::Session(after) = prepare(
            operation_id,
            &Record::Session(before.clone()),
            changes,
            actor,
            title_default,
        )?
        else {
            unreachable!("normalization preserves record kind")
        };
        Ok(Self {
            operation_id: operation_id.into(),
            before: before.clone(),
            after,
            actor,
            title_default,
        })
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn before(&self) -> &Session {
        &self.before
    }

    /// A valid move does not invalidate conversation-local work or retarget it to another record.
    pub fn apply(
        &self,
        current: &Session,
        stream: &Stream,
        title_default: NamingMode,
    ) -> Reconciliation {
        if current.id != self.before.id
            || current.native_identity() != self.before.native_identity()
            || current.stream_id != stream.id
            || current.host_id != stream.owner_host_id
        {
            return refuse(
                &self.operation_id,
                self.actor,
                "current conversation/Stream relationship is invalid",
                &["relationships"],
                false,
            );
        }
        if let Err(error) = stream.validate() {
            return refuse(
                &self.operation_id,
                self.actor,
                &error.to_string(),
                &["relationships"],
                false,
            );
        }
        apply_fields(
            &self.operation_id,
            self.actor,
            &Record::Session(self.before.clone()),
            &Record::Session(self.after.clone()),
            &Record::Session(current.clone()),
            self.title_default,
            title_default,
        )
    }
}

impl StreamEdit {
    pub fn new(
        operation_id: &str,
        before: &Stream,
        changes: &Map<String, Value>,
        actor: Actor,
        captured_session: Option<Session>,
        title_default: NamingMode,
    ) -> Result<Self, ValidationError> {
        let Record::Stream(after) = prepare(
            operation_id,
            &Record::Stream(before.clone()),
            changes,
            actor,
            title_default,
        )?
        else {
            unreachable!("normalization preserves record kind")
        };
        if actor == Actor::User && captured_session.is_some() {
            return Err(ValidationError::new(
                "user Stream edits cannot capture a Session dependency",
            ));
        }
        if actor == Actor::Automatic && captured_session.is_none() {
            return Err(ValidationError::new(
                "generated Stream edits require their captured conversation",
            ));
        }
        if let Some(session) = &captured_session {
            session.validate_portable()?;
            if session.host_id != before.owner_host_id || session.stream_id != before.id {
                return Err(ValidationError::new(
                    "captured conversation does not belong to the target Stream",
                ));
            }
        }
        Ok(Self {
            operation_id: operation_id.into(),
            before: before.clone(),
            after,
            actor,
            captured_session,
            title_default,
        })
    }
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn before(&self) -> &Stream {
        &self.before
    }
    pub fn captured_session(&self) -> Option<&Session> {
        self.captured_session.as_ref()
    }

    /// Compare the captured membership period, including an A → B → A return.
    pub fn apply(
        &self,
        current: &Stream,
        session: Option<&Session>,
        title_default: NamingMode,
    ) -> Reconciliation {
        if current.id != self.before.id || current.owner_host_id != self.before.owner_host_id {
            return refuse(
                &self.operation_id,
                self.actor,
                "target Stream identity changed",
                &["identity"],
                false,
            );
        }
        if let Some(captured) = &self.captured_session
            && session != Some(captured)
        {
            return refuse(
                &self.operation_id,
                self.actor,
                "captured conversation or membership period changed",
                &["inputs"],
                true,
            );
        }
        apply_fields(
            &self.operation_id,
            self.actor,
            &Record::Stream(self.before.clone()),
            &Record::Stream(self.after.clone()),
            &Record::Stream(current.clone()),
            self.title_default,
            title_default,
        )
    }
}

fn prepare(
    id: &str,
    before: &Record,
    changes: &Map<String, Value>,
    actor: Actor,
    title_default: NamingMode,
) -> Result<Record, ValidationError> {
    check_text(id, "operation_id", false)?;
    before.validate_portable()?;
    if actor == Actor::Native {
        return Err(ValidationError::new(
            "native membership changes use the move operation",
        ));
    }
    for field in changes.keys() {
        if ["owner_host_id", "stream_id", "previous_stream_ids"].contains(&field.as_str())
            || field_policy(before.kind(), field)
                .is_none_or(|policy| actor == Actor::Automatic && !policy.automatic)
        {
            return Err(ValidationError::new(format!(
                "field is not editable by this actor: {field}"
            )));
        }
    }
    let changes = if actor == Actor::Automatic {
        filter_automatic_changes(before, changes, title_default)
    } else {
        changes.clone()
    };
    let after = apply_changes(before, &changes, actor)?;
    after.validate_portable()?;
    Ok(after)
}

fn refuse(id: &str, actor: Actor, reason: &str, fields: &[&str], stale: bool) -> Reconciliation {
    let conflicts = fields.iter().map(|field| (*field).into()).collect();
    if stale && actor == Actor::Automatic {
        Reconciliation::Superseded {
            operation_id: id.into(),
            conflicts,
            reason: reason.into(),
        }
    } else {
        Reconciliation::Conflict {
            operation_id: id.into(),
            conflicts,
            reason: reason.into(),
        }
    }
}

fn apply_fields(
    id: &str,
    actor: Actor,
    before: &Record,
    after: &Record,
    current: &Record,
    captured_default: NamingMode,
    current_default: NamingMode,
) -> Reconciliation {
    if let Err(error) = current.validate_portable() {
        return refuse(id, actor, &error.to_string(), &["relationships"], false);
    }
    let changed: BTreeSet<_> = record_field_names(after)
        .iter()
        .copied()
        .filter(|field| field_eq(before, after, field) == Some(false))
        .collect();
    if actor == Actor::Automatic {
        if changed.contains("title") {
            let captured_mode = match before {
                Record::Stream(stream) => stream.title_mode,
                Record::Session(session) => session.title_mode,
                _ => unreachable!(),
            };
            if captured_mode == TitleMode::Inherit && captured_default != current_default {
                return refuse(
                    id,
                    actor,
                    "title generation default changed",
                    &["title_default"],
                    true,
                );
            }
            let current_mode = match current {
                Record::Stream(stream) => stream.title_mode,
                Record::Session(session) => session.title_mode,
                _ => unreachable!(),
            };
            if current_mode.resolve(current_default) == NamingMode::Manual {
                return refuse(id, actor, "manual text is protected", &["title_mode"], true);
            }
        }
        if let Record::Stream(stream) = current
            && stream.goal_mode == NamingMode::Manual
            && (changed.contains("goal") || changed.contains("goal_short"))
        {
            return refuse(id, actor, "manual text is protected", &["goal_mode"], true);
        }
    }
    if changed
        .iter()
        .all(|field| field_eq(current, after, field) == Some(true))
    {
        return Reconciliation::no_change(id, "");
    }
    if actor == Actor::Automatic
        && record_field_names(before).iter().any(|field| {
            // Membership is not an input to conversation-local work. All its other fields are.
            !(matches!(before, Record::Session(_))
                && ["stream_id", "previous_stream_ids"].contains(field))
                && field_eq(before, current, field) != Some(true)
        })
    {
        return refuse(
            id,
            actor,
            "generated description inputs changed",
            &["inputs"],
            true,
        );
    }
    let mut candidate = current.clone();
    let mut conflicts = Vec::new();
    for field in text_group_guards(after.kind(), changed.clone()) {
        if field_eq(current, before, field) == Some(true)
            || field_eq(current, after, field) == Some(true)
        {
            if changed.contains(field) {
                copy_field(&mut candidate, after, field);
            }
            continue;
        }
        if actor == Actor::User
            && let (
                Record::Stream(base),
                Record::Stream(desired),
                Record::Stream(actual),
                Record::Stream(target),
            ) = (before, after, current, &mut candidate)
        {
            match field {
                "documents" => {
                    if let Some(merged) = merge_additive(
                        &base.documents,
                        &desired.documents,
                        &actual.documents,
                        |a, b| a.identity() == b.identity(),
                    ) {
                        target.documents = merged;
                        continue;
                    }
                }
                "issues" => {
                    if let Some(merged) =
                        merge_additive(&base.issues, &desired.issues, &actual.issues, |a, b| {
                            a.url == b.url
                        })
                    {
                        target.issues = merged;
                        continue;
                    }
                }
                "prs" => {
                    if let Some(merged) =
                        merge_additive(&base.prs, &desired.prs, &actual.prs, |a, b| a.url == b.url)
                    {
                        target.prs = merged;
                        continue;
                    }
                }
                _ => {}
            }
        }
        conflicts.push(field);
    }
    if !conflicts.is_empty() {
        return refuse(
            id,
            actor,
            "concurrent intent conflicts with captured base",
            &conflicts,
            false,
        );
    }
    if let Err(error) = candidate.validate_portable() {
        return refuse(id, actor, &error.to_string(), &["relationships"], false);
    }
    if &candidate == current {
        Reconciliation::no_change(id, "")
    } else {
        Reconciliation::Changes {
            operation_id: id.into(),
            record: Box::new(candidate),
        }
    }
}

#[cfg(test)]
mod tests;
