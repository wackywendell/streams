//! One-record creation with named same-host Stream membership check, never a catalog scan.

use super::{ApplicationError, Result};
use crate::{
    model::{Record, Session, Stream, ValidationError},
    storage::records::{RecordAddress, RecordChange, RecordSave, RecordStore},
};

pub type CreateSave = RecordSave<()>;

/// Create a Stream. Retrying with identical content changes nothing.
pub fn create_stream(store: &impl RecordStore, stream: &Stream) -> Result<CreateSave> {
    stream.validate()?;
    let target = RecordAddress::for_stream(stream);
    store.save(&target, |_reader, before| -> Result<RecordChange<()>> {
        if let Some(before) = before {
            let candidate = Record::Stream(stream.clone());
            if !before.same_content(&candidate) {
                return Err(ApplicationError::invalid(format!(
                    "Stream {} already exists; use `stream edit` to change it",
                    target.full_key()
                )));
            }
            return Ok(RecordChange {
                record: before.clone(),
                outcome: (),
            });
        }
        Ok(RecordChange {
            record: Record::Stream(stream.clone()),
            outcome: (),
        })
    })
}

/// Create a Session for an existing conversation.
///
/// - Fails if the named same-host Stream does not exist.
/// - Re-running for the same Stream changes nothing (`written == false`).
/// - Re-running for a different Stream is refused (`session edit --stream` moves a Session).
pub fn create_session(store: &impl RecordStore, session: &Session) -> Result<CreateSave> {
    session.validate()?;
    let target = RecordAddress::for_session(session);
    store.save(&target, |reader, before| -> Result<RecordChange<()>> {
        if let Some(Record::Session(existing)) = before {
            if existing.stream_id == session.stream_id {
                return Ok(RecordChange {
                    record: Record::Session(existing.clone()),
                    outcome: (),
                });
            }
            return Err(ApplicationError::invalid(format!(
                "Session {} already belongs to Stream {}/{}; use `session edit --stream` to move it",
                target.full_key(),
                existing.host_id,
                existing.stream_id
            )));
        }
        if reader
            .read_stream(&session.host_id, &session.stream_id)?
            .is_none()
        {
            return Err(ApplicationError::not_found(format!(
                "no Stream {}/{}",
                session.host_id, session.stream_id
            )));
        }
        Ok(RecordChange {
            record: Record::Session(session.clone()),
            outcome: (),
        })
    })
}

/// Create any record (used by low-level tests and catalog setup).
pub fn create_record(store: &impl RecordStore, record: &Record) -> Result<CreateSave> {
    match record {
        Record::Stream(stream) => create_stream(store, stream),
        Record::Session(session) => create_session(store, session),
        Record::Host(host) => {
            host.validate()?;
            let target = RecordAddress::host_record(&host.id)?;
            store.save(&target, |_reader, before| -> Result<RecordChange<()>> {
                if let Some(before) = before {
                    if !before.same_content(record) {
                        return Err(ValidationError::new("Host record already exists").into());
                    }
                    return Ok(RecordChange {
                        record: before.clone(),
                        outcome: (),
                    });
                }
                Ok(RecordChange {
                    record: record.clone(),
                    outcome: (),
                })
            })
        }
    }
}
