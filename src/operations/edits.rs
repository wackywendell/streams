//! Pure captured edits. Conversation work and effort prose have different input guards.

use serde::Serialize;

use super::patches::{Access, Channel, Patch, PatchField, SessionPatch, StreamPatch};
use super::{Actor, Reconciliation};
use crate::model::{NamingMode, Session, Stream, TitleMode, ValidationError};

/// What a captured automatic report observes about its target when it is applied.
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

/// Why an automatic report may not replace text: a reason and the conflicting fields.
type Protection = Option<(&'static str, &'static [&'static str])>;

impl SessionEdit {
    pub fn new(
        before: &Session,
        patch: SessionPatch,
        actor: Actor,
        channel: Channel,
        title_default: NamingMode,
    ) -> Result<Self, ValidationError> {
        let patch = admitted(
            patch,
            actor,
            channel,
            &SessionPatch::protected(before, title_default),
        )?;
        if actor == Actor::Automatic && patch.status.is_some_and(|status| !status.reportable()) {
            return Err(ValidationError::new(
                "automatic reports cannot set session status to closed",
            ));
        }
        let after = applied(before, &patch.normalized(before, actor))?;
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
        let changed = SessionPatch::between(&self.before, &self.after);
        if self.actor == Actor::Automatic {
            let (admission, protection) =
                self.admission_and_protection(current, &changed, title_default);
            apply_automatic(
                current,
                &changed,
                AdmissionTarget::Session,
                admission,
                protection,
            )
        } else {
            apply_user_fields(&self.before, current, &changed)
        }
    }

    pub fn admission_context(
        &self,
        current: &Session,
        current_default: NamingMode,
    ) -> ReportAdmission {
        let changed = SessionPatch::between(&self.before, &self.after);
        self.admission_and_protection(current, &changed, current_default)
            .0
    }

    fn admission_and_protection(
        &self,
        current: &Session,
        changed: &SessionPatch,
        current_default: NamingMode,
    ) -> (ReportAdmission, Protection) {
        let protection = changed.title.as_ref().and_then(|_| {
            title_protection(
                self.before.title_mode,
                current.title_mode,
                self.title_default,
                current_default,
            )
        });
        let admission = ReportAdmission {
            in_captured_stream: same_captured_stream(&self.before, Some(current)),
            membership_unchanged: same_membership_period(&self.before, Some(current)),
            session_inputs_clean: prose_inputs_clean(&self.before, current, changed),
            session_protected: protection.is_some(),
            ..ReportAdmission::default()
        };
        (admission, protection)
    }
}

impl StreamEdit {
    pub fn new(
        before: &Stream,
        patch: StreamPatch,
        actor: Actor,
        channel: Channel,
        captured_session: Option<Session>,
        title_default: NamingMode,
    ) -> Result<Self, ValidationError> {
        let patch = admitted(
            patch,
            actor,
            channel,
            &StreamPatch::protected(before, title_default),
        )?;
        let after = applied(before, &patch.normalized(before, actor))?;
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
    /// The Stream this edit makes of its captured record.
    pub fn after(&self) -> &Stream {
        &self.after
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
        let changed = StreamPatch::between(&self.before, &self.after);
        if self.actor == Actor::Automatic {
            let (admission, protection) =
                self.admission_and_protection(current, session, &changed, title_default);
            apply_automatic(
                current,
                &changed,
                AdmissionTarget::Stream,
                admission,
                protection,
            )
        } else {
            apply_user_fields(&self.before, current, &changed)
        }
    }

    pub fn admission_context(
        &self,
        current: &Stream,
        session: Option<&Session>,
        current_default: NamingMode,
    ) -> ReportAdmission {
        let changed = StreamPatch::between(&self.before, &self.after);
        self.admission_and_protection(current, session, &changed, current_default)
            .0
    }

    fn admission_and_protection(
        &self,
        current: &Stream,
        session: Option<&Session>,
        changed: &StreamPatch,
        current_default: NamingMode,
    ) -> (ReportAdmission, Protection) {
        let title = changed.title.as_ref().and_then(|_| {
            title_protection(
                self.before.title_mode,
                current.title_mode,
                self.title_default,
                current_default,
            )
        });
        let goal = (current.goal_mode == NamingMode::Manual
            && (changed.goal.is_some() || changed.goal_short.is_some()))
        .then_some(("manual text is protected", &["goal_mode"][..]));
        let protection = title.or(goal);
        let captured = self.captured_session.as_ref();
        let admission = ReportAdmission {
            in_captured_stream: captured
                .is_some_and(|captured| same_captured_stream(captured, session)),
            membership_unchanged: captured
                .is_some_and(|captured| same_membership_period(captured, session)),
            stream_inputs_clean: prose_inputs_clean(&self.before, current, changed),
            stream_protected: protection.is_some(),
            ..ReportAdmission::default()
        };
        (admission, protection)
    }
}

/// The patch `actor` may submit through `channel`, without the fields the record protects
/// from automatic reports.
fn admitted<P: Patch>(
    mut patch: P,
    actor: Actor,
    channel: Channel,
    protected: &[P::Field],
) -> Result<P, ValidationError> {
    if actor == Actor::Native {
        return Err(ValidationError::new(
            "native membership changes use the move operation",
        ));
    }
    if let Some(field) = patch
        .fields()
        .into_iter()
        .find(|field| !field.access().permits(actor, channel))
    {
        return Err(ValidationError::new(format!(
            "field is not editable by this actor: {}",
            field.name()
        )));
    }
    if actor == Actor::Automatic {
        for field in protected {
            patch.remove(*field);
        }
    }
    Ok(patch)
}

fn applied<P: Patch>(before: &P::Record, patch: &P) -> Result<P::Record, ValidationError> {
    P::check(before)?;
    let mut after = before.clone();
    patch.apply(&mut after);
    P::check(&after)?;
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

/// Whether the record still holds the captured report prose and the fields this edit sets.
fn prose_inputs_clean<P: Patch>(before: &P::Record, current: &P::Record, changed: &P) -> bool {
    let drift = P::between(before, current);
    !drift.overlaps(changed)
        && drift
            .fields()
            .iter()
            .all(|field| field.access() != Access::Report)
}

fn title_protection(
    captured_mode: TitleMode,
    current_mode: TitleMode,
    captured_default: NamingMode,
    current_default: NamingMode,
) -> Protection {
    if captured_mode == TitleMode::Inherit && captured_default != current_default {
        return Some(("title generation default changed", &["title_default"]));
    }
    if current_mode.resolve(current_default) == NamingMode::Manual {
        return Some(("manual text is protected", &["title_mode"]));
    }
    None
}

fn apply_automatic<P: Patch>(
    current: &P::Record,
    changed: &P,
    target: AdmissionTarget,
    admission: ReportAdmission,
    protection: Protection,
) -> Reconciliation {
    if let Err(error) = P::check(current) {
        return refuse(
            Actor::Automatic,
            &error.to_string(),
            &["relationships"],
            false,
        );
    }
    let mut candidate = current.clone();
    changed.apply(&mut candidate);
    // A retried report that already landed, or one the record already holds, is no change.
    if candidate == *current {
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
    if let Err(error) = P::check(&candidate) {
        return refuse(
            Actor::Automatic,
            &error.to_string(),
            &["relationships"],
            false,
        );
    }
    Reconciliation::Changes {
        record: Box::new(P::into_record(candidate)),
    }
}

/// User edits compare and swap on the captured revision. A retry that already landed, or
/// intent the record already holds, is no change.
fn apply_user_fields<P: Patch>(
    before: &P::Record,
    current: &P::Record,
    changed: &P,
) -> Reconciliation {
    if let Err(error) = P::check(current) {
        return refuse(Actor::User, &error.to_string(), &["relationships"], false);
    }
    let mut candidate = current.clone();
    changed.apply(&mut candidate);
    if candidate == *current {
        return Reconciliation::no_change("");
    }
    if P::revision(current) != P::revision(before) {
        let mut conflicts: Vec<&str> = P::between(before, current)
            .fields()
            .into_iter()
            .map(PatchField::name)
            .collect();
        if conflicts.is_empty() {
            conflicts.push("revision");
        }
        return refuse(
            Actor::User,
            "record changed since capture; capture fresh inputs",
            &conflicts,
            false,
        );
    }
    if let Err(error) = P::check(&candidate) {
        return refuse(Actor::User, &error.to_string(), &["relationships"], false);
    }
    Reconciliation::Changes {
        record: Box::new(P::into_record(candidate)),
    }
}

#[cfg(test)]
mod tests;
