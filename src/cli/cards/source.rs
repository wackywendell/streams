use std::{path::PathBuf, sync::Arc};

use serde::Deserialize;
use streams_temper::runtime::RuntimeError;
use streams_temper::runtime::conversations::Harness;
use streams_temper::runtime::native_cards::{NativeCardSource, ResolvedEditor, SourceLaunch};
use streams_temper::tui::{
    CardRecords, CardSnapshot, CheckoutBinding, CheckoutSelection, DocumentLocation,
    DocumentProblem, DocumentTarget, NativeOwnerObservation, ResolvedDocument, Scope,
};

use super::super::{App, CliError, Result, application, documents};

#[cfg(test)]
mod tests;

pub(super) struct ApplicationSource {
    app: App,
    records: Arc<application::OpenedRecords>,
    native_session_id: String,
    expected_scope: Option<Scope>,
    context: Option<streams_temper::agent::RecordContext>,
}
impl ApplicationSource {
    pub(super) fn new(
        app: App,
        records: Arc<application::OpenedRecords>,
        native_session_id: String,
        scope: Option<Scope>,
        context: Option<streams_temper::agent::RecordContext>,
    ) -> Self {
        Self {
            app,
            records,
            native_session_id,
            expected_scope: scope,
            context,
        }
    }
    fn records(
        &mut self,
        owner: &streams_temper::frontend::herdr::HerdrInventoryPane,
    ) -> Result<CardRecords> {
        let context = match self.context.take() {
            Some(context) => context,
            None => {
                let settings = self.app.config.load()?;
                if settings.catalog_root != self.records.root() {
                    return Err(CliError::message(
                        "catalog root setting changed during card read",
                    ));
                }
                self.app.context_from_records(
                    &self.records,
                    streams_temper::agent::ContextRequest {
                        harness: Harness::Pi,
                        native_id: &self.native_session_id,
                        stream_id: None,
                        environment_stream_id: None,
                        workspace_stream_id: None,
                    },
                    settings.naming,
                )?
            }
        };
        let (stream, session) = context.stream.zip(context.session).ok_or_else(|| {
            CliError::message("native card requires an enrolled exact Pi Session")
        })?;
        let owner = NativeOwnerObservation {
            pane_id: owner.pane.id.clone(),
            tab_id: owner.pane.tab_id.clone(),
            workspace_id: owner.pane.workspace_id.clone(),
        };
        let records = CardRecords::new(stream, session, context.catalog_source, owner)
            .map_err(|error| CliError::message(error.to_string()))?;
        if self
            .expected_scope
            .as_ref()
            .is_some_and(|expected| !expected.same_conversation(records.scope()))
        {
            return Err(CliError::message(
                "captured viewing scope changed; recreate the card",
            ));
        }
        Ok(records)
    }
    fn observe(
        &mut self,
        owner: &streams_temper::frontend::herdr::HerdrInventoryPane,
    ) -> Result<CardSnapshot> {
        let records = self.records(owner)?;
        let documents: Documents = serde_json::from_value(documents::context(
            &self.app.registry(self.records.host().as_str())?,
            &self
                .app
                .locations(self.records.root(), self.records.host().as_str())?,
            records.stream(),
            records.session(),
        ))?;
        let checkouts = documents
            .checkouts
            .into_iter()
            .map(|checkout| CheckoutBinding {
                repository: checkout.repository,
                selection: CheckoutSelection {
                    checkout_id: checkout.checkout_id,
                    root: checkout.root,
                },
            })
            .collect();
        let locations = documents
            .document_locations
            .into_iter()
            .map(|location| {
                let selection = location
                    .checkout_id
                    .zip(location.root)
                    .map(|(checkout_id, root)| CheckoutSelection { checkout_id, root });
                let value = match (
                    location.kind.as_deref(),
                    selection,
                    location.file,
                    location.error,
                ) {
                    (Some("local"), Some(selection), Some(file), None) => {
                        ResolvedDocument::File { selection, file }
                    }
                    (None, selection, None, Some(error)) => ResolvedDocument::Unavailable {
                        selection,
                        problem: DocumentProblem::Other(
                            error
                                .chars()
                                .map(|c| if c.is_control() { ' ' } else { c })
                                .collect(),
                        ),
                    },
                    _ => return Err(CliError::message("invalid application document location")),
                };
                Ok(DocumentLocation {
                    repository: location.repository,
                    path: location.path,
                    location: value,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        CardSnapshot::from_records(records, checkouts, locations)
            .map_err(|e| CliError::message(e.to_string()))
    }
    fn resolve(&self, target: &DocumentTarget) -> Result<ResolvedEditor> {
        let ResolvedDocument::File {
            selection,
            file: expected,
        } = &target.location
        else {
            return Err(CliError::message(
                "document is not available in the selected checkout",
            ));
        };
        let checkout = self
            .app
            .registry(self.records.host().as_str())?
            .get(&selection.checkout_id)?;
        if checkout.directory != selection.root
            || !checkout.repository_urls.contains(&target.repository_url)
            || std::fs::canonicalize(&selection.root)? != selection.root
        {
            return Err(CliError::message(
                "selected checkout root or repository binding changed",
            ));
        }
        let file = checkout.document(&target.document.path)?;
        if &file != expected
            || !std::fs::metadata(&file)?.is_file()
            || std::fs::canonicalize(&file)? != file
            || std::fs::canonicalize(&selection.root)? != selection.root
        {
            return Err(CliError::message(
                "app-resolved document or checkout changed during resolution",
            ));
        }
        let identity = target
            .editor_identity()
            .map_err(|e| CliError::message(e.to_string()))?;
        Ok(ResolvedEditor {
            root: selection.root.clone(),
            identity,
        })
    }
}
impl NativeCardSource for ApplicationSource {
    fn read_records(
        &mut self,
        owner: &streams_temper::frontend::herdr::HerdrInventoryPane,
    ) -> std::result::Result<CardRecords, RuntimeError> {
        self.records(owner)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))
    }
    fn read(
        &mut self,
        owner: &streams_temper::frontend::herdr::HerdrInventoryPane,
    ) -> std::result::Result<CardSnapshot, RuntimeError> {
        self.observe(owner)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))
    }
    fn resolve_document(
        &mut self,
        target: &DocumentTarget,
    ) -> std::result::Result<ResolvedEditor, RuntimeError> {
        self.resolve(target)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))
    }
    fn launch_context(&self) -> SourceLaunch {
        SourceLaunch {
            environment: self.app.environment.clone(),
            cwd: self.app.cwd.clone(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Documents {
    checkouts: Vec<Checkout>,
    document_locations: Vec<Location>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkout {
    repository: String,
    checkout_id: String,
    root: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    repository: String,
    path: String,
    checkout_id: Option<String>,
    root: Option<PathBuf>,
    kind: Option<String>,
    file: Option<PathBuf>,
    error: Option<String>,
}
