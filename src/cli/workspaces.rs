use super::*;
use args::StreamFields;
use streams_temper::{
    frontend::FrontendId,
    runtime::{conversations::ConversationRoots, workspace::WorkspaceRuntime},
};

pub(super) enum Creation {
    Record,
    Workspace(Option<FrontendRequest>),
}

impl App {
    pub(super) fn create_stream(
        &self,
        title: String,
        fields: StreamFields,
        cwd: Option<PathBuf>,
        creation: Creation,
    ) -> Result<Output> {
        let opened = application::open(&self.config)?;
        let mut stream = Stream::new(
            Uuid::new_v4().to_string(),
            title,
            None,
            opened.host().as_str(),
        )?;
        stream.title_mode = NamingMode::Manual;
        let changes = fields::stream(&stream, &fields, &mut |path| self.read_text(path))?;
        let base = Record::Stream(stream);
        let desired = operations::apply_changes(&base, &changes, Actor::User)?;
        let operation = Operation::new(
            Uuid::new_v4().to_string(),
            None,
            desired,
            Actor::User,
            None,
            vec![],
        )?;
        let saved = save(opened.backend(), &operation)?;
        let event = match &creation {
            Creation::Record => SyncEvent::Stream,
            Creation::Workspace(_) => SyncEvent::Start,
        };
        let extra = if saved.accepted() {
            let completion: Result<Option<Value>> = (|| {
                let directory = self.path(cwd.as_deref().unwrap_or(&self.cwd))?;
                self.locations(&opened)?.remember(
                    operation.target().id(),
                    &directory,
                    None,
                    false,
                    None,
                    None,
                )?;
                if let Creation::Workspace(frontend) = creation {
                    let Record::Stream(stream) = operation.after() else {
                        unreachable!("stream creation operation")
                    };
                    let result =
                        self.runtime_open(&opened, stream, &[], frontend, None, cwd.as_deref())?;
                    Ok(Some(json!({"runtime":result})))
                } else {
                    Ok(None)
                }
            })();
            match completion {
                Ok(extra) => extra,
                Err(error) => Some(
                    json!({"runtime":{"error":error.data()["error"],"detail":"stream saved; retry open with its ID"}}),
                ),
            }
        } else {
            None
        };
        finish(opened.backend(), &operation, saved, event, extra)
    }
    fn runtime_open(
        &self,
        opened: &OpenedCatalog,
        stream: &Stream,
        sessions: &[Session],
        frontend: Option<FrontendRequest>,
        session: Option<&str>,
        cwd: Option<&Path>,
    ) -> Result<runtime::workspace::OpenResult> {
        let selected = self.selection(frontend)?;
        let frontend = self.native_frontend(&selected, 30.0)?;
        let state = self.state(opened)?;
        let locations = self.locations(opened)?;
        let roots = ConversationRoots::from_environment()?;
        let cwd = cwd.map(|p| self.path(p)).transpose()?;
        Ok(WorkspaceRuntime::new(
            &state,
            &locations,
            frontend.as_ref(),
            opened.backend(),
            &roots,
            &self.environment,
        )
        .open(stream, sessions, session, cwd.as_deref())?)
    }
    pub(super) fn open_workspace(
        &self,
        target: Target,
        session: Option<String>,
        cwd: Option<PathBuf>,
    ) -> Result<Output> {
        let opened = application::open(&self.config)?;
        let catalog = opened.backend().snapshot()?.catalog;
        let stream = self.target(&opened, &catalog, &target)?;
        let sessions: Vec<_> = catalog
            .sessions()
            .iter()
            .filter(|s| s.stream_id == stream.id)
            .cloned()
            .collect();
        let result = self.runtime_open(
            &opened,
            stream,
            &sessions,
            target.frontend,
            session.as_deref(),
            cwd.as_deref(),
        )?;
        let publication = opened.backend().sync(SyncEvent::Start);
        let incomplete = !result.needs_attention.is_empty() || publication.needs_attention();
        let mut output = Output::value(
            json!({"stream_id":stream.id,"runtime":result,"publication":result_data(&publication)?}),
        );
        output.code = if incomplete { 2 } else { 0 };
        Ok(output)
    }
    pub(super) fn capture_workspace(&self, target: Target) -> Result<Output> {
        let opened = application::open(&self.config)?;
        let catalog = opened.backend().snapshot()?.catalog;
        let stream = self.target(&opened, &catalog, &target)?;
        let sessions: Vec<_> = catalog
            .sessions()
            .iter()
            .filter(|s| s.stream_id == stream.id)
            .cloned()
            .collect();
        let selected = self.selection(target.frontend)?;
        let frontend = self.native_frontend(&selected, 30.0)?;
        let state = self.state(&opened)?;
        let locations = self.locations(&opened)?;
        let roots = ConversationRoots::from_environment()?;
        let arrangement = WorkspaceRuntime::new(
            &state,
            &locations,
            frontend.as_ref(),
            opened.backend(),
            &roots,
            &self.environment,
        )
        .capture(stream, &sessions)?;
        Ok(Output::value(
            json!({"stream_id":stream.id,"arrangement":arrangement}),
        ))
    }
    pub(super) fn resume_session(
        &self,
        target: Target,
        session: Option<String>,
        cwd: Option<PathBuf>,
    ) -> Result<Output> {
        let opened = application::open(&self.config)?;
        let catalog = opened.backend().snapshot()?.catalog;
        let stream = self.target(&opened, &catalog, &target)?;
        let sessions: Vec<_> = catalog
            .sessions()
            .iter()
            .filter(|s| s.stream_id == stream.id)
            .cloned()
            .collect();
        // Resume is an exec in this terminal, not a native launch-frontend selection.
        // Herdr inventory still protects duplicates even alongside CMUX caller variables.
        let configured = self.config.load()?.frontend;
        let use_herdr = matches!(&configured,FrontendMode::Explicit(id) if *id==FrontendId::herdr())
            || self.environment.get("HERDR_ENV").map(String::as_str) == Some("1");
        let frontend = if use_herdr {
            Some(BuiltinFrontend::connect(
                &FrontendId::herdr(),
                std::sync::Arc::new(streams_temper::process::NativeProcessRunner),
                self.environment.clone(),
                30.0,
            )?)
        } else {
            None
        };
        let state = self.state(&opened)?;
        let locations = self.locations(&opened)?;
        let roots = ConversationRoots::from_environment()?;
        let cwd = cwd.as_deref().map(|p| self.path(p)).transpose()?;
        opened.backend().sync(SyncEvent::Start);
        let runtime = WorkspaceRuntime::new(
            &state,
            &locations,
            frontend.as_ref(),
            opened.backend(),
            &roots,
            &self.environment,
        );
        let permit = runtime.resume(stream, &sessions, session.as_deref(), cwd.as_deref())?;
        match permit.exec() {
            Ok(never) => match never {},
            Err(error) => Err(error.into()),
        }
    }
}
