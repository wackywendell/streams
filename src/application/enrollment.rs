//! Enrollment persists the effort before the conversation, without a transaction or retry loop.
use super::Result;
use crate::{
    agent::{EnrollmentRequest, effort_id_for},
    model::{Record, RecordKind, Stream, ValidationError},
    operations::DomainOutcome,
    storage::records::{RecordAddress, RecordChange, RecordSave, RecordStore},
};

/// The effort ID derives from the conversation, so an existing effort with the requested
/// content is this enrollment retried; any other existing effort refuses.
pub fn enroll_stream(
    store: &impl RecordStore,
    host: &str,
    request: &EnrollmentRequest,
) -> Result<RecordSave<DomainOutcome>> {
    if request.stream_id().is_some() {
        return Err(
            ValidationError::new("existing Stream enrollment does not create an effort").into(),
        );
    }
    let id = effort_id_for(host, request.harness(), request.harness_session_id());
    let stream = request.stream(host, &id)?;
    let address = RecordAddress::new(host, RecordKind::Stream, &id)?;
    let requested = Record::Stream(stream);
    store.save(&address, |reader, before| {
        if let Some(before) = before {
            if !before.record.same_content(&requested) {
                return Err(ValidationError::new(
                    "effort already exists; attach to it or create work in another conversation",
                )
                .into());
            }
            return Ok(RecordChange {
                record: before.record.clone(),
                outcome: DomainOutcome::NoChange,
            });
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
            record: requested.clone(),
            outcome: DomainOutcome::Changes,
        })
    })
}

/// Attach exactly this native conversation; existing membership is never silently replaced.
pub fn enroll_session(
    store: &impl RecordStore,
    host: &str,
    request: &EnrollmentRequest,
) -> Result<RecordSave<DomainOutcome>> {
    Ok(enroll_session_observed(store, host, request)?.save)
}

/// The Stream read while admitting the Session.
pub struct EnrollmentSave {
    pub save: RecordSave<DomainOutcome>,
    pub stream: Stream,
}

pub fn enroll_session_observed(
    store: &impl RecordStore,
    host: &str,
    request: &EnrollmentRequest,
) -> Result<EnrollmentSave> {
    let selected = request
        .stream_id()
        .map(str::to_owned)
        .unwrap_or_else(|| effort_id_for(host, request.harness(), request.harness_session_id()));
    let proposed = request.session(host, &selected)?;
    let address = RecordAddress::for_record(&Record::Session(proposed.clone()));
    let mut observed_stream = None;
    let saved = store.save::<_, super::ApplicationError>(&address, |reader, before| {
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
        observed_stream = Some(stream.clone());
        Ok(RecordChange {
            record: Record::Session(session),
            outcome,
        })
    })?;
    Ok(EnrollmentSave {
        save: saved,
        stream: observed_stream.expect("a successful enrollment read its Stream"),
    })
}

#[cfg(test)]
mod tests;
