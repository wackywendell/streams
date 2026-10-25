//! One-record creation with its named membership dependency, never a catalog scan.
use super::Result;
use crate::{
    codec::record_to_data,
    model::{Record, RecordKind, ValidationError},
    storage::records::{RecordAddress, RecordChange, RecordRequest, RecordSave, RecordStore},
};

pub fn create_record(
    store: &impl RecordStore,
    operation_id: &str,
    record: &Record,
) -> Result<RecordSave> {
    let request = RecordRequest::new(
        RecordAddress::for_record(record),
        operation_id,
        &serde_json::json!({"command":"create","record":record_to_data(record)}),
    )?;
    store.save(&request, |reader, before| -> Result<RecordChange> {
        if before.is_some() {
            return Err(ValidationError::new(
                "record already exists; inspect or update it instead",
            )
            .into());
        }
        if let Record::Session(session) = record {
            let dependency = reader
                .read(&RecordAddress::new(
                    &session.host_id,
                    RecordKind::Stream,
                    &session.stream_id,
                )?)?
                .ok_or_else(|| ValidationError::new("Session's same-host Stream is missing"))?;
            let Record::Stream(stream) = dependency.record else {
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
            outcome: serde_json::json!({
                "operation_id":operation_id,"outcome":"changes",
            })
            .as_object()
            .expect("creation outcome")
            .clone(),
        })
    })
}
