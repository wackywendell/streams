//! Record-local edit application service for format-5 Streams and Sessions.
//!
//! Comparison, merge, destination Stream lookup, and `not_written` calculation all execute
//! inside `RecordStore::save`'s locked callback.

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    model::Record,
    operations::edits::{self, EditRequest},
    storage::records::{RecordAddress, RecordChange, RecordStore},
};

use super::{
    ApplicationError,
    fields::{SESSION_FIELDS, STREAM_FIELDS, classify_editable},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EditResult {
    pub key: String,
    pub not_written: Map<String, Value>,
    #[serde(skip)]
    pub written: bool,
}

impl EditResult {
    pub fn is_complete(&self) -> bool {
        self.not_written.is_empty()
    }
}

pub fn edit_stream(
    store: &impl RecordStore,
    target: &RecordAddress,
    request: &EditRequest,
) -> Result<EditResult, ApplicationError> {
    let key = target.full_key();
    let save = store.save(target, |_reader, before| {
        let Some(Record::Stream(current)) = before else {
            return Err(ApplicationError::not_found(format!("no Stream {key}")));
        };
        let outcome = edits::apply_stream(current, request, |field| {
            classify_editable(STREAM_FIELDS, "Stream", field)
        })?;
        Ok(RecordChange {
            record: Record::Stream(outcome.after),
            outcome: outcome.not_written,
        })
    })?;
    Ok(EditResult {
        key: save.target(),
        not_written: save.outcome,
        written: save.written,
    })
}

pub fn edit_session(
    store: &impl RecordStore,
    target: &RecordAddress,
    request: &EditRequest,
) -> Result<EditResult, ApplicationError> {
    let key = target.full_key();
    let save = store.save(target, |reader, before| {
        let Some(Record::Session(current)) = before else {
            return Err(ApplicationError::not_found(format!("no Session {key}")));
        };
        let outcome = edits::apply_session(current, request, |field| {
            classify_editable(SESSION_FIELDS, "Session", field)
        })?;
        if let Some(dest_stream_id) = &outcome.target_stream_id
            && reader
                .read_stream(&current.host_id, dest_stream_id)?
                .is_none()
        {
            return Err(ApplicationError::not_found(format!(
                "no Stream {}/{dest_stream_id}",
                current.host_id
            )));
        }
        Ok(RecordChange {
            record: Record::Session(outcome.after),
            outcome: outcome.not_written,
        })
    })?;
    Ok(EditResult {
        key: save.target(),
        not_written: save.outcome,
        written: save.written,
    })
}
