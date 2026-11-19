use super::{
    App, Config, Options, application, build,
    events::{Change, Events, Native, Routes},
    files::Files,
    source::ApplicationSource,
};
use std::{collections::BTreeSet, path::PathBuf, sync::Arc};
use streams::{
    cancellation::Cancellation,
    file_store::records::{decode_record, relative_record_path},
    frontend::herdr::{HerdrClient, HerdrNativeSnapshot, events::EventPane},
    model::{Record, RecordKind, Session},
    runtime::native_cards::{
        CardPlacement, DisplayLifetime, LaunchedCardContext, NativeCardsConfig, SOURCE_ENV_KEYS,
        publish_card_marker,
    },
    storage::records::{RecordAddress, RecordStore},
    tui::{
        CardRecords, CardSnapshot, Scope, Target,
        application::{PaneEvents, PaneHost, Refresh},
    },
};
#[cfg(test)]
mod tests;
type Result<T> = std::result::Result<T, String>;

struct Inputs {
    root: PathBuf,
    scope: Scope,
    session: RecordAddress,
    session_path: PathBuf,
    registry: PathBuf,
    directories: [PathBuf; 2],
}
impl Inputs {
    fn new(app: &App, records: &application::OpenedRecords, scope: &Scope) -> Result<Self> {
        if scope.catalog_source != records.source_id()
            || scope.host_id != records.host().as_str()
            || scope.harness != "pi"
            || scope.session_id
                != streams::model::Session::id_for(&scope.host_id, "pi", &scope.native_session_id)
        {
            return Err("watched scope does not match its admitted catalog conversation".into());
        }
        let root = std::fs::canonicalize(records.root()).map_err(|e| e.to_string())?;
        let session = RecordAddress::new(&scope.host_id, RecordKind::Session, &scope.session_id)
            .map_err(|e| e.to_string())?;
        let session_path = root.join(relative_record_path(&session).map_err(|e| e.to_string())?);
        let registry = records.registry(&app.config).map_err(|e| e.to_string())?;
        let bindings = records.locations(&app.config).map_err(|e| e.to_string())?;
        let physical =
            |path: PathBuf| streams::config::resolve_path(&path).map_err(|e| e.to_string());
        let directories = [
            physical(bindings.path(&scope.stream_id, Some(&scope.session_id)))?,
            physical(bindings.path(&scope.stream_id, None))?,
        ];
        Ok(Self {
            root,
            scope: scope.clone(),
            session,
            session_path,
            registry: physical(registry.path().to_path_buf())?,
            directories,
        })
    }
    fn paths(&self, stream: &str) -> Result<BTreeSet<PathBuf>> {
        let address = RecordAddress::new(&self.scope.host_id, RecordKind::Stream, stream)
            .map_err(|e| e.to_string())?;
        Ok(self
            .directories
            .iter()
            .cloned()
            .chain([
                self.session_path.clone(),
                self.root
                    .join(relative_record_path(&address).map_err(|e| e.to_string())?),
                self.registry.clone(),
            ])
            .collect())
    }
    fn session(&self, files: &Files) -> Result<Session> {
        let bytes = files
            .bytes(&self.session_path)?
            .ok_or("watched Session is missing")?;
        let Record::Session(session) = decode_record(&self.session, bytes)
            .map_err(|e| e.to_string())?
            .record
        else {
            return Err("card input is not a Session".into());
        };
        if session.host_id != self.scope.host_id
            || session.harness != self.scope.harness
            || session.harness_session_id != self.scope.native_session_id
        {
            return Err("watched Session belongs to another conversation".into());
        }
        Ok(session)
    }
    fn records(
        &self,
        files: &Files,
        session: Session,
        owner: streams::tui::NativeOwnerObservation,
    ) -> Result<CardRecords> {
        let address =
            RecordAddress::new(&self.scope.host_id, RecordKind::Stream, &session.stream_id)
                .map_err(|error| error.to_string())?;
        let path = self
            .root
            .join(relative_record_path(&address).map_err(|error| error.to_string())?);
        let bytes = files.bytes(&path)?.ok_or("watched Stream is missing")?;
        let Record::Stream(stream) = decode_record(&address, bytes)
            .map_err(|error| error.to_string())?
            .record
        else {
            return Err("card input is not a Stream".into());
        };
        CardRecords::new(stream, session, self.scope.catalog_source.clone(), owner)
            .map_err(|error| error.to_string())
    }
}

pub(super) struct Host {
    app: App,
    options: Options,
    config: NativeCardsConfig,
    lifetime: DisplayLifetime,
    cancel: Cancellation,
    events: Events,
    client: Option<HerdrClient>,
    native: Option<Native>,
    files: Option<Files>,
    records: Option<Arc<application::OpenedRecords>>,
    inputs: Option<Inputs>,
    snapshot: Option<HerdrNativeSnapshot>,
    placement: Option<CardPlacement>,
    stream: String,
    last: Option<CardSnapshot>,
}
impl Host {
    pub fn new(
        app: App,
        options: Options,
        config: NativeCardsConfig,
        context: LaunchedCardContext,
        cancel: Cancellation,
        events: PaneEvents,
    ) -> Result<Self> {
        if options
            .session_id
            .as_ref()
            .is_some_and(|id| id != &context.scope.native_session_id)
        {
            return Err("explicit Pi UUID differs from the card's conversation".into());
        }
        let stream = context.scope.stream_id.clone();
        Ok(Self {
            app,
            options,
            config,
            lifetime: DisplayLifetime::new(context),
            cancel,
            events: Events::new(events),
            client: None,
            native: None,
            files: None,
            records: None,
            inputs: None,
            snapshot: None,
            placement: None,
            stream,
            last: None,
        })
    }
    fn reader(&self, placement: Option<&CardPlacement>) -> Result<App> {
        let context = self.lifetime.context();
        let mut environment = self.app.environment.clone();
        for key in SOURCE_ENV_KEYS {
            environment.remove(key);
        }
        environment.extend(context.reader.environment.clone());
        environment.remove("STREAMS_STREAM_ID");
        if let Some(placement) = placement {
            environment.insert("HERDR_ENV".into(), "1".into());
            environment.insert("HERDR_PANE_ID".into(), placement.owner.pane_id.clone());
            environment.insert("HERDR_TAB_ID".into(), placement.owner.tab_id.clone());
            environment.insert(
                "HERDR_WORKSPACE_ID".into(),
                placement.owner.workspace_id.clone(),
            );
        }
        let cwd = context.reader.cwd.clone();
        Ok(App {
            config: Config::new(&environment, &cwd).map_err(|error| error.to_string())?,
            environment,
            cwd,
            input: self.app.input.clone(),
            herdr: None,
        })
    }
    fn prepare(&mut self) -> Result<()> {
        let client = HerdrClient::connect_cancellable(
            self.app.environment.clone(),
            self.config.timeout_seconds(),
            self.cancel.clone(),
        )
        .map_err(|error| error.to_string())?;
        let inherited = |key: &str| self.app.environment.get(key).cloned().unwrap_or_default();
        let routes = Routes {
            owner: EventPane {
                pane_id: String::new(),
                tab_id: String::new(),
                workspace_id: String::new(),
                terminal_id: Some(self.lifetime.context().owner_terminal().into()),
            },
            card: EventPane {
                pane_id: inherited("HERDR_PANE_ID"),
                tab_id: inherited("HERDR_TAB_ID"),
                workspace_id: inherited("HERDR_WORKSPACE_ID"),
                terminal_id: None,
            },
        };
        self.native = Some(Native::subscribe(&client, routes, self.events.clone())?);
        self.client = Some(client);
        let reader = self.reader(None)?;
        let records =
            Arc::new(application::open_records(&reader.config).map_err(|error| error.to_string())?);
        let inputs = Inputs::new(&reader, &records, self.lifetime.context().scope())?;
        self.files = Some(Files::new(
            inputs.paths(&self.stream)?,
            self.events.clone(),
        )?);
        self.inputs = Some(inputs);
        self.records = Some(records);
        Ok(())
    }
    fn routes(&self, placement: &CardPlacement) -> Routes {
        let pane = |location: &streams::tui::NativeOwnerObservation, terminal| EventPane {
            pane_id: location.pane_id.clone(),
            tab_id: location.tab_id.clone(),
            workspace_id: location.workspace_id.clone(),
            terminal_id: terminal,
        };
        Routes {
            owner: pane(
                &placement.owner,
                Some(self.lifetime.context().owner_terminal().into()),
            ),
            card: pane(
                &placement.card,
                self.lifetime.caller_terminal().map(str::to_owned),
            ),
        }
    }
    fn fresh_native(&mut self) -> Result<Option<CardPlacement>> {
        // Capture this subscription lifetime before observing; reconnection cannot revive the pass.
        let client = self
            .native
            .as_ref()
            .unwrap()
            .pass_client(self.client.as_ref().unwrap());
        let snapshot = HerdrNativeSnapshot::parse(
            &client
                .session_snapshot()
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        let placement = self
            .lifetime
            .observe_native(&client, &snapshot)
            .map_err(|error| error.to_string())?;
        if let Some(placement) = &placement {
            publish_card_marker(
                &client,
                &snapshot,
                &placement.card.pane_id,
                self.lifetime.context().scope(),
            )
            .map_err(|error| error.to_string())?;
        }
        self.client = Some(client);
        self.snapshot = Some(snapshot);
        self.placement = placement.clone();
        Ok(placement)
    }
    fn watch_scope(&mut self, scope: &Scope, placement: &CardPlacement) -> Result<()> {
        if &self.inputs.as_ref().unwrap().scope == scope {
            return Ok(());
        }
        let next = Inputs::new(
            &self.reader(Some(placement))?,
            self.records.as_ref().unwrap(),
            scope,
        )?;
        self.files
            .as_mut()
            .unwrap()
            .set_paths(next.paths(&scope.stream_id)?)?;
        self.stream = scope.stream_id.clone();
        self.inputs = Some(next);
        Ok(())
    }
    fn observe(&mut self, refresh: Refresh) -> Result<CardSnapshot> {
        let prepared = self.files.is_none();
        if prepared {
            self.prepare()?;
        }
        let initial = self.snapshot.is_none();
        let mut native_changed = false;
        let mut files = Vec::new();
        for change in self.events.drain() {
            match change {
                Change::Native => native_changed = true,
                Change::NativeRecovered => native_changed = true,
                Change::File(generation, event) => files.push((generation, event)),
            }
        }
        if refresh == Refresh::Full || native_changed {
            self.snapshot = None;
            self.placement = None;
        }
        let changed = self
            .files
            .as_mut()
            .unwrap()
            .consume(files, if prepared { Refresh::Changed } else { refresh })?;
        if changed.recovered {
            self.snapshot = None;
            self.placement = None;
        }
        if !initial
            && refresh != Refresh::Full
            && !native_changed
            && !changed.changed
            && let Some(last) = &self.last
        {
            return Ok(last.clone());
        }
        if (initial || refresh == Refresh::Full || native_changed || changed.recovered)
            && self.fresh_native()?.is_none()
        {
            self.events.closed();
            return Err("Pi exited or changed conversation".into());
        }
        let placement = self.placement.as_ref().unwrap().clone();
        let routes = self.routes(&placement);
        self.native.as_mut().unwrap().activate(routes);
        let inputs = self.inputs.as_ref().unwrap();
        let session = inputs.session(self.files.as_ref().unwrap())?;
        let mut scope = inputs.scope.clone();
        scope.stream_id = session.stream_id.clone();
        self.watch_scope(&scope, &placement)?;
        self.lifetime
            .context()
            .owner
            .locate(self.snapshot.as_ref().unwrap())
            .map_err(|error| error.to_string())?
            .ok_or("Pi exited or changed conversation")?;
        self.records
            .as_ref()
            .unwrap()
            .records()
            .with_reader(|_| -> application::Result<()> { Ok(()) })
            .map_err(|error| error.to_string())?;
        let observed = self.inputs.as_ref().unwrap().records(
            self.files.as_ref().unwrap(),
            session,
            placement.owner.clone(),
        )?;
        let reader = self.reader(Some(&placement))?;
        let source = ApplicationSource::new(
            reader,
            self.records.as_ref().unwrap().clone(),
            self.lifetime.context().scope.native_session_id.clone(),
            None,
            None,
        );
        let frame = source
            .display(observed)
            .map_err(|error| error.to_string())?;
        self.lifetime
            .admit(&frame, &placement)
            .map_err(|error| error.to_string())?;
        Ok(frame)
    }
}
impl PaneHost for Host {
    fn read(&mut self, refresh: Refresh) -> Result<CardSnapshot> {
        let result = self.observe(refresh);
        self.last = result.as_ref().ok().cloned();
        result
    }
    fn activate(&mut self, painted: &CardSnapshot, target: &Target) -> Result<()> {
        let Some(_) = self.fresh_native()? else {
            self.events.closed();
            return Err("Pi exited or changed conversation".into());
        };
        let mut app = self.app.clone();
        app.herdr = Some(super::super::HerdrRead {
            client: self.client.as_ref().unwrap().clone(),
            snapshot: self.snapshot.clone(),
            owner: Some(self.lifetime.context().owner.clone()),
            workspace_stream: None,
        });
        let mut engine = build(
            app,
            self.options.clone(),
            self.config.clone(),
            self.cancel.flag(),
            self.records.clone(),
            None,
        )
        .map_err(|error| error.to_string())?;
        super::activate(&mut engine, &self.app.environment, painted, target)
    }
}
