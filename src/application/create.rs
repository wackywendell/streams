//! One-record creation with its named membership dependency, never a catalog scan.
use super::Result;
use crate::{
    model::{Record, RecordKind, ValidationError},
    operations::DomainOutcome,
    storage::records::{RecordAddress, RecordChange, RecordSave, RecordStore},
};

/// A retried creation finds the same record and changes nothing.
pub fn create_record(
    store: &impl RecordStore,
    record: &Record,
) -> Result<RecordSave<DomainOutcome>> {
    store.save(
        &RecordAddress::for_record(record),
        |reader, before| -> Result<RecordChange<DomainOutcome>> {
            if let Some(before) = before {
                if !before.same_content(record) {
                    return Err(ValidationError::new(
                        "record already exists; inspect or update it instead",
                    )
                    .into());
                }
                return Ok(RecordChange {
                    record: before.clone(),
                    outcome: DomainOutcome::NoChange,
                });
            }
            if let Record::Session(session) = record {
                let dependency = reader
                    .read(&RecordAddress::new(
                        &session.host_id,
                        RecordKind::Stream,
                        &session.stream_id,
                    )?)?
                    .ok_or_else(|| ValidationError::new("Session's same-host Stream is missing"))?;
                let Record::Stream(stream) = dependency else {
                    return Err(ValidationError::new("expected Session's Stream").into());
                };
                if stream.owner_host_id != session.host_id || stream.id != session.stream_id {
                    return Err(ValidationError::new(
                        "Session and Stream must share their fixed writing host",
                    )
                    .into());
                }
            }
            Ok(RecordChange {
                record: record.clone(),
                outcome: DomainOutcome::Changes,
            })
        },
    )
}
