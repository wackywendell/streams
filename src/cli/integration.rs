//! A bounded owner boundary for saved reads and admitted hc actions.
use super::*;
use streams::integration::{ModBody, ModReply, ModRequest, ModResult, WireError, WireResult};

impl App {
    pub(super) fn integration_bind_space(
        &self,
        space_json: &str,
        stream: &str,
        scope_json: &str,
    ) -> Result<Output> {
        use streams::integration::{Effect, NativeRef, WorkScope};
        use streams::runtime::hc::{HcOperation, SpaceAssociation};
        let scope = streams::codec::load_json(scope_json)?;
        let space = streams::codec::load_json(space_json)?;
        if !scope.is_object() || !space.is_object() {
            return Err(CliError::message(
                "scope and space reference must be named objects",
            ));
        }
        let scope: WorkScope = serde_json::from_value(scope)?;
        let space: NativeRef = serde_json::from_value(space)?;
        space.validate()?;
        if space.adapter != frontend::FrontendId::hc() {
            return Err(CliError::message(
                "bind-space requires an hc space reference",
            ));
        }
        let records = application::open_records(&self.config)?;
        let facade = application::integration::Streams::admit(&records, &scope)?;
        facade.require_stream(stream)?;
        let socket = frontend::hc::HcEndpoint::resolve(&self.environment, &self.cwd)
            .map_err(|error| CliError::message(error.message))?;
        let client = frontend::hc::HcClient::connect(socket, Some(&space.endpoint), 30.0, None)
            .map_err(|error| CliError::message(error.message))?;
        let mut operation = HcOperation::new(client);
        let (before, published) = operation
            .snapshots()
            .map_err(|error| CliError::message(error.message))?;
        let wanted = SpaceAssociation {
            scope,
            stream: stream.into(),
            space: space.clone(),
        };
        let association = operation.finish_associations(&before, &published, Some(&wanted));
        let success = matches!(association, Effect::Applied { .. });
        let mut output = Output::value(json!({"requested_space":space,"association":association}));
        if !success {
            output.code = 2;
        }
        Ok(output)
    }
    pub(super) fn integration_view_document(
        &self,
        scope_json: &str,
        reference_json: &str,
        mode: streams::config::ViewerMode,
    ) -> Result<Output> {
        use std::io::IsTerminal;
        use streams::{
            config::ViewerMode,
            integration::{Reference, ResolvedTarget, WorkScope},
        };
        if mode == ViewerMode::Native {
            return Err(CliError::message(
                "native viewer mode does not run in a terminal",
            ));
        }
        let scope_value = streams::codec::load_json(scope_json)?;
        if !scope_value.is_object() {
            return Err(CliError::message("scope must be a named object"));
        }
        let scope: WorkScope = serde_json::from_value(scope_value)?;
        let reference: Reference =
            serde_json::from_value(streams::codec::load_json(reference_json)?)?;
        if !matches!(reference, Reference::Document { .. }) {
            return Err(CliError::message(
                "document viewer requires a saved Document reference",
            ));
        }
        if !std::io::stdout().is_terminal() {
            return Err(CliError::message("document view requires a terminal"));
        }
        let opened = application::open_records(&self.config)?;
        let facade = application::integration::Streams::admit(&opened, &scope)?;
        let resolved = facade.resolve_saved_reference(
            &reference,
            &self.registry(&scope.host)?,
            &self.locations(opened.root(), &scope.host)?,
        )?;
        let ResolvedTarget::Document {
            checkout_root,
            path,
            ..
        } = resolved
        else {
            unreachable!("document reference")
        };
        let identity = super::pager::observe_file(&path)?;
        let bytes = super::pager::read_document(&path, identity)?;
        match mode {
            ViewerMode::Less => {
                let program =
                    runtime::launch::resolve_executable("less", &self.environment, &self.cwd)
                        .map_err(|error| CliError::message(error.to_string()))?;
                super::pager::show(&program, &bytes, "document")?;
            }
            ViewerMode::Editor => match resolve_editor_command(&self.environment, &checkout_root)
                .map_err(CliError::message)?
            {
                Some((program, flags)) => {
                    let status = std::process::Command::new(program)
                        .args(flags)
                        .arg(&path)
                        .current_dir(&checkout_root)
                        .status()?;
                    if !status.success() {
                        return Err(CliError::message(format!(
                            "editor viewer exited with {status}"
                        )));
                    }
                }
                None => {
                    let program = runtime::launch::resolve_executable(
                        "less",
                        &self.environment,
                        &checkout_root,
                    )
                    .map_err(|error| CliError::message(error.to_string()))?;
                    super::pager::show(&program, &bytes, "document")?;
                }
            },
            ViewerMode::Hunk => {
                let program =
                    runtime::launch::resolve_executable("hunk", &self.environment, &checkout_root)
                        .map_err(|error| CliError::message(error.to_string()))?;
                let status = std::process::Command::new(program)
                    .arg("open")
                    .arg(&path)
                    .current_dir(&checkout_root)
                    .status()?;
                if !status.success() {
                    return Err(CliError::message(format!(
                        "hunk viewer exited with {status}"
                    )));
                }
            }
            ViewerMode::Native => unreachable!("handled above"),
        }
        Ok(Output::plain(Value::Null, String::new()))
    }
    pub(super) fn integration_describe(&self, frontend: FrontendRequest) -> Result<Output> {
        if frontend != FrontendRequest::Explicit(frontend::FrontendId::hc()) {
            return Err(CliError::message(
                "integration describe requires --frontend hc",
            ));
        }
        let records = application::open_records(&self.config)
            .map_err(|_| CliError::message("configured catalog is unavailable"))?;
        let facade = application::integration::Streams::new(&records);
        facade.scope().validate()?;
        let socket = frontend::hc::HcEndpoint::resolve(&self.environment, &self.cwd)
            .map_err(|error| CliError::message(error.message))?;
        let client = frontend::hc::HcClient::connect(socket, None, 30.0, None)
            .map_err(|error| CliError::message(error.message))?;
        let reply = json!({"version":1,"scope":facade.scope(),"endpoint":client.endpoint(),"notification_key":facade.scope().notification_key()});
        Ok(Output::plain(Value::Null, serde_json::to_string(&reply)?))
    }
    pub(super) fn integration_request(&self, text: &str) -> Result<Output> {
        let request = ModRequest::decode(text)?;
        let mut verified_endpoint = None;
        let result = (|| {
            let records = application::open_records(&self.config)
                .map_err(|_| WireError::unavailable("configured catalog is unavailable"))?;
            let facade = application::integration::Streams::admit(&records, &request.scope)
                .map_err(|_| {
                    WireError::unavailable(
                        "configured catalog and host do not match the requested scope",
                    )
                })?;
            match &request.body {
                ModBody::Read { selections } => {
                    use streams::integration::{
                        PresentationSnapshot, ViewSelection, ViewingChoice,
                    };
                    let needs_sidebar = selections
                        .as_slice()
                        .iter()
                        .any(|selection| matches!(selection, ViewSelection::Sidebar { .. }));
                    let needs_frame = selections
                        .as_slice()
                        .iter()
                        .any(|selection| matches!(selection, ViewSelection::Frame { .. }));
                    let mut presentation = None;
                    let mut execution = None;
                    if (needs_sidebar || needs_frame)
                        && let Ok(socket) =
                            frontend::hc::HcEndpoint::resolve(&self.environment, &self.cwd)
                        && let Ok(client) = frontend::hc::HcClient::connect(
                            socket,
                            Some(&request.endpoint),
                            30.0,
                            None,
                        )
                    {
                        verified_endpoint = Some(client.endpoint().clone());
                        let mut operation = streams::runtime::hc::HcOperation::new(client);
                        let mut candidates = std::collections::BTreeMap::new();
                        for selection in selections.as_slice() {
                            if let ViewSelection::Frame { terminal } = selection {
                                candidates.insert(terminal.key.clone(), terminal);
                            }
                        }
                        if needs_sidebar
                            && let Ok(native) = operation.presentation(facade.scope(), None)
                        {
                            for selection in selections.as_slice() {
                                if let ViewSelection::Sidebar {
                                    space,
                                    viewing:
                                        ViewingChoice::Follow {
                                            last_agent: Some(terminal),
                                        },
                                } = selection
                                    && native.stream_for_space(space).is_some()
                                    && native.space_for_terminal(terminal).as_ref() == Some(space)
                                {
                                    candidates.insert(terminal.key.clone(), terminal);
                                }
                            }
                            presentation = Some(native);
                        }
                        if !candidates.is_empty()
                            && let Ok(observer) = frontend::hc::processes::OwnerProcesses::new()
                        {
                            let mut snapshot = streams::runtime::hc::HcForeground::new(
                                operation.endpoint().clone(),
                            );
                            for terminal in candidates.into_values() {
                                let value = operation.foreground(terminal, &observer);
                                snapshot.insert(terminal, value);
                            }
                            execution = Some(snapshot);
                        }
                    }
                    let values = if presentation.is_some() || execution.is_some() {
                        facade.read_views(
                            selections.as_slice(),
                            match presentation.as_ref() {
                                Some(value) => streams::integration::Access::Available { value },
                                None => streams::integration::Access::Unavailable {
                                    reason: "native presentation is unavailable".into(),
                                },
                            },
                            match execution.as_ref() {
                                Some(value) => streams::integration::Access::Available { value },
                                None => streams::integration::Access::Unavailable {
                                    reason: "foreground context is unavailable".into(),
                                },
                            },
                        )
                    } else {
                        facade.read_saved_views(selections.as_slice())
                    }
                    .map_err(|_| WireError::unavailable("saved views are unavailable"))?;
                    let herdr_config = self
                        .config
                        .load()
                        .map(|settings| streams::integration::HerdrConfig {
                            tab_naming: settings.herdr.tab_naming,
                            viewer_mode: settings.herdr.viewer_mode,
                            viewer_placement: settings.herdr.viewer_placement,
                        })
                        .unwrap_or(streams::integration::HerdrConfig {
                            tab_naming: true,
                            viewer_mode: streams::config::ViewerMode::Editor,
                            viewer_placement: streams::config::ViewerPlacement::PaneRight,
                        });
                    Ok(ModResult::Read {
                        values,
                        notification_key: facade.scope().notification_key(),
                        herdr_config,
                    })
                }
                ModBody::Act { action } => match action.as_ref() {
                    streams::integration::ModAction::OpenWork { request: open } => {
                        let value = self.integration_open_work(
                            &records,
                            &facade,
                            &request.endpoint,
                            open,
                            &mut verified_endpoint,
                        )?;
                        Ok(ModResult::WorkOpened {
                            value: Box::new(value),
                        })
                    }
                    streams::integration::ModAction::OpenReference {
                        reference: streams::integration::Reference::Link { stream, link },
                        context,
                        ..
                    } => Ok(ModResult::ReferenceOpened {
                        value: facade.open_link(stream, link, context),
                    }),
                    streams::integration::ModAction::OpenReference { reference, .. } => {
                        let value =
                            self.integration_open_reference(&records, &facade, reference)?;
                        Ok(ModResult::ReferenceOpened { value })
                    }
                },
            }
        })();
        let reply = ModReply {
            version: 1,
            scope: request.scope,
            endpoint: request.endpoint,
            verified_endpoint,
            result: match result {
                Ok(value) => WireResult::Ok(value),
                Err(error) => WireResult::Error(error),
            },
            viewer_projection: vec![],
            warnings: vec![],
        };
        Ok(Output::plain(Value::Null, reply.encode()?))
    }

    fn integration_open_reference(
        &self,
        records: &application::OpenedRecords,
        facade: &application::integration::Streams<'_>,
        reference: &streams::integration::Reference,
    ) -> std::result::Result<streams::integration::ReferenceOpenReport, WireError> {
        use streams::{
            config::{ViewerMode, ViewerPlacement},
            integration::{Reference, ReferenceOpenReport, ReferenceOpenResult, ResolvedTarget},
        };
        let not_applied = |reason: &str| ReferenceOpenReport {
            known: vec![],
            result: ReferenceOpenResult::NotApplied {
                reason: reason.into(),
            },
        };
        let settings = self
            .config
            .load()
            .map_err(|_| WireError::unavailable("configuration is unavailable"))?;
        let mode = settings.herdr.viewer_mode;
        let placement = settings.herdr.viewer_placement;
        let host = &facade.scope().host;
        let registry = self
            .registry(host)
            .map_err(|_| WireError::unavailable("owner checkouts are unavailable"))?;
        let locations = self
            .locations(records.root(), host)
            .map_err(|_| WireError::unavailable("owner locations are unavailable"))?;
        let resolved = match facade.resolve_saved_reference(reference, &registry, &locations) {
            Ok(value) => value,
            Err(_) => return Ok(not_applied("saved reference is unavailable")),
        };
        if placement == ViewerPlacement::Floating && mode != ViewerMode::Native {
            return Ok(not_applied(
                "floating placement requires herdr.viewer.mode = native",
            ));
        }
        if mode == ViewerMode::Native {
            return Ok(match resolved {
                ResolvedTarget::Document { path, .. } => {
                    let Ok(identity) = super::pager::observe_file(&path) else {
                        return Ok(not_applied("document viewer requires a regular file"));
                    };
                    let bytes = match super::pager::read_document(&path, identity) {
                        Ok(bytes) => bytes,
                        Err(err) => return Ok(not_applied(&err.to_string())),
                    };
                    let (content, truncated) = match prepare_native_content(&bytes) {
                        Ok(value) => value,
                        Err(reason) => return Ok(not_applied(reason)),
                    };
                    let Reference::Document { document, .. } = reference else {
                        unreachable!("document reference")
                    };
                    let filename = document.path.clone();
                    ReferenceOpenReport {
                        known: vec![],
                        result: ReferenceOpenResult::ShowContent {
                            title: filename.clone(),
                            filename,
                            content,
                            truncated,
                            placement,
                        },
                    }
                }
                ResolvedTarget::Artifact { key } => {
                    let Ok(output) =
                        application::artifacts::read_artifact(records.artifacts(), &key)
                    else {
                        return Ok(not_applied("saved artifact is unavailable"));
                    };
                    let (content, truncated) =
                        match prepare_native_content(output.content.as_bytes()) {
                            Ok(value) => value,
                            Err(reason) => return Ok(not_applied(reason)),
                        };
                    let filename = key.name.clone();
                    ReferenceOpenReport {
                        known: vec![],
                        result: ReferenceOpenResult::ShowContent {
                            title: filename.clone(),
                            filename,
                            content,
                            truncated,
                            placement,
                        },
                    }
                }
                ResolvedTarget::Link { .. } => unreachable!("link handled before native branch"),
            });
        }

        match resolved {
            ResolvedTarget::Document {
                checkout_root,
                path,
                ..
            } => {
                let Ok(identity) = super::pager::observe_file(&path) else {
                    return Ok(not_applied("document viewer requires a regular file"));
                };
                if let Err(err) = super::pager::read_document(&path, identity) {
                    return Ok(not_applied(&err.to_string()));
                }
                let effective_mode = match mode {
                    ViewerMode::Editor => {
                        match resolve_editor_command(&self.environment, &checkout_root) {
                            Ok(Some(_)) => ViewerMode::Editor,
                            Ok(None) => {
                                if streams::runtime::launch::resolve_executable(
                                    "less",
                                    &self.environment,
                                    &checkout_root,
                                )
                                .is_err()
                                {
                                    return Ok(not_applied("less executable is unavailable"));
                                }
                                ViewerMode::Less
                            }
                            Err(reason) => return Ok(not_applied(&reason)),
                        }
                    }
                    ViewerMode::Hunk => {
                        if streams::runtime::launch::resolve_executable(
                            "hunk",
                            &self.environment,
                            &checkout_root,
                        )
                        .is_err()
                        {
                            return Ok(not_applied("hunk executable is unavailable"));
                        }
                        ViewerMode::Hunk
                    }
                    ViewerMode::Less => {
                        if streams::runtime::launch::resolve_executable(
                            "less",
                            &self.environment,
                            &checkout_root,
                        )
                        .is_err()
                        {
                            return Ok(not_applied("less executable is unavailable"));
                        }
                        ViewerMode::Less
                    }
                    ViewerMode::Native => unreachable!("handled above"),
                };
                let scope_json = serde_json::to_string(facade.scope())
                    .map_err(|_| WireError::unavailable("scope encoding is unavailable"))?;
                let ref_json = serde_json::to_string(reference)
                    .map_err(|_| WireError::unavailable("reference encoding is unavailable"))?;
                let Reference::Document { document, .. } = reference else {
                    unreachable!("document reference")
                };
                Ok(ReferenceOpenReport {
                    known: vec![],
                    result: ReferenceOpenResult::LaunchCommand {
                        title: document.path.clone(),
                        args: vec![
                            "integration".into(),
                            "view-document".into(),
                            "--mode".into(),
                            effective_mode.as_ref().into(),
                            "--scope-json".into(),
                            scope_json,
                            "--reference-json".into(),
                            ref_json,
                        ],
                        placement,
                    },
                })
            }
            ResolvedTarget::Artifact { key } => {
                if application::artifacts::read_artifact(records.artifacts(), &key).is_err() {
                    return Ok(not_applied("saved artifact is unavailable"));
                }
                if streams::runtime::launch::resolve_executable(
                    "less",
                    &self.environment,
                    &self.cwd,
                )
                .is_err()
                {
                    return Ok(not_applied("less executable is unavailable"));
                }
                Ok(ReferenceOpenReport {
                    known: vec![],
                    result: ReferenceOpenResult::LaunchCommand {
                        title: key.name.clone(),
                        args: vec!["artifact".into(), "view".into(), key.reference()],
                        placement,
                    },
                })
            }
            ResolvedTarget::Link { .. } => unreachable!("link handled before terminal branch"),
        }
    }

    fn integration_open_work(
        &self,
        records: &application::OpenedRecords,
        facade: &application::integration::Streams<'_>,
        endpoint: &frontend::EndpointId,
        request: &streams::integration::OpenRequest,
        verified_endpoint: &mut Option<frontend::EndpointId>,
    ) -> std::result::Result<streams::integration::OpenReport, WireError> {
        use streams::{
            integration::{Access, ConversationId, SessionOpenResult, complete_open},
            runtime::{
                conversations::ConversationRoots,
                hc::{HcOperation, open::OpenContext},
                workspace::{SelectedConversation, WorkspaceRuntime},
            },
        };
        if request.cwd.is_some() {
            return Err(WireError::unavailable(
                "desktop opening cannot supply an owner directory",
            ));
        }
        let (stream, sessions) = facade
            .select_open(&request.stream, &request.sessions)
            .map_err(|_| WireError::unavailable("selected work is unavailable"))?;
        let locations = self
            .locations(records.root(), &facade.scope().host)
            .map_err(|_| WireError::unavailable("owner locations are unavailable"))?;
        let mut locator_failures = std::collections::BTreeSet::new();
        let locators: Vec<_> = sessions
            .iter()
            .map(
                |session| match locations.transcript_file(&stream.id, &session.id) {
                    Ok(path) => path,
                    Err(_) => {
                        locator_failures.insert(session.id.clone());
                        None
                    }
                },
            )
            .collect();
        let selected: Vec<_> = sessions
            .iter()
            .zip(&locators)
            .map(|(session, locator)| SelectedConversation {
                session,
                locator: locator.as_deref(),
            })
            .collect();
        let conversations: Vec<_> = sessions.iter().map(ConversationId::from).collect();
        let socket = frontend::hc::HcEndpoint::resolve(&self.environment, &self.cwd)
            .map_err(|_| WireError::unavailable("hc connection is unavailable"))?;
        let client = frontend::hc::HcClient::connect(socket.clone(), Some(endpoint), 30.0, None)
            .map_err(|_| WireError::unavailable("hc connection is unavailable"))?;
        *verified_endpoint = Some(client.endpoint().clone());
        let mut operation = HcOperation::new(client);
        let execution = if operation.supports("terminal.list") {
            frontend::hc::processes::OwnerProcesses::new()
                .ok()
                .and_then(|observer| {
                    operation
                        .registered_sources(&observer)
                        .ok()?
                        .for_scope(facade.scope().clone())
                        .ok()
                })
        } else {
            None
        };
        let presentation = if operation.supports("doc.get") {
            operation.presentation(facade.scope(), None).ok()
        } else {
            None
        };
        let roots = ConversationRoots::default();
        let runtime = WorkspaceRuntime::new(
            &facade.scope().host,
            &locations,
            None,
            &facade.scope().catalog,
            &roots,
            &self.environment,
        );
        let (plan, mut outcomes) = runtime
            .prepare_hc_open(
                &std::sync::atomic::AtomicBool::new(false),
                &stream,
                &selected,
                None,
                execution.as_ref().map_or_else(
                    || Access::Unavailable {
                        reason: "hc Pi presence is unavailable".into(),
                    },
                    |value| Access::Available { value },
                ),
                presentation.as_ref().map_or_else(
                    || Access::Unavailable {
                        reason: "hc presentation is unavailable".into(),
                    },
                    |value| Access::Available { value },
                ),
            )
            .map_err(|_| WireError::unavailable("work opening could not be prepared"))?;
        for id in locator_failures {
            if matches!(outcomes.get(&id), Some(SessionOpenResult::Skipped { .. })) {
                outcomes.insert(
                    id,
                    SessionOpenResult::Skipped {
                        reason: "saved Session locator is unavailable".into(),
                    },
                );
            }
        }
        let effects = operation.execute_prepared_open(
            &plan,
            OpenContext {
                scope: facade.scope(),
                socket: &socket,
                environment: &self.environment,
                selected: &conversations,
                execution: execution.as_ref(),
                presentation: presentation.as_ref(),
            },
        );
        Ok(complete_open(plan, effects, outcomes))
    }
}

const MAX_NATIVE_VIEW_BYTES: usize = 400_000;

fn resolve_editor_command(
    environment: &BTreeMap<String, String>,
    cwd: &Path,
) -> std::result::Result<Option<(PathBuf, Vec<String>)>, String> {
    use unicode_general_category::{GeneralCategory, get_general_category};
    let Some(raw) = environment.get("EDITOR") else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().any(|c| {
        matches!(
            get_general_category(c),
            GeneralCategory::Control | GeneralCategory::Format
        )
    }) {
        return Err("configured EDITOR contains unsafe characters".into());
    }
    let mut parts = trimmed.split_ascii_whitespace();
    let program = parts
        .next()
        .ok_or_else(|| "configured EDITOR is empty".to_owned())?;
    let flags: Vec<String> = parts.map(str::to_owned).collect();
    let resolved = streams::runtime::launch::resolve_executable(program, environment, cwd)
        .map_err(|_| "configured EDITOR executable is unavailable".to_owned())?;
    Ok(Some((resolved, flags)))
}

fn prepare_native_content(bytes: &[u8]) -> std::result::Result<(String, bool), &'static str> {
    if bytes.contains(&0) {
        return Err("binary file cannot be opened in native viewer");
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "non-UTF-8 file cannot be opened in native viewer")?;
    if text.len() <= MAX_NATIVE_VIEW_BYTES {
        return Ok((text.to_owned(), false));
    }
    let mut end = MAX_NATIVE_VIEW_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    Ok((format!("{}\n… (file truncated)", &text[..end]), true))
}
