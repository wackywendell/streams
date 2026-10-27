use super::*;
use args::{SessionSelection, StreamFields};
use std::sync::{Arc, atomic::AtomicBool};
use streams::{
    frontend::{FrontendId, herdr::HerdrNativeSnapshot},
    model::{RecordKind, Status},
    runtime::{
        conversations::ConversationRoots,
        native_markers::selected_stream,
        workspace::{SelectedConversation, WorkspaceRuntime},
    },
    storage::records::{RecordAddress, RecordRead, RecordStore},
};

pub(super) enum Creation {
    Record,
    Workspace(Option<FrontendRequest>),
}

struct MetadataSelection {
    stream: Stream,
    snapshot: HerdrNativeSnapshot,
}

fn require_local(opened: &application::OpenedRecords, stream: &Stream) -> Result<()> {
    if stream.owner_host_id != opened.host().as_str() {
        return Err(CliError::message(
            "native workspace action requires the Stream's fixed local writer",
        ));
    }
    Ok(())
}

fn exact_stream(opened: &application::OpenedRecords, id: &str) -> Result<Stream> {
    let address = RecordAddress::new(opened.host().as_str(), RecordKind::Stream, id)
        .map_err(application::ApplicationError::from)?;
    Ok(opened
        .records()
        .with_reader(|reader| -> application::Result<Stream> {
            let read = reader
                .read(&address)?
                .ok_or_else(|| ValidationError::new("addressed Stream is missing"))?;
            let Record::Stream(stream) = read.record else {
                unreachable!("addressed Stream codec")
            };
            Ok(stream)
        })?)
}

fn member_sessions<'a>(stream: &Stream, inventory: &'a [RecordRead]) -> Vec<&'a Session> {
    inventory
        .iter()
        .filter_map(|read| match &read.record {
            Record::Session(session)
                if session.stream_id == stream.id && session.host_id == stream.owner_host_id =>
            {
                Some(session)
            }
            _ => None,
        })
        .collect()
}

fn requested_sessions<'a>(
    stream: &Stream,
    inventory: &'a [RecordRead],
    reference: Option<&str>,
) -> Result<Vec<&'a Session>> {
    let members = member_sessions(stream, inventory);
    if let Some(reference) = reference {
        return Ok(vec![*resolve(&members, reference, |session| {
            (&session.id, &session.title, Some(&session.host_id))
        })?]);
    }
    Ok(members
        .into_iter()
        .filter(|session| session.status == Status::Active)
        .collect())
}

fn selected_sessions(
    opened: &application::OpenedRecords,
    stream: &Stream,
    selection: &SessionSelection,
    inventory: Option<&[RecordRead]>,
) -> Result<Vec<Session>> {
    if let Some(key) = &selection.key {
        let (host, id) = key
            .split_once('/')
            .ok_or_else(|| CliError::InvalidRef("--session-key requires HOST/ID".into()))?;
        if host != stream.owner_host_id {
            return Err(CliError::InvalidRef(
                "Session key must name the selected Stream's local writer".into(),
            ));
        }
        let address = RecordAddress::new(host, RecordKind::Session, id)
            .map_err(application::ApplicationError::from)?;
        let session = opened
            .records()
            .with_reader(|reader| -> application::Result<Session> {
                let read = reader
                    .read(&address)?
                    .ok_or_else(|| ValidationError::new("addressed Session is missing"))?;
                let Record::Session(session) = read.record else {
                    unreachable!("addressed Session codec")
                };
                if session.stream_id != stream.id {
                    return Err(ValidationError::new(
                        "addressed Session is not a member of the selected Stream",
                    )
                    .into());
                }
                Ok(session)
            })?;
        return Ok(vec![session]);
    }
    let discovered;
    let inventory = match inventory {
        Some(inventory) => inventory,
        None => {
            discovered = opened.inventory()?;
            &discovered
        }
    };
    Ok(
        requested_sessions(stream, inventory, selection.reference.as_deref())?
            .into_iter()
            .cloned()
            .collect(),
    )
}

impl App {
    // Positional references always use human discovery, even when they look like UUIDs.
    fn explicit_workspace_target(
        &self,
        opened: &application::OpenedRecords,
        target: &Target,
        inventory: &[RecordRead],
    ) -> Result<Option<Stream>> {
        if target.current {
            if target.reference.is_some() {
                return Err(CliError::message(
                    "--current cannot be combined with a reference",
                ));
            }
            return self
                .environment
                .get("STREAMS_STREAM_ID")
                .map(|id| exact_stream(opened, id))
                .transpose();
        }
        let reference = target
            .reference
            .as_deref()
            .ok_or_else(|| CliError::InvalidRef("supply a reference or --current".into()))?;
        Ok(Some(catalog::select_stream(inventory, reference)?.clone()))
    }

    fn metadata_workspace_target(
        &self,
        opened: &application::OpenedRecords,
        frontend: Option<&BuiltinFrontend>,
    ) -> Result<MetadataSelection> {
        let client = frontend.and_then(BuiltinFrontend::herdr).ok_or_else(|| {
            CliError::message(
                "metadata-only --current requires Herdr; supply an explicit Stream key",
            )
        })?;
        let snapshot = client
            .session_snapshot()
            .and_then(|snapshot| HerdrNativeSnapshot::parse(&snapshot))
            .map_err(runtime::RuntimeError::from)?;
        let caller = client
            .current_pane_from(snapshot.raw())
            .map_err(runtime::RuntimeError::from)?;
        let id = selected_stream(
            &snapshot,
            &caller.workspace_id,
            opened.source_id(),
            opened.host().as_str(),
        )?
        .ok_or_else(|| {
            CliError::message("calling workspace has no Stream marker; supply a reference")
        })?;
        let stream = exact_stream(opened, &id)?;
        require_local(opened, &stream)?;
        Ok(MetadataSelection { stream, snapshot })
    }

    pub(super) fn create_stream(
        &self,
        title: String,
        fields: StreamFields,
        cwd: Option<PathBuf>,
        creation: Creation,
    ) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let stream = self.proposed_stream(opened.host().as_str(), title, &fields)?;
        let operation = Uuid::new_v4().to_string();
        let saved =
            application::create_record(opened.records(), &operation, &Record::Stream(stream));
        let current = saved
            .as_ref()
            .ok()
            .map(|saved| match &saved.current.record {
                Record::Stream(stream) => stream.clone(),
                _ => unreachable!("Stream creation target"),
            });
        let mut output = catalog::finish_save(&operation, saved)?;
        if !output.accepted_save {
            return Ok(output);
        }
        let stream = current.expect("accepted creation returns its actual Stream");
        let completion = (|| -> Result<Option<runtime::workspace::OpenResult>> {
            let directory = self.path(cwd.as_deref().unwrap_or(&self.cwd))?;
            let locations = self.locations(opened.root(), opened.host().as_str())?;
            locations.remember(&stream.id, &directory, None, false, None, None)?;
            let Creation::Workspace(requested) = creation else {
                return Ok(None);
            };
            let frontend = self.native_frontend(&self.selection(requested)?, 30.0)?;
            let roots = ConversationRoots::from_environment()?;
            let cancelled = AtomicBool::new(false);
            Ok(Some(
                WorkspaceRuntime::new(
                    opened.host().as_str(),
                    &locations,
                    frontend.as_ref(),
                    opened.source_id(),
                    &roots,
                    &self.environment,
                )
                .open(&cancelled, &stream, &[], Some(&directory), None)?,
            ))
        })();
        match completion {
            Ok(Some(result)) => {
                output.code = if result.needs_attention.is_empty() {
                    0
                } else {
                    2
                };
                output.value["runtime"] = json!(result);
            }
            Ok(None) => {}
            Err(error) => {
                output.code = 2;
                output.value["completion_error"] = json!({"message":error.to_string(),
                    "detail":"Stream saved; select a valid directory or retry open with its reference"});
            }
        }
        Ok(output)
    }

    fn proposed_stream(&self, host: &str, title: String, intent: &StreamFields) -> Result<Stream> {
        let mut stream = Stream::new(Uuid::new_v4().to_string(), title, None, host)?;
        stream.title_mode = TitleMode::Manual;
        let changes = fields::stream(&stream, intent, &mut |path| self.read_text(path))?;
        let Record::Stream(stream) =
            operations::apply_changes(&Record::Stream(stream), &changes, Actor::User)?
        else {
            unreachable!("Stream fields preserve record kind")
        };
        Ok(stream)
    }

    pub(super) fn open_workspace(
        &self,
        target: Target,
        session: SessionSelection,
        cwd: Option<PathBuf>,
    ) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let settings = self.config.load()?;
        if settings.catalog_root != opened.root() {
            return Err(CliError::message(
                "catalog root setting changed during open",
            ));
        }
        let inventory = (!target.current).then(|| opened.inventory()).transpose()?;
        let explicit = self.explicit_workspace_target(
            &opened,
            &target,
            inventory.as_deref().unwrap_or_default(),
        )?;
        if let Some(stream) = &explicit {
            require_local(&opened, stream)?;
        }
        let selection =
            frontend::resolve_frontend(settings.frontend, target.frontend, &self.environment)?;
        let frontend = self.native_frontend(&selection, 30.0)?;
        let cancelled = AtomicBool::new(false);
        let metadata = explicit
            .is_none()
            .then(|| self.metadata_workspace_target(&opened, frontend.as_ref()))
            .transpose()?;
        let stream = explicit
            .as_ref()
            .unwrap_or_else(|| &metadata.as_ref().expect("metadata target").stream);
        let observed = metadata.as_ref().map(|selected| &selected.snapshot);
        let sessions = selected_sessions(&opened, stream, &session, inventory.as_deref())?;
        let locations = self.locations(opened.root(), opened.host().as_str())?;
        let bindings = sessions
            .iter()
            .map(|session| locations.transcript_file(&stream.id, &session.id))
            .collect::<std::result::Result<Vec<_>, runtime::RuntimeError>>()?;
        let selected = sessions
            .iter()
            .zip(&bindings)
            .map(|(session, binding)| SelectedConversation {
                session,
                locator: binding.as_deref(),
            })
            .collect::<Vec<_>>();
        let roots = ConversationRoots::from_environment()?;
        let cwd = cwd.as_deref().map(|path| self.path(path)).transpose()?;
        let result = WorkspaceRuntime::new(
            opened.host().as_str(),
            &locations,
            frontend.as_ref(),
            opened.source_id(),
            &roots,
            &self.environment,
        )
        .open(&cancelled, stream, &selected, cwd.as_deref(), observed)?;
        let mut output = Output::value(json!({"stream_id":stream.id,"runtime":result}));
        output.code = if result.needs_attention.is_empty() {
            0
        } else {
            2
        };
        Ok(output)
    }

    pub(super) fn resume_session(
        &self,
        target: Target,
        session: SessionSelection,
        cwd: Option<PathBuf>,
    ) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let settings = self.config.load()?;
        if settings.catalog_root != opened.root() {
            return Err(CliError::message(
                "catalog root setting changed during resume",
            ));
        }
        let inventory = (!target.current).then(|| opened.inventory()).transpose()?;
        let explicit = self.explicit_workspace_target(
            &opened,
            &target,
            inventory.as_deref().unwrap_or_default(),
        )?;
        if let Some(stream) = &explicit {
            require_local(&opened, stream)?;
        }
        // Resume executes here; observed reuse is best effort, without discovery.
        let use_herdr = matches!(&settings.frontend, FrontendMode::Explicit(id) if *id == FrontendId::herdr())
            || self.environment.get("HERDR_ENV").map(String::as_str) == Some("1");
        let frontend = if use_herdr {
            Some(BuiltinFrontend::connect(
                &FrontendId::herdr(),
                Arc::new(streams::process::NativeProcessRunner),
                self.environment.clone(),
                30.0,
            )?)
        } else {
            None
        };
        let metadata = explicit
            .is_none()
            .then(|| self.metadata_workspace_target(&opened, frontend.as_ref()))
            .transpose()?;
        let stream = explicit
            .as_ref()
            .unwrap_or_else(|| &metadata.as_ref().expect("metadata target").stream);
        let observed = metadata.as_ref().map(|selected| &selected.snapshot);
        let sessions = selected_sessions(&opened, stream, &session, inventory.as_deref())?;
        let session = match sessions.as_slice() {
            [session] => session,
            [] => {
                return Err(CliError::NotFound(
                    "Stream has no active Session to resume".into(),
                ));
            }
            _ => {
                return Err(CliError::Ambiguous {
                    message: "multiple active Sessions; choose --session".into(),
                    candidates: sessions.iter().map(|session| session.id.clone()).collect(),
                });
            }
        };
        let locations = self.locations(opened.root(), opened.host().as_str())?;
        let locator = locations.transcript_file(&stream.id, &session.id)?;
        let selected = SelectedConversation {
            session,
            locator: locator.as_deref(),
        };
        let roots = ConversationRoots::from_environment()?;
        let cwd = cwd.as_deref().map(|path| self.path(path)).transpose()?;
        let runtime = WorkspaceRuntime::new(
            opened.host().as_str(),
            &locations,
            frontend.as_ref(),
            opened.source_id(),
            &roots,
            &self.environment,
        );
        let command = runtime.resume(stream, selected, cwd.as_deref(), observed)?;
        match command.exec() {
            Ok(never) => match never {},
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests;
