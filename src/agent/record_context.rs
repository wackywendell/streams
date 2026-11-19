//! Record-local capture. Membership comes only from the addressed conversation record.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    model::{
        Record, RecordKind, Session, Status, Stream, TitleDefaults, ValidationError, check_text,
    },
    operations::{derived_stream_status, patches::PatchSchemas},
    runtime::conversations::Harness,
    storage::{
        StorageError,
        records::{RecordAddress, RecordReader},
    },
};

/// Exact selectors; ambient hints cannot replace a saved Session's membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextRequest<'a> {
    pub harness: Harness,
    pub native_id: &'a str,
    pub stream_id: Option<&'a str>,
    pub environment_stream_id: Option<&'a str>,
    pub workspace_stream_id: Option<&'a str>,
}

/// The same native key used by Session construction and canonical record paths.
pub fn session_id_for(host_id: &str, harness: Harness, native_id: &str) -> String {
    Session::id_for(host_id, harness.as_ref(), native_id)
}

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
    pub title_defaults: TitleDefaults,
    pub stream: Option<Stream>,
    pub stream_status: Option<Status>,
    pub session: Option<Session>,
    pub sessions: Vec<Session>,
}

/// The wire form of a context also carries `patch_schemas`, what an agent may change in
/// these records, so a frontend builds its tools and forms without a field policy of its own.
#[derive(Serialize)]
pub struct RecordContextWire<'a> {
    #[serde(flatten)]
    context: &'a RecordContext,
    patch_schemas: PatchSchemas,
}

impl RecordContext {
    pub fn wire(&self) -> RecordContextWire<'_> {
        RecordContextWire {
            context: self,
            patch_schemas: PatchSchemas::new(
                self.stream.as_ref(),
                self.session.as_ref(),
                self.title_defaults,
            ),
        }
    }
}

/// The caller holds the admitted record reader; no discovery, history or native evidence.
pub fn record_context(
    reader: &dyn RecordReader,
    host: &str,
    catalog_source: &str,
    request: ContextRequest<'_>,
    title_defaults: TitleDefaults,
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
        .map(|read| match read {
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
            let Record::Stream(stream) = read else {
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
    let members: Vec<&Session> = session.iter().collect();
    let stream_status = stream
        .as_ref()
        .map(|stream| derived_stream_status(stream, &members, None));
    Ok(RecordContext {
        catalog_source: catalog_source.into(),
        host_id: host.into(),
        title_defaults,
        sessions: session.iter().cloned().collect(),
        stream,
        stream_status,
        session,
    })
}

#[cfg(test)]
mod tests;
