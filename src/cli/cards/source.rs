use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;

use serde::Deserialize;
use streams_temper::runtime::RuntimeError;
use streams_temper::runtime::conversations::Harness;
use streams_temper::runtime::native_cards::{
    CardObservation, HandoverInventory, NativeCardSource, ResolvedEditor, SourceLaunch,
};
use streams_temper::tui::{
    CardSnapshot, CheckoutBinding, CheckoutSelection, DisplayContext, DocumentLocation,
    DocumentProblem, DocumentTarget, NativeOwnerObservation, ResolvedDocument, Scope,
};

use super::super::{App, CliError, Result, application};

pub(super) struct ApplicationSource {
    app: App,
    native_session_id: String,
    expected_scope: Option<Scope>,
    expected_owner: Option<NativeOwnerObservation>,
}
impl ApplicationSource {
    pub(super) fn new(
        app: App,
        native_session_id: String,
        scope: Option<Scope>,
        owner: Option<NativeOwnerObservation>,
    ) -> Self {
        Self {
            app,
            native_session_id,
            expected_scope: scope,
            expected_owner: owner,
        }
    }
    fn observe(
        &self,
        client: &streams_temper::frontend::herdr::HerdrClient,
        snapshot: &streams_temper::frontend::herdr::HerdrNativeSnapshot,
        inventory: bool,
    ) -> Result<CardObservation> {
        let connection = streams_temper::runtime::native::BuiltinFrontend::Herdr(
            client.with_context(self.app.environment.clone()),
        );
        let observed = self.app.agent_observation_from_native(
            Harness::Pi,
            &self.native_session_id,
            self.expected_scope
                .as_ref()
                .map(|scope| scope.stream_id.as_str()),
            true,
            inventory,
            Some((&connection, Some(snapshot.raw()))),
        )?;
        let context = observed.context;
        let (stream, session) = context.stream.zip(context.session).ok_or_else(|| {
            CliError::message("native card requires an enrolled exact Pi Session")
        })?;
        let native = observed
            .native
            .ok_or_else(|| CliError::message("native card requires fresh Herdr attestation"))?;
        let owner = NativeOwnerObservation {
            pane_id: native.pane_id().into(),
            tab_id: native.tab_id().into(),
            workspace_id: native.workspace_id().into(),
        };
        if self
            .expected_owner
            .as_ref()
            .is_some_and(|expected| expected != &owner)
        {
            return Err(CliError::message(
                "attestation does not match the captured owner tuple",
            ));
        }
        let documents: Documents = serde_json::from_value(observed.documents)?;
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
        let frame = CardSnapshot::new(
            stream,
            session,
            DisplayContext {
                catalog_source: context.catalog_source.clone(),
                owner,
                checkouts,
                documents: locations,
            },
        )
        .map_err(|e| CliError::message(e.to_string()))?;
        if self
            .expected_scope
            .as_ref()
            .is_some_and(|expected| expected != frame.scope())
        {
            return Err(CliError::message(
                "captured viewing scope changed; recreate the card",
            ));
        }
        let handover = if inventory {
            let native = observed
                .native_inventory
                .ok_or_else(|| CliError::message("native handover inventory missing"))?;
            Some(HandoverInventory {
                scopes: context
                    .sessions
                    .into_iter()
                    .filter(|session| {
                        session.host_id == frame.scope().host_id
                            && session.stream_id == frame.scope().stream_id
                            && session.harness == "pi"
                    })
                    .map(|session| Scope {
                        catalog_source: context.catalog_source.clone(),
                        host_id: session.host_id,
                        stream_id: session.stream_id,
                        session_id: session.id,
                        harness: session.harness,
                        native_session_id: session.harness_session_id,
                    })
                    .collect(),
                native,
            })
        } else {
            None
        };
        Ok(CardObservation {
            card: frame,
            handover,
        })
    }
    fn resolve(
        &self,
        client: &streams_temper::frontend::herdr::HerdrClient,
        snapshot: &streams_temper::frontend::herdr::HerdrNativeSnapshot,
        target: &DocumentTarget,
    ) -> Result<ResolvedEditor> {
        let current = self.observe(client, snapshot, false)?;
        if current
            .card
            .document_target(&target.document.repository, &target.document.path)
            .as_ref()
            != Some(target)
        {
            return Err(CliError::message(
                "painted document or viewing checkout changed",
            ));
        }
        let ResolvedDocument::File {
            selection,
            file: expected,
        } = &target.location
        else {
            return Err(CliError::message(
                "document is not available in the selected checkout",
            ));
        };
        let opened = application::open(&self.app.config)?;
        let checkout = self.app.registry(&opened)?.get(&selection.checkout_id)?;
        if checkout.directory != selection.root
            || !checkout.repository_urls.contains(&target.repository_url)
            || std::fs::canonicalize(&selection.root)? != selection.root
        {
            return Err(CliError::message(
                "selected checkout root or repository binding changed",
            ));
        }
        let before = std::fs::metadata(&selection.root)?;
        if !before.is_dir() {
            return Err(CliError::message("checkout root is not a directory"));
        }
        let file = checkout.document(&target.document.path)?;
        let after = std::fs::metadata(&selection.root)?;
        if &file != expected
            || !std::fs::metadata(&file)?.is_file()
            || std::fs::canonicalize(&file)? != file
            || std::fs::canonicalize(&selection.root)? != selection.root
            || before.dev() != after.dev()
            || before.ino() != after.ino()
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
    fn read(
        &mut self,
        client: &streams_temper::frontend::herdr::HerdrClient,
        snapshot: &streams_temper::frontend::herdr::HerdrNativeSnapshot,
        inventory: bool,
    ) -> std::result::Result<CardObservation, RuntimeError> {
        self.observe(client, snapshot, inventory)
            .map_err(|e| RuntimeError::Invalid(e.to_string()))
    }
    fn resolve_document(
        &mut self,
        client: &streams_temper::frontend::herdr::HerdrClient,
        snapshot: &streams_temper::frontend::herdr::HerdrNativeSnapshot,
        target: &DocumentTarget,
    ) -> std::result::Result<ResolvedEditor, RuntimeError> {
        self.resolve(client, snapshot, target)
            .map_err(|e| RuntimeError::Invalid(e.to_string()))
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
