//! A bounded saved-read boundary. Native requests stay unavailable until hc is admitted.
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
    ) -> Result<Output> {
        use std::io::IsTerminal;
        use streams::integration::{Reference, ResolvedTarget, WorkScope};
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
        let ResolvedTarget::Document { path, .. } = resolved else {
            unreachable!("document reference")
        };
        let identity = super::pager::observe_file(&path)?;
        let bytes = super::pager::read_document(&path, identity)?;
        let program = runtime::launch::resolve_executable("less", &self.environment, &self.cwd)
            .map_err(|error| CliError::message(error.to_string()))?;
        super::pager::show(&program, &bytes, "document")?;
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
                    let mut presentation = None;
                    let mut execution = None;
                    if selections.as_slice().iter().any(|selection| {
                        matches!(
                            selection,
                            streams::integration::ViewSelection::Sidebar { .. }
                        )
                    }) && let Ok(socket) =
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
                        if let Ok(native) = operation.presentation(facade.scope(), None) {
                            let mut candidates = std::collections::BTreeMap::new();
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
                            presentation = Some(native);
                        }
                    }
                    let values = match presentation.as_ref() {
                        Some(value) => facade.read_views(
                            selections.as_slice(),
                            streams::integration::Access::Available { value },
                            match execution.as_ref() {
                                Some(value) => streams::integration::Access::Available { value },
                                None => streams::integration::Access::Unavailable {
                                    reason: "foreground context is unavailable".into(),
                                },
                            },
                        ),
                        None => facade.read_saved_views(selections.as_slice()),
                    }
                    .map_err(|_| WireError::unavailable("saved views are unavailable"))?;
                    Ok(ModResult::Read {
                        values,
                        notification_key: facade.scope().notification_key(),
                    })
                }
                ModBody::Act { action } => match action.as_ref() {
                    streams::integration::ModAction::OpenReference {
                        reference: streams::integration::Reference::Link { stream, link },
                        context,
                        ..
                    } => Ok(ModResult::ReferenceOpened {
                        value: facade.open_link(stream, link, context),
                    }),
                    _ => Err(WireError::unavailable("native integration is not enabled")),
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
}
