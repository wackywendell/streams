//! Record-local capture. Membership comes only from the addressed conversation record.

use std::collections::BTreeSet;

use serde::{Serialize, ser::SerializeStruct};

use crate::{
    model::{Record, RecordKind, Session, Stream, TitleDefaults, ValidationError, check_text},
    operations::patches::agent_patch_schemas,
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
#[derive(Debug, Clone, PartialEq)]
pub struct RecordContext {
    pub catalog_source: String,
    pub host_id: String,
    pub title_defaults: TitleDefaults,
    pub stream: Option<Stream>,
    pub session: Option<Session>,
    pub sessions: Vec<Session>,
}

/// The wire form also carries `patch_schemas`, what an agent may change in these records,
/// so a frontend builds its tools and forms without a field policy of its own.
impl Serialize for RecordContext {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut wire = serializer.serialize_struct("RecordContext", 7)?;
        wire.serialize_field("catalog_source", &self.catalog_source)?;
        wire.serialize_field("host_id", &self.host_id)?;
        wire.serialize_field("title_defaults", &self.title_defaults)?;
        wire.serialize_field("stream", &self.stream)?;
        wire.serialize_field("session", &self.session)?;
        wire.serialize_field("sessions", &self.sessions)?;
        wire.serialize_field(
            "patch_schemas",
            &agent_patch_schemas(
                self.stream.as_ref(),
                self.session.as_ref(),
                self.title_defaults,
            ),
        )?;
        wire.end()
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
        title_defaults,
        sessions: session.iter().cloned().collect(),
        stream,
        session,
    })
}

#[cfg(test)]
mod tests;
