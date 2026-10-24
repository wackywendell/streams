//! Enrollment persists the effort before the conversation, without a transaction or retry loop.
use super::Result;
use crate::{
    agent::{EnrollmentRequest, effort_id_for},
    model::{Record, RecordKind, ValidationError},
    operations::DomainOutcome,
    storage::records::{RecordAddress, RecordChange, RecordRequest, RecordSave, RecordStore},
};
use serde_json::json;

/// A retained receipt proves this invocation created the effort; an unrelated existing key refuses.
pub fn enroll_stream(
    store: &impl RecordStore,
    host: &str,
    request: &EnrollmentRequest,
) -> Result<RecordSave> {
    if request.stream_id().is_some() {
        return Err(
            ValidationError::new("existing Stream enrollment does not create an effort").into(),
        );
    }
    let id = effort_id_for(host, request.harness(), request.harness_session_id());
    let stream = request.stream(host, &id)?;
    let operation = format!("{}:stream", request.operation_id());
    let save = RecordRequest::new(
        RecordAddress::new(host, RecordKind::Stream, &id)?,
        &operation,
        &json!({"enrollment":request}),
    )?;
    store.save(&save, |reader, before| {
        if before.is_some() {
            return Err(ValidationError::new(
                "effort already exists; attach to it or create work in another conversation",
            )
            .into());
        }
        let session = request.session(host, &id)?;
        if reader
            .read(&RecordAddress::for_record(&Record::Session(session)))?
            .is_some()
        {
            return Err(ValidationError::new(
                "conversation is already enrolled; creating a new effort does not move it",
            )
            .into());
        }
        Ok(RecordChange {
            record: Record::Stream(stream.clone()),
            outcome: json!({"operation_id":operation,"outcome":DomainOutcome::Changes})
                .as_object()
                .unwrap()
                .clone(),
        })
    })
}

/// Attach exactly this native conversation; existing membership is never silently replaced.
pub fn enroll_session(
    store: &impl RecordStore,
    host: &str,
    request: &EnrollmentRequest,
) -> Result<RecordSave> {
    let selected = request
        .stream_id()
        .map(str::to_owned)
        .unwrap_or_else(|| effort_id_for(host, request.harness(), request.harness_session_id()));
    let proposed = request.session(host, &selected)?;
    let operation = format!("{}:session", request.operation_id());
    let save = RecordRequest::new(
        RecordAddress::for_record(&Record::Session(proposed.clone())),
        &operation,
        &json!({"enrollment":request}),
    )?;
    store.save(&save, |reader, before| {
        let session = match before {
            Some(current) => {
                let Record::Session(current) = &current.record else {
                    return Err(ValidationError::new("enrollment target must be a Session").into());
                };
                if current.host_id != host
                    || current.harness != request.harness().as_ref()
                    || current.harness_session_id != request.harness_session_id()
                {
                    return Err(ValidationError::new(
                        "enrolled native identity differs from request",
                    )
                    .into());
                }
                if request
                    .stream_id()
                    .is_some_and(|id| id != current.stream_id)
                {
                    return Err(ValidationError::new(
                        "conversation already belongs to another Stream; use an explicit move",
                    )
                    .into());
                }
                current.clone()
            }
            None => proposed.clone(),
        };
        let stream = reader
            .read(&RecordAddress::new(
                host,
                RecordKind::Stream,
                &session.stream_id,
            )?)?
            .ok_or_else(|| {
                ValidationError::new(
                    "Stream is missing; create it before enrolling the conversation",
                )
            })?;
        let Record::Stream(stream) = stream.record else {
            return Err(ValidationError::new("enrollment dependency must be a Stream").into());
        };
        if stream.owner_host_id != host {
            return Err(ValidationError::new(
                "conversation and Stream must share their fixed writing host",
            )
            .into());
        }
        let outcome = if before.is_some() {
            DomainOutcome::NoChange
        } else {
            DomainOutcome::Changes
        };
        Ok(RecordChange {
            record: Record::Session(session),
            outcome: json!({"operation_id":operation,"outcome":outcome,"stream":stream})
                .as_object()
                .unwrap()
                .clone(),
        })
    })
}

#[cfg(test)]
mod tests;
