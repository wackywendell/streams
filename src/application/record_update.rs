//! Exact-read shells for captured edits; each save replaces only its own target record.

use super::Result;
use crate::{
    agent::{RecordUpdate, record_update::CapturedEdit},
    model::{Record, Session, Stream, TitleDefaults, ValidationError},
    operations::{
        Actor, EditOutcome, Reconciliation,
        edits::{SessionEdit, StreamEdit},
        patches::{Channel, SessionPatch, StreamPatch},
        references::{ReferenceChange, ReferenceChanges},
    },
    storage::records::{RecordAddress, RecordChange, RecordSave, RecordStore},
};

pub fn save_record_update(
    store: &impl RecordStore,
    update: &RecordUpdate,
) -> Result<RecordSave<EditOutcome>> {
    store.save(
        &update.target,
        |reader, target| -> Result<RecordChange<EditOutcome>> {
            let target = target.ok_or_else(|| {
                ValidationError::new("captured edit target is missing; capture fresh inputs")
            })?;
            let captured_session = |captured: Option<&Session>| {
                captured
                    .map(|session| reader.read_session(&session.host_id, &session.id))
                    .transpose()
                    .map(Option::flatten)
            };
            let decision = match (&update.edit, target) {
                (CapturedEdit::Session(edit), Record::Session(session)) => {
                    let stream = reader
                        .read_stream(&session.host_id, &session.stream_id)?
                        .ok_or_else(|| ValidationError::new("current Session Stream is missing"))?;
                    edit.apply(session, &stream, update.title_defaults.session)
                }
                (CapturedEdit::Stream(edit), Record::Stream(stream)) => {
                    let session = captured_session(edit.captured_session())?;
                    edit.apply(stream, session.as_ref(), update.title_defaults.stream)
                }
                (CapturedEdit::References(edit), Record::Stream(stream)) => {
                    let session = captured_session(edit.captured_session())?;
                    let result = edit.apply(stream, session.as_ref())?;
                    let outcome = result.outcome();
                    let mut change = change(target, result.reconciliation);
                    change.outcome = outcome;
                    return Ok(change);
                }
                _ => return Err(ValidationError::new("captured edit target kind changed").into()),
            };
            Ok(change(target, decision))
        },
    )
}

/// A direct user edit of a Stream, including repository configuration, which the agent
/// wire cannot carry.
pub fn save_stream_edit(
    store: &impl RecordStore,
    before: &Stream,
    patch: StreamPatch,
    defaults: TitleDefaults,
) -> Result<RecordSave<EditOutcome>> {
    let edit = StreamEdit::new(
        before,
        patch,
        Actor::User,
        Channel::Cli,
        None,
        defaults.stream,
    )?;
    save_record_update(
        store,
        &RecordUpdate {
            target: RecordAddress::stream(before),
            edit: CapturedEdit::Stream(edit),
            title_defaults: defaults,
        },
    )
}

/// A direct user edit of a Session.
pub fn save_session_edit(
    store: &impl RecordStore,
    before: &Session,
    patch: SessionPatch,
    defaults: TitleDefaults,
) -> Result<RecordSave<EditOutcome>> {
    let edit = SessionEdit::new(before, patch, Actor::User, Channel::Cli, defaults.session)?;
    save_record_update(
        store,
        &RecordUpdate {
            target: RecordAddress::session(before),
            edit: CapturedEdit::Session(edit),
            title_defaults: defaults,
        },
    )
}

/// Explicit user reference intent has only its Stream target, never a Session dependency.
pub fn save_user_references(
    store: &impl RecordStore,
    before: &Stream,
    changes: Vec<ReferenceChange>,
    defaults: TitleDefaults,
) -> Result<RecordSave<EditOutcome>> {
    let edit = ReferenceChanges::new(before, Actor::User, None, changes)?;
    save_record_update(
        store,
        &RecordUpdate {
            target: RecordAddress::stream(before),
            edit: CapturedEdit::References(edit),
            title_defaults: defaults,
        },
    )
}

fn change(target: &Record, decision: Reconciliation) -> RecordChange<EditOutcome> {
    let outcome = EditOutcome::of(&decision);
    let record = match decision {
        Reconciliation::Changes { record, .. } => *record,
        _ => target.clone(),
    };
    RecordChange { record, outcome }
}
