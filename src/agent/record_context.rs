//! Record-local capture. Membership comes only from the addressed conversation record.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    model::{Record, RecordKind, Session, Stream, ValidationError, check_text},
    storage::{
        StorageError,
        records::{RecordAddress, RecordReader},
    },
};

use super::{ContextRequest, session_id_for};

#[derive(Debug, thiserror::Error)]
pub enum RecordContextError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Captured inputs for this conversation, not an enumeration of the effort's contributors.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordContext {
    pub catalog_source: String,
    pub host_id: String,
    pub stream: Option<Stream>,
    pub session: Option<Session>,
    pub sessions: Vec<Session>,
}

/// The caller holds the admitted record reader; no discovery, history or native evidence.
pub fn record_context(
    reader: &dyn RecordReader,
    host: &str,
    catalog_source: &str,
    request: ContextRequest<'_>,
) -> Result<RecordContext, RecordContextError> {
    check_text(host, "host_id", false)?;
    check_text(catalog_source, "catalog_source", false)?;
    check_text(request.native_id, "harness_session_id", false)?;
    let address = RecordAddress::new(
        host,
        RecordKind::Session,
        session_id_for(host, request.harness, request.native_id),
    )?;
    let session = reader
        .read(&address)?
        .map(|read| match read.record {
            Record::Session(session) => Ok(session),
            _ => Err(ValidationError::new("expected addressed Session record")),
        })
        .transpose()?;
    let stream_id = if let Some(session) = &session {
        if request.stream_id.is_some_and(|id| id != session.stream_id) {
            return Err(ValidationError::new(
                "explicit Stream does not match the conversation's current membership",
            )
            .into());
        }
        // Ambient native associations may predate a valid move. They cannot override membership.
        Some(session.stream_id.as_str())
    } else {
        let mut hints = BTreeSet::new();
        for id in [
            request.stream_id,
            request.environment_stream_id,
            request.workspace_stream_id,
        ]
        .into_iter()
        .flatten()
        {
            check_text(id, "stream_id", false)?;
            hints.insert(id);
        }
        if hints.len() > 1 {
            return Err(ValidationError::new(
                "conflicting Stream hints for an unenrolled conversation",
            )
            .into());
        }
        hints.first().copied()
    };
    let stream = stream_id
        .map(|id| -> Result<Stream, RecordContextError> {
            let address = RecordAddress::new(host, RecordKind::Stream, id)?;
            let Some(read) = reader.read(&address)? else {
                return Err(ValidationError::new(
                    "addressed Stream is missing; run explicit catalog check or sync",
                )
                .into());
            };
            let Record::Stream(stream) = read.record else {
                return Err(ValidationError::new("expected addressed Stream record").into());
            };
            if stream.owner_host_id != host {
                return Err(ValidationError::new(
                    "conversation and Stream must belong to this producer",
                )
                .into());
            }
            Ok(stream)
        })
        .transpose()?;
    Ok(RecordContext {
        catalog_source: catalog_source.into(),
        host_id: host.into(),
        sessions: session.iter().cloned().collect(),
        stream,
        session,
    })
}

#[cfg(test)]
mod tests;
