use std::{path::PathBuf, sync::Arc};

use streams::{
    application::documents,
    model::{Record, ReferenceKind, ReferenceStatus, Session, Stream, ValidationError},
    runtime::{
        RuntimeError,
        native_cards::{NativeCardSource, ResolvedEditor, SourceLaunch},
    },
    storage::records::{RecordAddress, RecordStore},
    tui::{
        CardRecords, CardSnapshot, DocumentLocation, DocumentTarget, EditorIdentity,
        NativeOwnerObservation, ResolvedDocument, Scope,
    },
};

use super::super::{App, CliError, Result, application};

pub(super) struct ApplicationSource {
    app: App,
    records: Arc<application::OpenedRecords>,
    native_session_id: String,
    expected_scope: Option<Scope>,
    preloaded: Option<(Stream, Session)>,
}

impl ApplicationSource {
    pub(super) fn new(
        app: App,
        records: Arc<application::OpenedRecords>,
        native_session_id: String,
        scope: Option<Scope>,
        preloaded: Option<(Stream, Session)>,
    ) -> Self {
        Self {
            app,
            records,
            native_session_id,
            expected_scope: scope,
            preloaded,
        }
    }

    fn records(
        &mut self,
        owner: &streams::frontend::herdr::HerdrInventoryPane,
    ) -> Result<CardRecords> {
        let missing = || CliError::message("native card requires an enrolled exact Pi Session");
        let (stream, session) = match self.preloaded.take() {
            Some(pair) => pair,
            None => {
                let host = self.records.host().as_str();
                let found =
                    self.records
                        .records()
                        .with_reader(|reader| -> application::Result<_> {
                            let Some(session) =
                                reader.read_session(host, "pi", &self.native_session_id)?
                            else {
                                return Ok(None);
                            };
                            let Some(stream) = reader.read_stream(host, &session.stream_id)? else {
                                return Err(
                                    ValidationError::new("current Stream is missing").into()
                                );
                            };
                            Ok(Some((stream, session)))
                        })?;
                found.ok_or_else(missing)?
            }
        };
        let owner = NativeOwnerObservation {
            pane_id: owner.pane.id.clone(),
            tab_id: owner.pane.tab_id.clone(),
            workspace_id: owner.pane.workspace_id.clone(),
        };
        let records = CardRecords::new(stream, session, self.records.source_id().to_owned(), owner)
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
        let host = self.records.host().as_str();
        let session = records.session();
        let resolved_root = session
            .directory
            .as_ref()
            .and_then(|dir| std::fs::canonicalize(&dir.path).ok());
        let mut locations = Vec::new();
        for (path, entry) in &session.references {
            if entry.status == ReferenceStatus::Dropped || entry.kind != ReferenceKind::Document {
                continue;
            }
            let location = match documents::locate(host, session, path) {
                Ok(documents::DocumentLocation::Path { path: file }) => {
                    let root = resolved_root.clone().unwrap_or_else(|| {
                        PathBuf::from(&session.directory.as_ref().unwrap().path)
                    });
                    ResolvedDocument::File { root, file }
                }
                Ok(documents::DocumentLocation::Unavailable { reason }) => {
                    ResolvedDocument::Unavailable { reason }
                }
                Err(error) => ResolvedDocument::Unavailable {
                    reason: error
                        .to_string()
                        .chars()
                        .map(|c| if c.is_control() { ' ' } else { c })
                        .collect(),
                },
            };
            locations.push(DocumentLocation {
                path: path.clone(),
                location,
            });
        }
        CardSnapshot::from_records(records, locations).map_err(|e| CliError::message(e.to_string()))
    }

    fn resolve(&self, target: &DocumentTarget) -> Result<ResolvedEditor> {
        let ResolvedDocument::File {
            root: expected_root,
            file: expected_file,
        } = &target.location
        else {
            return Err(CliError::message(
                "document is not available in the Session's directory",
            ));
        };
        if self.records.source_id() != target.scope.catalog_source
            || self.records.host().as_str() != target.scope.host_id
        {
            return Err(CliError::message(
                "captured viewing scope changed; recreate the card",
            ));
        }
        let host = self.records.host().as_str();
        let session =
            self.records
                .records()
                .with_reader(|reader| -> application::Result<Session> {
                    let stream_key = RecordAddress::stream(host, &target.scope.stream_id)?;
                    if reader.read(&stream_key)?.is_none() {
                        return Err(ValidationError::new("addressed Stream is missing").into());
                    }
                    let session_key = RecordAddress::session(
                        host,
                        &target.scope.harness,
                        &target.scope.session_id,
                    )?;
                    let Some(Record::Session(session)) = reader.read(&session_key)? else {
                        return Err(ValidationError::new("viewing Session is unavailable").into());
                    };
                    if session.stream_id != target.scope.stream_id {
                        return Err(ValidationError::new(
                            "viewing Session is not a current member of this Stream",
                        )
                        .into());
                    }
                    Ok(session)
                })?;
        let documents::DocumentLocation::Path { path: file } =
            documents::locate(host, &session, target.path())?
        else {
            return Err(CliError::message("document is no longer available"));
        };
        let root = session
            .directory
            .as_ref()
            .and_then(|dir| std::fs::canonicalize(&dir.path).ok())
            .ok_or_else(|| CliError::message("Session directory is unavailable"))?;
        if root != *expected_root || file != *expected_file {
            return Err(CliError::message(
                "app-resolved document or directory changed during resolution",
            ));
        }
        Ok(ResolvedEditor {
            root,
            identity: EditorIdentity {
                catalog_source: target.scope.catalog_source.clone(),
                host_id: target.scope.host_id.clone(),
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
