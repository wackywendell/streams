//! Record-local enrollment and host-local directory selection. Native admission belongs to the driver.

use super::{App, CliError, Output, Result, application};
use serde::Serialize;
use streams::{
    agent::{ContextRequest, EnrollmentRequest, RecordContext},
    model::{Record, Session, Stream},
    operations::DomainOutcome,
    runtime::checkouts::canonical_directory,
    storage::records::RecordSave,
};

#[cfg(test)]
mod tests;

pub(super) struct LocalEnrollment {
    pub(super) output: Output,
    pub(super) session: Option<Session>,
    pub(super) stream: Option<Stream>,
    pub(super) directory_selected: bool,
}

#[derive(Default, Serialize)]
struct Receipt {
    catalog_source: String,
    saves: Vec<Saved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<Stream>,
    #[serde(skip_serializing_if = "Option::is_none")]
    session: Option<Session>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_error: Option<CompletionError>,
    #[serde(skip)]
    accepted: bool,
}
#[derive(Serialize)]
struct Saved {
    target: String,
    save: serde_json::Value,
}
#[derive(Serialize)]
struct CompletionError {
    message: String,
    detail: &'static str,
}
impl Receipt {
    fn persist(&mut self, saved: RecordSave<DomainOutcome>) -> RecordSave<DomainOutcome> {
        self.saves.push(Saved {
            target: saved.target(),
            save: serde_json::json!({"local": saved.local(), "domain": {"outcome": saved.outcome}}),
        });
        self.accepted = true;
        saved
    }
}

impl App {
    pub(super) fn enroll(&self) -> Result<Output> {
        self.enroll_request(streams::codec::load_json(&self.input.read()?)?)
    }

    pub(super) fn enroll_request(&self, request: serde_json::Value) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let request = EnrollmentRequest::parse(request, opened.source_id())?;
        let settings = self.config.load()?;
        if settings.catalog_root != opened.root() {
            return Err(CliError::message(
                "catalog root setting changed during enrollment",
            ));
        }
        let observed = self.context_from_records(
            &opened,
            ContextRequest {
                harness: request.harness(),
                native_id: request.harness_session_id(),
                stream_id: None,
                environment_stream_id: None,
                workspace_stream_id: None,
            },
            settings.naming,
        )?;
        Ok(self.enroll_records(&opened, &request, &observed)?.output)
    }

    pub(super) fn enroll_records(
        &self,
        opened: &application::OpenedRecords,
        request: &EnrollmentRequest,
        observed: &RecordContext,
    ) -> Result<LocalEnrollment> {
        if request.catalog_source() != opened.source_id()
            || observed.catalog_source != opened.source_id()
            || observed.host_id != opened.host().as_str()
        {
            return Err(CliError::message(
                "enrollment context belongs to another catalog producer",
            ));
        }
        let mut receipt = Receipt {
            catalog_source: opened.source_id().into(),
            ..Receipt::default()
        };
        let mut directory_selected = false;
        let completion = (|| -> Result<()> {
            let directory = canonical_directory(&self.path(std::path::Path::new(request.cwd()))?)?;
            if request.stream_proposal().is_some() {
                let created = receipt.persist(application::enroll_stream(
                    opened.records(),
                    opened.host().as_str(),
                    request,
                )?);
                // Reported even if the Session save below fails; that save replaces it.
                if let Record::Stream(stream) = created.current.record {
                    receipt.stream = Some(stream);
                }
            }
            let enrolled = application::enroll_session_observed(
                opened.records(),
                opened.host().as_str(),
                request,
            )?;
            let dependency = enrolled.stream;
            let saved = receipt.persist(enrolled.save);
            let Record::Session(session) = saved.current.record else {
                return Err(CliError::message(
                    "enrollment returned a non-Session conversation",
                ));
            };
            receipt.stream = Some(dependency);
            receipt.session = Some(session);
            let session = receipt.session.as_ref().expect("accepted current Session");
            let stream = receipt.stream.as_ref().expect("accepted current Stream");
            let locations = self.locations(opened.root(), opened.host().as_str())?;
            locations.remember(&stream.id, &directory, None, true, None, None)?;
            let binding =
                locations.remember(&stream.id, &directory, Some(&session.id), true, None, None)?;
            if let Some(checkout) = binding.checkout {
                self.registry(opened.host().as_str())?
                    .bind_inferred_repositories(
                        &checkout.id,
                        &streams::process::NativeProcessRunner,
                    )?;
            }
            if stream.status == streams::model::Status::Closed
                && session.status == streams::model::Status::Active
                && let Some(reopened) =
                    application::lifecycle::reopen_closed_stream(opened, &stream.id)?
                && let Record::Stream(updated) = reopened.current.record
            {
                receipt.stream = Some(updated);
            }
            directory_selected = true;
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
        let value = serde_json::to_value(&receipt)?;
        let output = Output {
            value,
            plain: None,
            notices: vec![],
            code: if receipt.completion_error.is_some() {
                2
            } else {
                0
            },
            accepted_save: receipt.accepted,
        };
        Ok(LocalEnrollment {
            output,
            session: receipt.session,
            stream: receipt.stream,
            directory_selected,
        })
    }
}
