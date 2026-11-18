//! Enrollment persists the effort before the conversation, without a transaction or retry loop.
use std::path::Path;

use serde::Serialize;

use super::{OpenedRecords, Result};
use crate::{
    agent::{EnrollmentRequest, RecordContext, effort_id_for},
    config::Config,
    model::{Record, RecordKind, Session, Status, Stream, ValidationError},
    operations::DomainOutcome,
    runtime::CheckoutRegistry,
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
                        "conversation already belongs to another Stream; moving a Session to another Stream is not supported yet",
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

/// What one local enrollment saved, and the error that stopped it after its first save.
#[derive(Default, Serialize)]
pub struct EnrollmentReceipt {
    catalog_source: String,
    saves: Vec<SavedRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<Stream>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<Session>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_error: Option<CompletionError>,
    /// The Session's directory was remembered for its Stream.
    #[serde(skip)]
    pub directory_selected: bool,
}
#[derive(Serialize)]
struct SavedRecord {
    target: String,
    save: serde_json::Value,
}
#[derive(Serialize)]
pub struct CompletionError {
    message: String,
    detail: &'static str,
}
impl EnrollmentReceipt {
    pub fn accepted(&self) -> bool {
        !self.saves.is_empty()
    }
    fn persist(&mut self, saved: RecordSave<DomainOutcome>) -> RecordSave<DomainOutcome> {
        self.saves.push(SavedRecord {
            target: saved.target(),
            save: serde_json::json!({"local": saved.local(), "domain": {"outcome": saved.outcome}}),
        });
        saved
    }
}

/// Enroll a conversation from `directory`: create its proposed Stream, save its Session,
/// remember the directory and reopen a closed Stream that gains an active Session. A
/// failure after the first save is reported in the receipt, never retried.
pub fn enroll(
    config: &Config,
    opened: &OpenedRecords,
    request: &EnrollmentRequest,
    observed: &RecordContext,
    directory: &Path,
) -> Result<EnrollmentReceipt> {
    if request.catalog_source() != opened.source_id()
        || observed.catalog_source != opened.source_id()
        || observed.host_id != opened.host().as_str()
    {
        return Err(
            ValidationError::new("enrollment context belongs to another catalog producer").into(),
        );
    }
    let mut receipt = EnrollmentReceipt {
        catalog_source: opened.source_id().into(),
        ..EnrollmentReceipt::default()
    };
    let completion = (|| -> Result<()> {
        if request.stream_proposal().is_some() {
            let created = receipt.persist(enroll_stream(
                opened.records(),
                opened.host().as_str(),
                request,
            )?);
            // Reported even if the Session save below fails; that save replaces it.
            if let Record::Stream(stream) = created.current.record {
                receipt.stream = Some(stream);
            }
        }
        let enrolled = enroll_session_observed(opened.records(), opened.host().as_str(), request)?;
        let dependency = enrolled.stream;
        let saved = receipt.persist(enrolled.save);
        let Record::Session(session) = saved.current.record else {
            return Err(
                ValidationError::new("enrollment returned a non-Session conversation").into(),
            );
        };
        let reopen = dependency.status == Status::Closed && session.status == Status::Active;
        let (stream_id, session_id) = (dependency.id.clone(), session.id.clone());
        receipt.stream = Some(dependency);
        receipt.session = Some(session);
        remember_directory(config, opened, &stream_id, &session_id, directory)?;
        if reopen
            && let Some(reopened) = super::lifecycle::reopen_closed_stream(opened, &stream_id)?
            && let Record::Stream(updated) = reopened.current.record
        {
            receipt.stream = Some(updated);
        }
        receipt.directory_selected = true;
        Ok(())
    })();
    if let Err(error) = completion {
        if receipt.saves.is_empty() {
            return Err(error);
        }
        receipt.completion_error = Some(CompletionError {
            message: error.to_string(),
            detail: "local enrollment needs attention; inspect the saved records before retrying",
        });
    }
    Ok(receipt)
}

/// Remember that `directory` serves this Stream and Session, and bind the repositories of
/// the checkout it is in.
pub fn remember_directory(
    config: &Config,
    opened: &OpenedRecords,
    stream_id: &str,
    session_id: &str,
    directory: &Path,
) -> Result<()> {
    let host = opened.host().as_str();
    let locations = OpenedRecords::locations(config, opened.root(), host)?;
    locations.remember(stream_id, directory, None, true, None, None)?;
    let binding = locations.remember(stream_id, directory, Some(session_id), true, None, None)?;
    if let Some(checkout) = binding.checkout {
        CheckoutRegistry::new(config.checkout_state_dir(), host)?
            .bind_inferred_repositories(&checkout.id, &crate::process::NativeProcessRunner)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
