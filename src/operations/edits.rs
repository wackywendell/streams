//! Pure captured edits. Conversation work and effort prose have different input guards.

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Map, Value};

use super::{
    Actor, Reconciliation, agent_field_allowed, apply_changes, copy_field, field_eq, field_policy,
    filter_automatic_changes, record_field_names,
};
use crate::model::{
    NamingMode, Record, Session, Stream, TitleDefaults, TitleMode, ValidationError,
};

/// What a captured automatic report observes about its target when it is applied.
/// `specs/report_admission.ioa.toml` models the same predicate; `tests/verification.rs`
/// checks that the two agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportAdmission {
    pub in_captured_stream: bool,
    pub membership_unchanged: bool,
    pub session_inputs_clean: bool,
    pub stream_inputs_clean: bool,
    pub session_protected: bool,
    pub stream_protected: bool,
}

impl Default for ReportAdmission {
    fn default() -> Self {
        Self {
            in_captured_stream: true,
            membership_unchanged: true,
            session_inputs_clean: true,
            stream_inputs_clean: true,
            session_protected: false,
            stream_protected: false,
        }
    }
}

impl ReportAdmission {
    pub fn admits_session(self) -> bool {
        self.session_inputs_clean && !self.session_protected
    }

    pub fn admits_stream(self) -> bool {
        self.membership_unchanged && self.stream_inputs_clean && !self.stream_protected
    }

    fn admits(self, target: AdmissionTarget) -> bool {
        match target {
            AdmissionTarget::Session => self.admits_session(),
            AdmissionTarget::Stream => self.admits_stream(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdmissionTarget {
    Session,
    Stream,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionEdit {
    before: Session,
    after: Session,
    actor: Actor,
    title_default: NamingMode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamEdit {
    before: Stream,
    after: Stream,
    actor: Actor,
    captured_session: Option<Session>,
    title_default: NamingMode,
}

impl SessionEdit {
    pub fn new(
        before: &Session,
        changes: &Map<String, Value>,
        actor: Actor,
        title_default: NamingMode,
    ) -> Result<Self, ValidationError> {
        let Record::Session(after) = prepare(
            &Record::Session(before.clone()),
            changes,
            actor,
            title_default,
        )?
        else {
            unreachable!("normalization preserves record kind")
        };
        Ok(Self {
            before: before.clone(),
            after,
            actor,
            title_default,
        })
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
                self.actor,
                "current conversation/Stream relationship is invalid",
                &["relationships"],
                false,
            );
        }
        if let Err(error) = stream.validate() {
            return refuse(self.actor, &error.to_string(), &["relationships"], false);
        }
        let before = Record::Session(self.before.clone());
        let after = Record::Session(self.after.clone());
        let current_record = Record::Session(current.clone());
        let changed = changed_fields(&before, &after);
        if self.actor == Actor::Automatic {
            let (admission, protection) = self.admission_and_protection(
                &before,
                &current_record,
                current,
                &changed,
                title_default,
            );
            apply_automatic(
                &before,
                &after,
                &current_record,
                &changed,
                AdmissionTarget::Session,
                admission,
                protection,
            )
        } else {
            apply_user_fields(&before, &after, &current_record, &changed)
        }
    }

    pub fn admission_context(
        &self,
        current: &Session,
        current_default: NamingMode,
    ) -> ReportAdmission {
        let before = Record::Session(self.before.clone());
        let after = Record::Session(self.after.clone());
        let current_record = Record::Session(current.clone());
        let changed = changed_fields(&before, &after);
        self.admission_and_protection(&before, &current_record, current, &changed, current_default)
            .0
    }

    fn admission_and_protection(
        &self,
        before: &Record,
        current_record: &Record,
        current: &Session,
        changed: &BTreeSet<&'static str>,
        current_default: NamingMode,
    ) -> (
        ReportAdmission,
        Option<(&'static str, &'static [&'static str])>,
    ) {
        let protection = automatic_protection_conflict(
            before,
            current_record,
            changed,
            self.title_default,
            current_default,
        );
        let admission = ReportAdmission {
            in_captured_stream: same_captured_stream(&self.before, Some(current)),
            membership_unchanged: same_membership_period(&self.before, Some(current)),
            session_inputs_clean: prose_inputs_clean(before, current_record, changed),
            session_protected: protection.is_some(),
            ..ReportAdmission::default()
        };
        (admission, protection)
    }
}

impl StreamEdit {
    pub fn new(
        before: &Stream,
        changes: &Map<String, Value>,
        actor: Actor,
        captured_session: Option<Session>,
        title_default: NamingMode,
    ) -> Result<Self, ValidationError> {
        let Record::Stream(after) = prepare(
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
            before: before.clone(),
            after,
            actor,
            captured_session,
            title_default,
        })
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
                self.actor,
                "target Stream identity changed",
                &["identity"],
                false,
            );
        }
        let before = Record::Stream(self.before.clone());
        let after = Record::Stream(self.after.clone());
        let current_record = Record::Stream(current.clone());
        let changed = changed_fields(&before, &after);
        if self.actor == Actor::Automatic {
            let (admission, protection) = self.admission_and_protection(
                &before,
                &current_record,
                session,
                &changed,
                title_default,
            );
            apply_automatic(
                &before,
                &after,
                &current_record,
                &changed,
                AdmissionTarget::Stream,
                admission,
                protection,
            )
        } else {
            apply_user_fields(&before, &after, &current_record, &changed)
        }
    }

    pub fn admission_context(
        &self,
        current: &Stream,
        session: Option<&Session>,
        current_default: NamingMode,
    ) -> ReportAdmission {
        let before = Record::Stream(self.before.clone());
        let after = Record::Stream(self.after.clone());
        let current_record = Record::Stream(current.clone());
        let changed = changed_fields(&before, &after);
        self.admission_and_protection(&before, &current_record, session, &changed, current_default)
            .0
    }

    fn admission_and_protection(
        &self,
        before: &Record,
        current_record: &Record,
        session: Option<&Session>,
        changed: &BTreeSet<&'static str>,
        current_default: NamingMode,
    ) -> (
        ReportAdmission,
        Option<(&'static str, &'static [&'static str])>,
    ) {
        let protection = automatic_protection_conflict(
            before,
            current_record,
            changed,
            self.title_default,
            current_default,
        );
        let captured = self.captured_session.as_ref();
        let admission = ReportAdmission {
            in_captured_stream: captured
                .is_some_and(|captured| same_captured_stream(captured, session)),
            membership_unchanged: captured
                .is_some_and(|captured| same_membership_period(captured, session)),
            stream_inputs_clean: prose_inputs_clean(before, current_record, changed),
            stream_protected: protection.is_some(),
            ..ReportAdmission::default()
        };
        (admission, protection)
    }
}

/// Combined state projection for a captured conversation/effort report pair.
pub fn report_admission_context(
    session_edit: &SessionEdit,
    stream_edit: &StreamEdit,
    current_session: &Session,
    current_stream: &Stream,
    current_defaults: TitleDefaults,
) -> ReportAdmission {
    let session = session_edit.admission_context(current_session, current_defaults.session);
    let stream = stream_edit.admission_context(
        current_stream,
        Some(current_session),
        current_defaults.stream,
    );
    ReportAdmission {
        session_inputs_clean: session.session_inputs_clean,
        session_protected: session.session_protected,
        ..stream
    }
}

fn prepare(
    before: &Record,
    changes: &Map<String, Value>,
    actor: Actor,
    title_default: NamingMode,
) -> Result<Record, ValidationError> {
    before.validate_portable()?;
    if actor == Actor::Native {
        return Err(ValidationError::new(
            "native membership changes use the move operation",
        ));
    }
    for field in changes.keys() {
        if ["owner_host_id", "stream_id", "membership_epoch"].contains(&field.as_str())
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

fn refuse(actor: Actor, reason: &str, fields: &[&str], stale: bool) -> Reconciliation {
    let conflicts = fields.iter().map(|field| (*field).into()).collect();
    if stale && actor == Actor::Automatic {
        Reconciliation::Superseded {
            conflicts,
            reason: reason.into(),
        }
    } else {
        Reconciliation::Conflict {
            conflicts,
            reason: reason.into(),
        }
    }
}

fn changed_fields(before: &Record, after: &Record) -> BTreeSet<&'static str> {
    record_field_names(after)
        .iter()
        .copied()
        .filter(|field| field_eq(before, after, field) == Some(false))
        .collect()
}

fn same_captured_stream(captured: &Session, current: Option<&Session>) -> bool {
    current.is_some_and(|session| {
        session.id == captured.id
            && session.native_identity() == captured.native_identity()
            && session.host_id == captured.host_id
            && session.stream_id == captured.stream_id
    })
}

fn same_membership_period(captured: &Session, current: Option<&Session>) -> bool {
    same_captured_stream(captured, current)
        && current.is_some_and(|session| session.membership_epoch == captured.membership_epoch)
}

fn prose_inputs_clean(before: &Record, current: &Record, changed: &BTreeSet<&'static str>) -> bool {
    record_field_names(before).iter().all(|field| {
        !(agent_field_allowed(before.kind(), Actor::Automatic, field) || changed.contains(field))
            || field_eq(before, current, field) == Some(true)
    })
}

fn automatic_protection_conflict(
    before: &Record,
    current: &Record,
    changed: &BTreeSet<&'static str>,
    captured_default: NamingMode,
    current_default: NamingMode,
) -> Option<(&'static str, &'static [&'static str])> {
    if changed.contains("title") {
        let captured_mode = match before {
            Record::Stream(stream) => stream.title_mode,
            Record::Session(session) => session.title_mode,
            _ => unreachable!(),
        };
        if captured_mode == TitleMode::Inherit && captured_default != current_default {
            return Some(("title generation default changed", &["title_default"]));
        }
        let current_mode = match current {
            Record::Stream(stream) => stream.title_mode,
            Record::Session(session) => session.title_mode,
            _ => unreachable!(),
        };
        if current_mode.resolve(current_default) == NamingMode::Manual {
            return Some(("manual text is protected", &["title_mode"]));
        }
    }
    if let Record::Stream(stream) = current
        && stream.goal_mode == NamingMode::Manual
        && (changed.contains("goal") || changed.contains("goal_short"))
    {
        return Some(("manual text is protected", &["goal_mode"]));
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn apply_automatic(
    _before: &Record,
    after: &Record,
    current: &Record,
    changed: &BTreeSet<&'static str>,
    target: AdmissionTarget,
    admission: ReportAdmission,
    protection: Option<(&'static str, &'static [&'static str])>,
) -> Reconciliation {
    if let Err(error) = current.validate_portable() {
        return refuse(
            Actor::Automatic,
            &error.to_string(),
            &["relationships"],
            false,
        );
    }
    // A retried report that already landed, or one the record already holds, is no change.
    if changed
        .iter()
        .all(|field| field_eq(current, after, field) == Some(true))
    {
        return Reconciliation::no_change("");
    }
    if target == AdmissionTarget::Stream && !admission.membership_unchanged {
        return refuse(
            Actor::Automatic,
            "captured conversation or membership period changed",
            &["inputs"],
            true,
        );
    }
    if let Some((reason, fields)) = protection {
        return refuse(Actor::Automatic, reason, fields, true);
    }
    if !admission.admits(target) {
        return refuse(
            Actor::Automatic,
            "generated description inputs changed",
            &["inputs"],
            true,
        );
    }
    let mut candidate = current.clone();
    for field in changed {
        copy_field(&mut candidate, after, field);
    }
    if let Err(error) = candidate.validate_portable() {
        return refuse(
            Actor::Automatic,
            &error.to_string(),
            &["relationships"],
            false,
        );
    }
    if &candidate == current {
        Reconciliation::no_change("")
    } else {
        Reconciliation::Changes {
            record: Box::new(candidate),
        }
    }
}

/// User edits compare and swap on the captured revision. A retry that already landed, or
/// intent the record already holds, is no change.
fn apply_user_fields(
    before: &Record,
    after: &Record,
    current: &Record,
    changed: &BTreeSet<&'static str>,
) -> Reconciliation {
    if let Err(error) = current.validate_portable() {
        return refuse(Actor::User, &error.to_string(), &["relationships"], false);
    }
    if changed
        .iter()
        .all(|field| field_eq(current, after, field) == Some(true))
    {
        return Reconciliation::no_change("");
    }
    if current.revision() != before.revision() {
        let conflicts: Vec<&str> = record_field_names(before)
            .iter()
            .copied()
            .filter(|field| field_eq(current, before, field) == Some(false))
            .collect();
        return refuse(
            Actor::User,
            "record changed since capture; capture fresh inputs",
            &conflicts,
            false,
        );
    }
    let mut candidate = current.clone();
    for field in changed {
        copy_field(&mut candidate, after, field);
    }
    if let Err(error) = candidate.validate_portable() {
        return refuse(Actor::User, &error.to_string(), &["relationships"], false);
    }
    Reconciliation::Changes {
        record: Box::new(candidate),
    }
}

#[cfg(test)]
mod tests;
