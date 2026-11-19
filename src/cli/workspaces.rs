use super::*;
use args::{SessionSelection, StreamFields};
use streams::{
    model::NamingMode,
    operations::{edits::StreamEdit, patches::Channel},
    runtime::{
        conversations::ConversationRoots,
        workspace::{SelectedConversation, WorkspaceRuntime},
    },
    storage::records::RecordStore,
};

pub(super) enum Creation {
    Record,
    Workspace(Option<FrontendRequest>),
}

fn no_frontend() -> CliError {
    CliError::message(
        "no native frontend selected; pass --frontend herdr, --frontend cmux or --frontend hc (or use resume)",
    )
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
    Ok(opened
        .records()
        .with_reader(|reader| -> application::Result<Stream> {
            Ok(reader
                .read_stream(opened.host().as_str(), id)?
                .ok_or_else(|| ValidationError::new("addressed Stream is missing"))?)
        })?)
}

fn member_sessions<'a>(stream: &Stream, inventory: &'a [Record]) -> Vec<&'a Session> {
    inventory
        .iter()
        .filter_map(|read| match read {
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
    inventory: &'a [Record],
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
        .filter(|session| session.status.is_open_eligible())
        .collect())
}

fn selected_sessions(
    opened: &application::OpenedRecords,
    stream: &Stream,
    selection: &SessionSelection,
    inventory: Option<&[Record]>,
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
        let session = opened
            .records()
            .with_reader(|reader| -> application::Result<Session> {
                let session = reader
                    .read_session(host, id)?
                    .ok_or_else(|| ValidationError::new("addressed Session is missing"))?;
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
        inventory: &[Record],
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

    /// The Stream the caller's workspace names, for metadata-only `--current`.
    fn workspace_target(
        &self,
        opened: &application::OpenedRecords,
        frontend: Option<&mut (dyn Frontend + 'static)>,
    ) -> Result<Stream> {
        let frontend = frontend.ok_or_else(|| {
            CliError::message(
                "metadata-only --current requires a frontend; supply an explicit Stream key",
            )
        })?;
        let id = frontend.workspace_stream()?.ok_or_else(|| {
            CliError::message("calling workspace has no Stream marker; supply a reference")
        })?;
        let stream = exact_stream(opened, &id)?;
        require_local(opened, &stream)?;
        Ok(stream)
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
        let saved = application::create_record(opened.records(), &Record::Stream(stream))?;
        let Record::Stream(stream) = saved.current.clone() else {
            unreachable!("Stream creation target")
        };
        let mut output = catalog::finish_created(saved);
        if !output.accepted_save {
            return Ok(output);
        }
        let completion = (|| -> Result<Option<(Value, bool)>> {
            let directory = self.path(cwd.as_deref().unwrap_or(&self.cwd))?;
            let locations = opened.locations()?;
            locations.remember(
                streams::runtime::BindingKey::Stream(&stream.id),
                &directory,
                false,
                None,
                None,
            )?;
            let Creation::Workspace(requested) = creation else {
                return Ok(None);
            };
            let mut frontend = self
                .frontend(&opened, requested, 30.0)?
                .ok_or_else(no_frontend)?;
            let roots = ConversationRoots::from_environment()?;
            let runtime = WorkspaceRuntime::new(
                opened.host().as_str(),
                &locations,
                opened.source_id(),
                &roots,
                &self.environment,
            );
            let result = frontend.open(&runtime, &stream, &[], Some(&directory))?;
            let ok = result.needs_attention().is_empty();
            Ok(Some((json!(result), ok)))
        })();
        match completion {
            Ok(Some((result, ok))) => {
                output.code = if ok { 0 } else { 2 };
                output.value["runtime"] = result;
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
        let mut stream = Stream::new(uuid::Uuid::new_v4().to_string(), title, None, host)?;
        stream.title_mode = TitleMode::Manual;
        let patch = fields::stream(&stream, intent, &mut |path| self.read_text(path))?;
        let edit = StreamEdit::new(
            &stream,
            patch,
            Actor::User,
            Channel::Cli,
            None,
            NamingMode::default(),
        )?;
        Ok(edit.after().clone())
    }

    pub(super) fn open_workspace(
        &self,
        target: Target,
        session: SessionSelection,
        cwd: Option<PathBuf>,
    ) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let inventory = (!target.current).then(|| opened.inventory()).transpose()?;
        let explicit = self.explicit_workspace_target(
            &opened,
            &target,
            inventory.as_deref().unwrap_or_default(),
        )?;
        if let Some(stream) = &explicit {
            require_local(&opened, stream)?;
        }
        let mut frontend = self.frontend(&opened, target.frontend, 30.0)?;
        let cwd = cwd.as_deref().map(|path| self.path(path)).transpose()?;
        let stream = match explicit {
            Some(stream) => stream,
            None => self.workspace_target(&opened, frontend.as_deref_mut())?,
        };
        let sessions = selected_sessions(&opened, &stream, &session, inventory.as_deref())?;
        let mut frontend = frontend.ok_or_else(no_frontend)?;
        let locations = opened.locations()?;
        let roots = ConversationRoots::from_environment()?;
        let runtime = WorkspaceRuntime::new(
            opened.host().as_str(),
            &locations,
            opened.source_id(),
            &roots,
            &self.environment,
        );
        let result = frontend.open(&runtime, &stream, &sessions, cwd.as_deref())?;
        let mut output = Output::value(json!({"stream_id":stream.id,"runtime":result}));
        output.code = if result.needs_attention().is_empty() {
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
        let id = streams::frontend::resolve_resume_frontend(
            opened.settings().frontend.clone(),
            target.frontend,
            &self.environment,
        )?;
        let mut frontend = id
            .map(|id| self.connect_frontend(id, &opened, 30.0))
            .transpose()?;
        let stream = match explicit {
            Some(stream) => stream,
            None => self.workspace_target(&opened, frontend.as_deref_mut())?,
        };
        let sessions = selected_sessions(&opened, &stream, &session, inventory.as_deref())?;
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
        let locations = opened.locations()?;
        let locator = locations.transcript_file(&session.id)?;
        let selected = SelectedConversation {
            session,
            locator: locator.as_deref(),
        };
        let roots = ConversationRoots::from_environment()?;
        let cwd = cwd.as_deref().map(|path| self.path(path)).transpose()?;
        let runtime = WorkspaceRuntime::new(
            opened.host().as_str(),
            &locations,
            opened.source_id(),
            &roots,
            &self.environment,
        );
        let command = runtime.resume(&stream, selected, cwd.as_deref(), |selected| {
            frontend
                .as_mut()
                .map_or(Ok(false), |frontend| frontend.running(selected))
        })?;
        match command.exec() {
            Ok(never) => match never {},
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests;
