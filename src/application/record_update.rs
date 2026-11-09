//! Exact-read shells for captured edits; each save replaces only its own target envelope.

use super::Result;
use crate::{
    agent::{RecordUpdate, record_update::CapturedEdit},
    codec::record_to_data,
    model::{Record, RecordKind, Stream, TitleDefaults, ValidationError},
    operations::{
        Actor, Reconciliation,
        edits::{SessionEdit, StreamEdit},
        references::{ReferenceChange, ReferenceChanges},
    },
    storage::records::{RecordAddress, RecordChange, RecordRequest, RecordSave, RecordStore},
};

pub fn save_record_update(store: &impl RecordStore, update: &RecordUpdate) -> Result<RecordSave> {
    store.save(&update.request, |reader, target| -> Result<RecordChange> {
        let target = target.ok_or_else(|| {
            ValidationError::new("captured edit target is missing; capture fresh inputs")
        })?;
        let decision = match (&update.edit, &target.record) {
            (CapturedEdit::Session(edit), Record::Session(session)) => {
                let address =
                    RecordAddress::new(&session.host_id, RecordKind::Stream, &session.stream_id)?;
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
                        let address =
                            RecordAddress::new(&session.host_id, RecordKind::Session, &session.id)?;
                        reader.read(&address)
                    })
                    .transpose()?
                    .flatten();
                let session = match &session {
                    Some(read) => {
                        let Record::Session(session) = &read.record else {
                            return Err(ValidationError::new("expected captured Session").into());
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
                            return Err(ValidationError::new("expected captured Session").into());
                        };
                        Some(session)
                    }
                    None => None,
                };
                let result = edit.apply(stream, session)?;
                let outcome = result.domain();
                let mut change = change(&target.record, result.reconciliation);
                change.outcome = outcome;
                return Ok(change);
            }
            _ => return Err(ValidationError::new("captured edit target kind changed").into()),
        };
        Ok(change(&target.record, decision))
    })
}

/// Human edits include repository configuration, outside the narrower agent wire.
pub fn save_user_edit(
    store: &impl RecordStore,
    operation_id: &str,
    before: &Record,
    changes: &serde_json::Map<String, serde_json::Value>,
    defaults: TitleDefaults,
) -> Result<RecordSave> {
    let edit = match before {
        Record::Stream(stream) => CapturedEdit::Stream(StreamEdit::new(
            operation_id,
            stream,
            changes,
            Actor::User,
            None,
            defaults.stream,
        )?),
        Record::Session(session) => CapturedEdit::Session(SessionEdit::new(
            operation_id,
            session,
            changes,
            Actor::User,
            defaults.session,
        )?),
        Record::Host(_) => {
            return Err(ValidationError::new("user edits require a Stream or Session").into());
        }
    };
    let title_default = match before {
        Record::Stream(_) => defaults.stream,
        _ => defaults.session,
    };
    let request = RecordRequest::new(
        RecordAddress::for_record(before),
        operation_id,
        &serde_json::json!({
            "command":"user-edit","before":record_to_data(before),"changes":changes,"title_default":title_default,
        }),
    )?;
    save_record_update(
        store,
        &RecordUpdate {
            request,
            edit,
            title_defaults: defaults,
        },
    )
}

/// Explicit user reference intent has only its Stream target, never a Session dependency.
pub fn save_user_references(
    store: &impl RecordStore,
    operation_id: &str,
    before: &Stream,
    changes: Vec<ReferenceChange>,
    defaults: TitleDefaults,
) -> Result<RecordSave> {
    let before_record = Record::Stream(before.clone());
    let request = RecordRequest::new(
        RecordAddress::for_record(&before_record),
        operation_id,
        &serde_json::json!({"command":"user-references", "before":record_to_data(&before_record),
            "changes":changes,"title_default":defaults.stream}),
    )?;
    let edit = ReferenceChanges::new(operation_id, before, Actor::User, None, changes)?;
    save_record_update(
        store,
        &RecordUpdate {
            request,
            edit: CapturedEdit::References(edit),
            title_defaults: defaults,
        },
    )
}

fn change(target: &Record, decision: Reconciliation) -> RecordChange {
    let outcome=serde_json::json!({"outcome":decision.outcome(),"conflicts":decision.conflicts(),"reason":decision.reason()})
        .as_object().expect("decision outcome is an object").clone();
    let record = match decision {
        Reconciliation::Changes { record, .. } => *record,
        _ => target.clone(),
    };
    RecordChange { record, outcome }
}
