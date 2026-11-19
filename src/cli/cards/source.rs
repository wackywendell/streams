use std::{path::PathBuf, sync::Arc};

use serde::Deserialize;
use streams::runtime::RuntimeError;
use streams::runtime::conversations::Harness;
use streams::runtime::native_cards::{NativeCardSource, ResolvedEditor, SourceLaunch};
use streams::tui::{
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
    context: Option<streams::agent::RecordContext>,
}
impl ApplicationSource {
    pub(super) fn new(
        app: App,
        records: Arc<application::OpenedRecords>,
        native_session_id: String,
        scope: Option<Scope>,
        context: Option<streams::agent::RecordContext>,
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
        owner: &streams::frontend::herdr::HerdrInventoryPane,
    ) -> Result<CardRecords> {
        let missing = || CliError::message("native card requires an enrolled exact Pi Session");
        let (stream, session, catalog_source) = match self.context.take() {
            Some(context) => {
                let (stream, session) = context.stream.zip(context.session).ok_or_else(missing)?;
                (stream, session, context.catalog_source)
            }
            None => {
                let application = application::integration::Streams::new(&self.records);
                let (stream, session) = application
                    .conversation_records(&streams::integration::ConversationId {
                        harness: Harness::Pi.to_string(),
                        native_id: self.native_session_id.clone(),
                    })?
                    .ok_or_else(missing)?;
                (stream, session, application.scope().catalog.clone())
            }
        };
        let owner = NativeOwnerObservation {
            pane_id: owner.pane.id.clone(),
            tab_id: owner.pane.tab_id.clone(),
            workspace_id: owner.pane.workspace_id.clone(),
        };
        let records = CardRecords::new(stream, session, catalog_source, owner)
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
        owner: &streams::frontend::herdr::HerdrInventoryPane,
    ) -> Result<CardSnapshot> {
        let records = self.records(owner)?;
        self.display(records)
    }
    pub(super) fn display(&self, records: CardRecords) -> Result<CardSnapshot> {
        let documents: Documents = serde_json::from_value(documents::context(
            &self.records.registry()?,
            &self.records.locations()?,
            records.stream(),
            records.session(),
        ))?;
        let checkouts = documents
            .checkouts
            .into_iter()
            .map(|checkout| CheckoutBinding {
                repository_url: checkout.repository_url,
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
                    repository_url: location.repository_url,
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
        let document = target
            .document()
            .ok_or_else(|| CliError::message("document reference required"))?;
        let facade = application::integration::Streams::admit(
            &self.records,
            &streams::integration::WorkScope {
                catalog: target.scope.catalog_source.clone(),
                host: target.scope.host_id.clone(),
            },
        )?;
        let resolved = facade.resolve_saved_reference(
            &streams::integration::Reference::Document {
                stream: target.scope.stream_id.clone(),
                document: streams::integration::DocumentKey {
                    repository: document.repository_url.clone(),
                    path: document.path.clone(),
                },
                viewing_session: target.scope.session_id.clone(),
            },
            &self.records.registry()?,
            &self.records.locations()?,
        )?;
        let streams::integration::ResolvedTarget::Document {
            path: file,
            checkout_id,
            checkout_root,
            ..
        } = resolved
        else {
            unreachable!("document resolution");
        };
        if checkout_id != selection.checkout_id
            || checkout_root != selection.root
            || &file != expected
        {
            return Err(CliError::message(
                "app-resolved document or checkout changed during resolution",
            ));
        }
        Ok(ResolvedEditor {
            root: checkout_root,
            identity: streams::tui::EditorIdentity {
                catalog_source: facade.scope().catalog.clone(),
                host_id: facade.scope().host.clone(),
                checkout_id,
                file,
            },
        })
    }
}
impl NativeCardSource for ApplicationSource {
    fn read_records(
        &mut self,
        owner: &streams::frontend::herdr::HerdrInventoryPane,
    ) -> std::result::Result<CardRecords, RuntimeError> {
        self.records(owner)
            .map_err(|error| RuntimeError::Invalid(error.to_string()))
    }
    fn read(
        &mut self,
        owner: &streams::frontend::herdr::HerdrInventoryPane,
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
    repository_url: String,
    checkout_id: String,
    root: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Location {
    repository_url: String,
    path: String,
    checkout_id: Option<String>,
    root: Option<PathBuf>,
    kind: Option<String>,
    file: Option<PathBuf>,
    error: Option<String>,
}
