//! Exact-read shells for captured edits; each save replaces only its own target record.

use super::Result;
use crate::{
    agent::{RecordUpdate, record_update::CapturedEdit},
    model::{Record, RecordKind, Stream, TitleDefaults, ValidationError},
    operations::{
        Actor, EditOutcome, Reconciliation,
        edits::{SessionEdit, StreamEdit},
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
            let decision = match (&update.edit, &target.record) {
                (CapturedEdit::Session(edit), Record::Session(session)) => {
                    let address = RecordAddress::new(
                        &session.host_id,
                        RecordKind::Stream,
                        &session.stream_id,
                    )?;
                    let dependency = reader
                        .read(&address)?
                        .ok_or_else(|| ValidationError::new("current Session Stream is missing"))?;
                    let Record::Stream(stream) = dependency.record else {
                        return Err(ValidationError::new("expected current Stream").into());
                    };
                    edit.apply(session, &stream, update.title_defaults.session)
                }
                (CapturedEdit::Stream(edit), Record::Stream(stream)) => {
                    let session = edit
                        .captured_session()
                        .map(|session| {
                            let address = RecordAddress::new(
                                &session.host_id,
                                RecordKind::Session,
                                &session.id,
                            )?;
                            reader.read(&address)
                        })
                        .transpose()?
                        .flatten();
                    let session = match &session {
                        Some(read) => {
                            let Record::Session(session) = &read.record else {
                                return Err(
                                    ValidationError::new("expected captured Session").into()
                                );
                            };
                            Some(session)
                        }
                        None => None,
                    };
                    edit.apply(stream, session, update.title_defaults.stream)
                }
                (CapturedEdit::References(edit), Record::Stream(stream)) => {
                    let session = edit
                        .captured_session()
                        .map(|captured| {
                            let address = RecordAddress::new(
                                &captured.host_id,
                                RecordKind::Session,
                                &captured.id,
                            )?;
                            reader.read(&address)
                        })
                        .transpose()?
                        .flatten();
                    let session = match &session {
                        Some(read) => {
                            let Record::Session(session) = &read.record else {
                                return Err(
                                    ValidationError::new("expected captured Session").into()
                                );
                            };
                            Some(session)
                        }
                        None => None,
                    };
                    let result = edit.apply(stream, session)?;
                    let outcome = result.outcome();
                    let mut change = change(&target.record, result.reconciliation);
                    change.outcome = outcome;
                    return Ok(change);
                }
                _ => return Err(ValidationError::new("captured edit target kind changed").into()),
            };
            Ok(change(&target.record, decision))
        },
    )
}

/// Human edits include repository configuration, outside the narrower agent wire.
pub fn save_user_edit(
    store: &impl RecordStore,
    before: &Record,
    changes: &serde_json::Map<String, serde_json::Value>,
    defaults: TitleDefaults,
) -> Result<RecordSave<EditOutcome>> {
    let edit = match before {
        Record::Stream(stream) => CapturedEdit::Stream(StreamEdit::new(
            stream,
            changes,
            Actor::User,
            None,
            defaults.stream,
        )?),
        Record::Session(session) => CapturedEdit::Session(SessionEdit::new(
            session,
            changes,
            Actor::User,
            defaults.session,
        )?),
        Record::Host(_) => {
            return Err(ValidationError::new("user edits require a Stream or Session").into());
        }
    };
    save_record_update(
        store,
        &RecordUpdate {
            target: RecordAddress::for_record(before),
            edit,
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
            target: RecordAddress::for_record(&Record::Stream(before.clone())),
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
