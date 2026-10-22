//! Ordered local enrollment. Receipts and native claims survive incomplete completion.

use super::*;
use serde::Serialize;
use streams_temper::{
    agent::{EnrollmentRequest, effort_id_for},
    operations::Authority,
    runtime::{checkouts::canonical_directory, conversations::Harness, native::verify_pi_caller},
};

#[derive(Default, Serialize)]
struct Receipt {
    catalog_source: String,
    saves: Vec<Saved>,
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<Value>,
    stream: Option<Stream>,
    session: Option<Session>,
    authority: Option<Authority>,
    #[serde(skip_serializing_if = "Option::is_none")]
    completion_error: Option<CompletionError>,
}
#[derive(Serialize)]
struct Saved {
    operation_id: String,
    #[serde(serialize_with = "serialize_save")]
    save: outcomes::Result,
}
#[derive(Serialize)]
struct CompletionError {
    message: String,
    detail: &'static str,
}

fn serialize_save<S: serde::Serializer>(
    saved: &outcomes::Result,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    let mut data = result_data(saved).map_err(serde::ser::Error::custom)?;
    if let Some(domain) = &saved.domain {
        data["domain"]["record"] = match domain {
            operations::Reconciliation::Changes { record, .. } => record_to_data(record),
            _ => Value::Null,
        };
    }
    data.serialize(serializer)
}

impl Receipt {
    fn persist(&mut self, backend: &dyn CatalogBackend, operation: Operation) -> Result<()> {
        let saved = backend.save(
            &operation,
            Some(&Origin::from([("command".into(), "streams-agent".into())])),
        )?;
        let accepted = saved.accepted();
        self.saves.push(Saved {
            operation_id: operation.id().into(),
            save: saved,
        });
        if !accepted {
            return Err(CliError::message(
                "enrollment operation was not accepted; inspect its save receipt",
            ));
        }
        Ok(())
    }
}

impl App {
    pub(super) fn enroll(&self) -> Result<Output> {
        let source = self.input.read()?;
        let opened = application::open(&self.config)?;
        let catalog_source = opened.source_id()?;
        let request = EnrollmentRequest::parse(&source, &catalog_source)?;
        let directory = canonical_directory(&self.path(Path::new(request.cwd()))?)?;
        let state = self.state(&opened)?;
        let connection = self.native_frontend(&self.selection(None)?, 30.0)?;
        let caller = connection
            .as_ref()
            .map(BuiltinFrontend::caller)
            .transpose()?;
        let explicit = self
            .environment
            .get("STREAMS_STREAM_ID")
            .map(String::as_str);
        let verify = || -> Result<()> {
            if request.actor() == Actor::Automatic
                && request.harness() == Harness::Pi
                && let Some(client) = connection.as_ref().and_then(BuiltinFrontend::herdr)
            {
                let proof = verify_pi_caller(client, request.harness_session_id())?;
                if caller
                    .as_ref()
                    .is_none_or(|caller| caller.workspace_id() != proof.workspace_id())
                {
                    return Err(CliError::message(
                        "calling workspace changed during enrollment",
                    ));
                }
            }
            Ok(())
        };
        verify()?;
        let mut receipt = Receipt {
            catalog_source: catalog_source.clone(),
            ..Receipt::default()
        };
        let completion = (|| -> Result<()> {
            let _conversation = state
                .conversation_lock(request.harness().as_ref(), request.harness_session_id())?;
            let mut scope = state.enrollment(
                caller
                    .as_ref()
                    .map(|caller| (caller.scope(), caller.workspace_id())),
                explicit,
            )?;
            // Waiting for either lock must not turn an earlier observation into native authority.
            verify()?;
            let current = agent::context(
                opened.backend(),
                opened.host().as_str(),
                &catalog_source,
                ContextRequest {
                    harness: request.harness(),
                    native_id: request.harness_session_id(),
                    stream_id: request.stream_id(),
                    environment_stream_id: explicit,
                    workspace_stream_id: None,
                },
            )
            .map_err(|error| CliError::message(error.to_string()))?;
            if let (Some(stream), Some(scope_id)) = (&current.stream, scope.stream_id())
                && stream.id != scope_id
            {
                return Err(CliError::message(
                    "calling workspace has a different effort; moving requires an explicit move or fork operation",
                ));
            }
            let selected_id = current
                .stream
                .as_ref()
                .map(|stream| stream.id.clone())
                .or_else(|| scope.stream_id().map(str::to_owned))
                .unwrap_or_else(|| {
                    effort_id_for(
                        opened.host().as_str(),
                        request.harness(),
                        request.harness_session_id(),
                    )
                });
            let mut stream = current.stream;
            if stream.is_none() {
                stream = opened
                    .backend()
                    .snapshot()?
                    .catalog
                    .stream(&selected_id)
                    .cloned();
            }
            let mut session = current.session;
            receipt.stream = stream.clone();
            receipt.session = session.clone();
            receipt.authority = current.authority;
            if request.actor() == Actor::User
                && request.stream_proposal().is_some()
                && let Some(existing) = &stream
            {
                let authority = opened.backend().authority(&existing.id)?;
                if authority.as_ref().is_none_or(|authority| {
                    authority.host_id != opened.host().as_str()
                        || authority.token != format!("{}:stream", request.operation_id())
                }) {
                    return Err(CliError::message(
                        "effort already exists here; attach to it or create work in another workspace",
                    ));
                }
                receipt.persist(
                    opened.backend(),
                    Operation::new(
                        format!("{}:stream", request.operation_id()),
                        None,
                        Record::Stream(request.stream(opened.host().as_str(), &existing.id)?),
                        Actor::User,
                        None,
                        vec![],
                    )?,
                )?;
            }
            let stream = match stream {
                Some(stream) => stream,
                None => {
                    let stream = request.stream(opened.host().as_str(), &selected_id)?;
                    scope.claim(&stream.id)?;
                    receipt.persist(
                        opened.backend(),
                        Operation::new(
                            format!("{}:stream", request.operation_id()),
                            None,
                            Record::Stream(stream.clone()),
                            request.actor(),
                            None,
                            vec![],
                        )?,
                    )?;
                    receipt.stream = Some(stream.clone());
                    stream
                }
            };
            let authority = opened
                .backend()
                .authority(&stream.id)?
                .filter(|authority| authority.host_id == opened.host().as_str())
                .ok_or_else(|| {
                    CliError::message(
                        "effective ownership on this host is required to attach a conversation",
                    )
                })?;
            scope.claim(&stream.id)?;
            if session.is_none() {
                let proposed = request.session(opened.host().as_str(), &stream.id)?;
                receipt.persist(
                    opened.backend(),
                    Operation::new(
                        format!("{}:session", request.operation_id()),
                        None,
                        Record::Session(proposed.clone()),
                        request.actor(),
                        Some(authority.token.clone()),
                        vec![],
                    )?,
                )?;
                session = Some(proposed);
            }
            let session = session.expect("existing or accepted session");
            receipt.stream = Some(stream.clone());
            receipt.session = Some(session.clone());
            receipt.authority = Some(authority.clone());
            let checkouts = self.registry(&opened)?;
            let locations = self.locations(&opened)?;
            locations.remember(&stream.id, &directory, None, true, None, None)?;
            let binding =
                locations.remember(&stream.id, &directory, Some(&session.id), true, None, None)?;
            if let Some(checkout) = binding.checkout {
                checkouts.bind_inferred_repositories(
                    &checkout.id,
                    &streams_temper::process::NativeProcessRunner,
                )?;
            }
            scope.bind(&stream.id, &authority)?;
            Ok(())
        })();
        let mut code = if let Err(error) = completion {
            if receipt.saves.is_empty() && receipt.session.is_none() {
                return Err(error);
            }
            receipt.completion_error = Some(CompletionError {
                message: error.to_string(),
                detail: "enrollment incomplete; inspect saved records and receipts before retrying",
            });
            2
        } else {
            0
        };
        if code == 0 || receipt.saves.iter().any(|saved| saved.save.accepted()) {
            let publication = opened.backend().sync(SyncEvent::Attach);
            if publication.requires_repair() {
                code = 2;
            }
            receipt.publication = Some(result_data(&publication)?);
        }
        Ok(Output {
            value: serde_json::to_value(receipt)?,
            code,
            plain: None,
            notices: vec![],
        })
    }
}
