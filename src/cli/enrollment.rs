//! Record-local enrollment and host-local directory selection. Native admission belongs to the driver.

use super::{App, CliError, Output, Result, application};
use serde::Serialize;
use streams::{
    agent::{ContextRequest, EnrollmentRequest, RecordContext},
    model::{Record, Session, Stream},
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
    operation_id: String,
    save: serde_json::Value,
}
#[derive(Serialize)]
struct CompletionError {
    message: String,
    detail: &'static str,
}
impl Receipt {
    fn persist(
        &mut self,
        operation_id: String,
        result: application::Result<RecordSave>,
    ) -> Result<RecordSave> {
        let saved = match result {
            Ok(saved) => saved,
            Err(application::ApplicationError::Storage(error)) if error.is_uncertain() => {
                self.saves.push(Saved {
                    operation_id,
                    save: serde_json::json!({
                        "local":"uncertain","domain":null,"detail":error.to_string(),
                    }),
                });
                return Err(application::ApplicationError::Storage(error).into());
            }
            Err(error) => return Err(error.into()),
        };
        self.saves.push(Saved {
            operation_id,
            save: serde_json::json!({
                "local":if saved.repeated {"unchanged"} else {"persisted"},"domain":saved.outcome,
            }),
        });
        self.accepted = true;
        Ok(saved)
    }
}

impl App {
    pub(super) fn enroll(&self) -> Result<Output> {
        self.enroll_request(&self.input.read()?)
    }

    pub(super) fn enroll_request(&self, source: &str) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let request = EnrollmentRequest::parse(source, opened.source_id())?;
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
                let saved = receipt.persist(
                    format!("{}:stream", request.operation_id()),
                    application::enroll_stream(opened.records(), opened.host().as_str(), request),
                )?;
                let Record::Stream(stream) = saved.current.record else {
                    return Err(CliError::message("enrollment returned a non-Stream effort"));
                };
                receipt.stream = Some(stream);
            }
            let saved = application::enroll_session_observed(
                opened.records(),
                opened.host().as_str(),
                request,
            );
            let (save, dependency) = match saved {
                Ok(saved) => (Ok(saved.save), saved.stream),
                Err(error) => (Err(error), None),
            };
            let saved = receipt.persist(format!("{}:session", request.operation_id()), save)?;
            let Record::Session(session) = saved.current.record else {
                return Err(CliError::message(
                    "enrollment returned a non-Session conversation",
                ));
            };
            let matching = |stream: &Stream| {
                stream.id == session.stream_id && stream.owner_host_id == session.host_id
            };
            receipt.stream = dependency
                .filter(matching)
                .or_else(|| receipt.stream.take().filter(matching))
                .or_else(|| {
                    observed
                        .stream
                        .as_ref()
                        .filter(|stream| matching(stream))
                        .cloned()
                });
            let changed_membership = saved
                .outcome
                .get("stream_id")
                .and_then(serde_json::Value::as_str)
                != Some(session.stream_id.as_str());
            receipt.session = Some(session);
            if changed_membership {
                return Err(CliError::message(
                    "retained enrollment predates the current membership; inspect current records before completing directory or native setup",
                ));
            }
            let session = receipt.session.as_ref().expect("accepted current Session");
            let stream = receipt.stream.as_ref().ok_or_else(||CliError::message(
                "accepted enrollment has no already-observed matching Stream; refresh before completing local or native setup"))?;
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
            directory_selected = true;
            Ok(())
        })();
        if let Err(error) = completion {
            if receipt.saves.is_empty() {
                return Err(error);
            }
            receipt.completion_error = Some(CompletionError {
                message: error.to_string(),
                detail: "local enrollment needs attention; retain these receipts and inspect before retrying",
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
