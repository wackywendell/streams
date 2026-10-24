//! Exact-read shells for captured edits; each save replaces only its own target envelope.

use super::Result;
use crate::{
    agent::{RecordUpdate, record_update::CapturedEdit},
    model::{Record, RecordKind, ValidationError},
    operations::Reconciliation,
    storage::records::{RecordAddress, RecordChange, RecordSave, RecordStore},
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
                edit.apply(session, &stream)
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
                edit.apply(stream, session)
            }
            _ => return Err(ValidationError::new("captured edit target kind changed").into()),
        };
        let record = match &decision {
            Reconciliation::Changes { record, .. } => record.as_ref().clone(),
            _ => target.record.clone(),
        };
        let outcome = serde_json::to_value(decision)
            .expect("validated reconciliation serializes")
            .as_object()
            .expect("reconciliation serializes an object")
            .clone();
        Ok(RecordChange { record, outcome })
    })
}
