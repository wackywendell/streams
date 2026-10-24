use super::{
    App, Config, Options, build,
    events::{Bootstrap, Change, Events, Native, Routes},
    files::Files,
    source::ApplicationSource,
};
use std::{collections::BTreeSet, path::PathBuf};
use streams_temper::{
    cancellation::Cancellation,
    frontend::herdr::{HerdrClient, HerdrEndpoint, HerdrNativeSnapshot, events::EventPane},
    model::Record,
    runtime::{
        checkouts::CheckoutRegistry,
        locations::DirectoryBindings,
        native_cards::{
            CardPlacement, DisplayLifetime, LaunchedCardContext, NativeCardSource,
            NativeCardsConfig, SOURCE_ENV_KEYS,
        },
        native_state::{NativeAttempt, NativeState},
    },
    tui::{
        CardSnapshot, Scope, Target,
        application::{PaneEvents, PaneHost, Refresh},
    },
    yaml_store::{YamlStore, encode_component},
};

type Result<T> = std::result::Result<T, String>;

// These are the active context adapter's addresses. Its explicit record-format
// cutover must change this adapter too; cards never probe another storage format.
struct Inputs {
    root: PathBuf,
    scope: Scope,
    sessions: Vec<PathBuf>,
    registry: PathBuf,
    directories: Vec<PathBuf>,
    evidence: PathBuf,
}
impl Inputs {
    fn new(app: &App, scope: &Scope, evidence: PathBuf) -> Result<Self> {
        let root =
            std::fs::canonicalize(app.config.load().map_err(|e| e.to_string())?.catalog_root)
                .map_err(|e| e.to_string())?;
        let session_ids = BTreeSet::from([
            scope.session_id.clone(),
            streams_temper::agent::session_id_for(
                &scope.host_id,
                streams_temper::runtime::conversations::Harness::Pi,
                &scope.native_session_id,
            ),
        ]);
        let registry = CheckoutRegistry::new(app.config.checkout_state_dir(), &scope.host_id)
            .map_err(|e| e.to_string())?;
        let runtime = app
            .config
            .runtime_state_dir_for(&root)
            .map_err(|e| e.to_string())?;
        let bindings = DirectoryBindings::new(runtime, registry.clone());
        let physical =
            |path: PathBuf| streams_temper::config::resolve_path(&path).map_err(|e| e.to_string());
        let mut sessions = Vec::new();
        let mut directories = Vec::new();
        for id in session_ids {
            let name = format!("{}.yaml", encode_component(&id));
            sessions.push(
                root.join("conversations")
                    .join(encode_component(&scope.host_id))
                    .join(&name),
            );
            sessions.push(
                root.join("sessions")
                    .join(encode_component(&scope.stream_id))
                    .join(encode_component(&scope.host_id))
                    .join(name),
            );
            directories.push(physical(bindings.path(&scope.stream_id, Some(&id)))?);
        }
        Ok(Self {
            root,
            scope: scope.clone(),
            sessions,
            registry: physical(registry.path().to_path_buf())?,
            directories,
            evidence: physical(evidence)?,
        })
    }
    fn paths(&self, stream: &str) -> BTreeSet<PathBuf> {
        self.sessions
            .iter()
            .cloned()
            .chain([
                self.root
                    .join("streams")
                    .join(format!("{}.yaml", encode_component(stream))),
                self.registry.clone(),
            ])
            .chain(self.directories.iter().cloned())
            .collect()
    }
    fn membership(&self, files: &Files) -> Result<Option<String>> {
        let scope = &self.scope;
        for path in &self.sessions {
            let Some(bytes) = files.bytes(path)? else {
                continue;
            };
            let relative = path.strip_prefix(&self.root).map_err(|e| e.to_string())?;
            let Record::Session(session) =
                YamlStore::decode_record(&relative.to_string_lossy(), bytes)
                    .map_err(|e| e.to_string())?
            else {
                return Err("card input is not a Session".into());
            };
            if session.host_id != scope.host_id
                || session.harness != scope.harness
                || session.harness_session_id != scope.native_session_id
            {
                return Err("watched Session belongs to another conversation".into());
            }
            return Ok(Some(session.stream_id));
        }
        Ok(None)
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
        cancel: Cancellation,
        events: PaneEvents,
    ) -> Result<Self> {
        let context = LaunchedCardContext::from_environment(&app.environment)
            .map_err(|e| e.to_string())?
            .ok_or("native pane requires a protected launched-card context")?;
        if options
            .session_id
            .as_ref()
            .is_some_and(|id| id != &context.scope().native_session_id)
        {
            return Err("explicit Pi UUID differs from captured viewing scope".into());
        }
        uuid::Uuid::parse_str(&context.scope().native_session_id).map_err(|e| e.to_string())?;
        if options
            .state_dir
            .as_deref()
            .is_some_and(|path| path != context.state_directory())
        {
            return Err("explicit state directory differs from protected card context".into());
        }
        let socket = HerdrEndpoint::resolve(&app.environment)
            .map_err(|e| e.to_string())?
            .socket;
        let stream = context.scope().stream_id.clone();
        Ok(Self {
            lifetime: DisplayLifetime::new(context, socket).map_err(|e| e.to_string())?,
            app,
            options,
            config,
            cancel,
            events: Events::new(events),
            client: None,
            native: None,
            files: None,
            inputs: None,
            snapshot: None,
            placement: None,
            stream,
            last: None,
        })
    }
    fn reader(&self, placement: &CardPlacement) -> Result<App> {
        let context = self.lifetime.context();
        let mut environment = self.app.environment.clone();
        for key in SOURCE_ENV_KEYS {
            environment.remove(key);
        }
        environment.extend(context.reader().environment.clone());
        environment.remove("STREAMS_STREAM_ID");
        environment.insert("HERDR_ENV".into(), "1".into());
        environment.insert("HERDR_PANE_ID".into(), placement.owner.pane_id.clone());
        environment.insert("HERDR_TAB_ID".into(), placement.owner.tab_id.clone());
        environment.insert(
            "HERDR_WORKSPACE_ID".into(),
            placement.owner.workspace_id.clone(),
        );
        let cwd = context.reader().cwd.clone();
        Ok(App {
            config: Config::new(&environment, &cwd).map_err(|e| e.to_string())?,
            environment,
            cwd,
            input: self.app.input.clone(),
            herdr: None,
        })
    }
    fn prepare(&mut self, evidence: &NativeAttempt) -> Result<()> {
        if self.client.is_none() {
            self.client = Some(
                HerdrClient::connect_cancellable(
                    self.app.environment.clone(),
                    self.config.timeout_seconds(),
                    self.cancel.clone(),
                )
                .map_err(|e| e.to_string())?,
            );
        }
        if self.native.is_none() {
            let owner = self.lifetime.context().owner();
            let inherited = |key: &str, fallback: &str| {
                self.app
                    .environment
                    .get(key)
                    .cloned()
                    .unwrap_or_else(|| fallback.into())
            };
            let routes = Routes {
                owner: EventPane {
                    pane_id: owner.pane_id.clone(),
                    tab_id: owner.tab_id.clone(),
                    workspace_id: owner.workspace_id.clone(),
                    terminal_id: Some(self.lifetime.context().owner_terminal().into()),
                },
                card: EventPane {
                    pane_id: inherited("HERDR_PANE_ID", ""),
                    tab_id: inherited("HERDR_TAB_ID", &owner.tab_id),
                    workspace_id: inherited("HERDR_WORKSPACE_ID", &owner.workspace_id),
                    terminal_id: evidence
                        .resources()
                        .map(|resource| resource.terminal_id().to_owned()),
                },
            };
            self.native = Some(Native::subscribe(
                self.client.as_ref().unwrap(),
                routes,
                self.events.clone(),
            )?);
        }
        if self.files.is_none() {
            let context = self.lifetime.context();
            // Configuration is recovered from the protected reader, not the
            // plugin process's working directory or mutable Stream hint.
            let placement = CardPlacement {
                owner: context.owner().clone(),
                card: context.owner().clone(),
            };
            let evidence_path = NativeState::new(context.state_directory())
                .and_then(|state| state.path(evidence.key()))
                .map_err(|e| e.to_string())?;
            let inputs = Inputs::new(&self.reader(&placement)?, context.scope(), evidence_path)?;
            self.files = Some(Files::new(
                inputs.paths(&self.stream),
                inputs.evidence.clone(),
                self.events.clone(),
            )?);
            self.inputs = Some(inputs);
        }
        Ok(())
    }
    fn routes(&self, placement: &CardPlacement) -> Routes {
        let event = |pane: &streams_temper::tui::NativeOwnerObservation, terminal| EventPane {
            pane_id: pane.pane_id.clone(),
            tab_id: pane.tab_id.clone(),
            workspace_id: pane.workspace_id.clone(),
            terminal_id: terminal,
        };
        Routes {
            owner: event(
                &placement.owner,
                Some(self.lifetime.context().owner_terminal().into()),
            ),
            card: event(
                &placement.card,
                self.lifetime.caller_terminal().map(str::to_owned),
            ),
        }
    }
    fn evidence(&self) -> Result<NativeAttempt> {
        self.lifetime
            .read_evidence()
            .inspect_err(|error| self.events.failed(error))
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                self.events.closed();
                "card lifetime ended".into()
            })
    }
    fn fresh_native(&mut self, evidence: &NativeAttempt) -> Result<Option<CardPlacement>> {
        let client = self.client.as_ref().unwrap();
        let snapshot =
            HerdrNativeSnapshot::parse(&client.session_snapshot().map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        let placement = self
            .lifetime
            .observe_native(client, &snapshot, evidence)
            .map_err(|e| e.to_string())?;
        self.snapshot = Some(snapshot);
        self.placement = placement.clone();
        Ok(placement)
    }
    fn watch_scope(&mut self, scope: &Scope, placement: &CardPlacement) -> Result<()> {
        let inputs = self.inputs.as_ref().unwrap();
        if &inputs.scope == scope {
            return Ok(());
        }
        let next = Inputs::new(&self.reader(placement)?, scope, inputs.evidence.clone())?;
        self.files
            .as_mut()
            .unwrap()
            .set_paths(next.paths(&scope.stream_id))
            .inspect_err(|error| self.events.failed(error))?;
        self.stream = scope.stream_id.clone();
        self.inputs = Some(next);
        Ok(())
    }
    fn observe(&mut self, refresh: Refresh) -> Result<CardSnapshot> {
        if self.files.is_none() {
            let evidence = self.evidence()?;
            self.prepare(&evidence).inspect_err(|error| {
                self.events
                    .failed(format!("required card subscriptions: {error}"))
            })?;
        }
        let initial = self.snapshot.is_none();
        let changes = self.events.drain();
        // Registration precedes this recheck. Drain first, so an evidence change
        // cannot be consumed with a proof read before its notification arrived.
        let evidence = self.evidence()?;
        let manual = refresh == Refresh::Full;
        let mut native_changed = false;
        let mut recovery = false;
        let mut files = Vec::new();
        for change in changes {
            match change {
                Change::Native => native_changed = true,
                Change::NativeRecovered => {
                    native_changed = true;
                    recovery = true;
                }
                Change::File(generation, event) => files.push((generation, event)),
            }
        }
        let changed = self
            .files
            .as_mut()
            .unwrap()
            .consume(files)
            .inspect_err(|error| self.events.failed(error))?;
        let full = changed.recovered || recovery;
        let result = (|| {
            if !initial
                && !manual
                && !native_changed
                && !changed.changed
                && let Some(last) = &self.last
            {
                return Ok(last.clone());
            }
            if initial || manual || native_changed || changed.recovered {
                let placement = self.fresh_native(&evidence).inspect_err(|error| {
                    self.events
                        .failed(format!("card native observation failed: {error}"))
                })?;
                let Some(_) = placement else {
                    self.events.closed();
                    return Err("card owner closed".into());
                };
            }
            let placement = self.placement.as_ref().unwrap().clone();
            // Native routing is established independently of catalog availability.
            // Otherwise a failed first context read can hide its repair events.
            if initial {
                let routes = self.routes(&placement);
                self.native
                    .as_mut()
                    .unwrap()
                    .activate(Bootstrap::Initial, routes);
            }
            let inputs = self.inputs.as_ref().unwrap();
            if let Some(stream) = inputs.membership(self.files.as_ref().unwrap())? {
                let mut scope = inputs.scope.clone();
                scope.stream_id = stream;
                self.watch_scope(&scope, &placement)?;
            }
            let mut source = ApplicationSource::new(
                self.reader(&placement)?,
                self.lifetime.context().scope().native_session_id.clone(),
                None,
                Some(placement.owner.clone()),
            );
            let result = source
                .read(
                    self.client.as_ref().unwrap(),
                    self.snapshot.as_ref().unwrap(),
                    false,
                )
                .map_err(|e| e.to_string())
                .and_then(|observation| {
                    self.lifetime
                        .admit(&observation.card, &placement)
                        .map_err(|e| e.to_string())?;
                    Ok(observation.card)
                });
            let frame = result?;
            self.watch_scope(frame.scope(), &placement)?;
            if recovery {
                let routes = self.routes(&placement);
                self.native
                    .as_mut()
                    .unwrap()
                    .activate(Bootstrap::Recovery, routes);
            }
            Ok(frame)
        })();
        if full && let Err(error) = &result {
            self.events
                .failed(format!("required card recovery read failed: {error}"));
        }
        result
    }
}
impl PaneHost for Host {
    fn read(&mut self, refresh: Refresh) -> Result<CardSnapshot> {
        let result = self.observe(refresh);
        self.last = result.as_ref().ok().cloned();
        result
    }
    fn activate(&mut self, painted: &CardSnapshot, target: &Target) -> Result<()> {
        // Display caches do not authorize effects. Build the existing guarded
        // executor from a fresh snapshot and the retained exact caller terminal.
        let evidence = self.evidence()?;
        let Some(placement) = self.fresh_native(&evidence)? else {
            self.events.closed();
            return Err("card owner closed".into());
        };
        let mut app = self.app.clone();
        app.environment
            .insert("HERDR_PANE_ID".into(), placement.card.pane_id.clone());
        app.environment
            .insert("HERDR_TAB_ID".into(), placement.card.tab_id.clone());
        app.environment.insert(
            "HERDR_WORKSPACE_ID".into(),
            placement.card.workspace_id.clone(),
        );
        let scope = painted.scope();
        app.environment.insert("STREAMS_CARDS_SCOPE".into(), serde_json::json!({"catalogSource":scope.catalog_source,
            "hostId":scope.host_id,"streamId":scope.stream_id,"sessionId":scope.session_id,"nativeSessionId":scope.native_session_id}).to_string());
        app.environment.insert(
            "STREAMS_CARDS_OWNER".into(),
            serde_json::json!({"pane_id":placement.owner.pane_id,
            "tab_id":placement.owner.tab_id,"workspace_id":placement.owner.workspace_id})
            .to_string(),
        );
        app.herdr = Some(super::super::HerdrRead {
            client: self.client.as_ref().unwrap().clone(),
            snapshot: self.snapshot.clone(),
            terminal: self.lifetime.caller_terminal().map(str::to_owned),
            refusal: None,
        });
        let mut engine = build(
            app,
            self.options.clone(),
            self.config.clone(),
            self.cancel.flag(),
        )
        .map_err(|e| e.to_string())?;
        super::activate(&mut engine, &self.app.environment, painted, target)
    }
}
