//! Record-local enrollment and host-local directory selection. Native admission belongs to the driver.

use super::{App, Output, Result, application};
use streams::{
    agent::{ContextRequest, EnrollmentRequest, RecordContext},
    model::{Session, Stream},
    runtime::checkouts::canonical_directory,
};

#[cfg(test)]
mod tests;

pub(super) struct LocalEnrollment {
    pub(super) output: Output,
    pub(super) session: Option<Session>,
    pub(super) stream: Option<Stream>,
    pub(super) directory_selected: bool,
}

impl App {
    pub(super) fn enroll(&self) -> Result<Output> {
        self.enroll_request(streams::codec::load_json(&self.input.read()?)?)
    }

    pub(super) fn enroll_request(&self, request: serde_json::Value) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        self.enroll_request_opened(&opened, request)
    }

    pub(super) fn enroll_request_opened(
        &self,
        opened: &application::OpenedRecords,
        request: serde_json::Value,
    ) -> Result<Output> {
        let request = EnrollmentRequest::parse(request, opened.source_id())?;
        let observed = self.context_from_records(
            opened,
            ContextRequest {
                harness: request.harness(),
                native_id: request.harness_session_id(),
                stream_id: None,
                environment_stream_id: None,
                workspace_stream_id: None,
            },
            opened.settings().naming,
        )?;
        Ok(self.enroll_records(opened, &request, &observed)?.output)
    }

    pub(super) fn enroll_records(
        &self,
        opened: &application::OpenedRecords,
        request: &EnrollmentRequest,
        observed: &RecordContext,
    ) -> Result<LocalEnrollment> {
        let directory = canonical_directory(&self.path(std::path::Path::new(request.cwd()))?)?;
        let receipt = application::enroll(opened, request, observed, &directory)?;
        Ok(LocalEnrollment {
            output: Output {
                value: serde_json::to_value(&receipt)?,
                plain: None,
                notices: vec![],
                code: if receipt.completion_error.is_some() {
                    2
                } else {
                    0
                },
                accepted_save: receipt.accepted(),
                wrote: receipt.wrote(),
            },
            session: receipt.session,
            stream: receipt.stream,
            directory_selected: receipt.directory_selected,
        })
    }
}
