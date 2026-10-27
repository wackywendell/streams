//! Human Session discovery and one-record edits; no native binding or publication.
use super::args::SessionCommand;
use super::{
    App, CliError, Output, Result, application, binding_data, catalog, fields, json, resolve,
};
use streams::{
    model::{Checkout, Record, RecordKind, Session, Stream, TitleMode, ValidationError},
    storage::records::{RecordAddress, RecordStore},
};
use uuid::Uuid;

fn select_session(
    records: &[streams::storage::records::RecordRead],
    reference: &str,
) -> Result<Session> {
    let sessions: Vec<_> = records
        .iter()
        .filter_map(|read| match &read.record {
            Record::Session(session) => Some(session),
            _ => None,
        })
        .collect();
    Ok((*resolve(&sessions, reference, |session| {
        (&session.id, &session.title, Some(&session.host_id))
    })?)
    .clone())
}

impl App {
    pub(super) fn session(&self, command: SessionCommand) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        match command {
            SessionCommand::List { stream } => {
                let records = opened.inventory()?;
                let stream = stream
                    .as_deref()
                    .map(|reference| catalog::select_stream(&records, reference))
                    .transpose()?;
                let sessions: Vec<_> = records
                    .iter()
                    .filter_map(|read| match &read.record {
                        Record::Session(session)
                            if stream.is_none_or(|stream| {
                                session.host_id == stream.owner_host_id
                                    && session.stream_id == stream.id
                            }) =>
                        {
                            Some(session)
                        }
                        _ => None,
                    })
                    .collect();
                Ok(Output::value(json!({"sessions":sessions})))
            }
            SessionCommand::Show { reference } => Ok(Output::value(
                json!({"session":select_session(&opened.inventory()?,&reference)?}),
            )),
            SessionCommand::Attach {
                target,
                harness,
                session_id,
                title,
                cwd,
                repository,
                vcs_ref,
                directory_within_checkout,
            } => {
                let stream = self.record_target(&opened, &target)?;
                if stream.owner_host_id != opened.host().as_str() {
                    return Err(CliError::message("Stream belongs to another writing host"));
                }
                let checkout = match (repository.as_deref(), vcs_ref) {
                    (Some(alias), Some(reference)) => {
                        let url = stream.repositories.get(alias).ok_or_else(|| {
                            CliError::message(format!("unknown repository alias: {alias:?}"))
                        })?;
                        let mut checkout =
                            Checkout::new(alias, reference, directory_within_checkout)?;
                        checkout.repository_url = Some(url.url.clone());
                        Some(checkout)
                    }
                    (None, None) if directory_within_checkout == "." => None,
                    (None, _) => {
                        return Err(CliError::message(
                            "checkout --ref/--directory-within-checkout requires --repository",
                        ));
                    }
                    (Some(_), None) => {
                        return Err(CliError::message("checkout --repository requires --ref"));
                    }
                };
                let host = opened.host().as_str();
                let mut session = Session::new(
                    Session::id_for(host, harness.as_ref(), &session_id),
                    host,
                    &stream.id,
                    harness.as_ref(),
                    session_id,
                    title,
                )?;
                session.title_mode = TitleMode::Manual;
                session.checkout = checkout;
                let operation = Uuid::new_v4().to_string();
                let saved = application::create_record(
                    opened.records(),
                    &operation,
                    &Record::Session(session),
                );
                let actual = saved
                    .as_ref()
                    .ok()
                    .map(|saved| match &saved.current.record {
                        Record::Session(session) => session.clone(),
                        _ => unreachable!("Session creation target"),
                    });
                let mut output = catalog::finish_save(&operation, saved)?;
                if output.accepted_save {
                    let session = actual.expect("accepted creation returns its actual Session");
                    let completion = (|| -> Result<()> {
                        let directory = self.path(cwd.as_deref().unwrap_or(&self.cwd))?;
                        let url = session
                            .checkout
                            .as_ref()
                            .and_then(|checkout| checkout.repository_url.as_deref());
                        self.locations(opened.root(), host)?.remember(
                            &session.stream_id,
                            &directory,
                            Some(&session.id),
                            false,
                            url,
                            None,
                        )?;
                        Ok(())
                    })();
                    if let Err(error) = completion {
                        output.code = 2;
                        output.value["completion_error"] = json!({"message":error.to_string(),"detail":"Session saved; select a valid directory before resuming"});
                    }
                }
                Ok(output)
            }
            SessionCommand::Update {
                reference,
                title,
                fields: intent,
            } => {
                let before = select_session(&opened.inventory()?, &reference)?;
                let settings = self.config.load()?;
                if settings.catalog_root != opened.root() {
                    return Err(CliError::message(
                        "catalog root setting changed during Session update",
                    ));
                }
                let mut changes =
                    fields::session(&before, &intent, &mut |path| self.read_text(path))?;
                if let Some(title) = title {
                    changes.insert("title".into(), json!(title));
                }
                if changes.is_empty() {
                    return Err(CliError::message(
                        "session update requires at least one field change",
                    ));
                }
                let operation = Uuid::new_v4().to_string();
                catalog::finish_save(
                    &operation,
                    application::save_user_edit(
                        opened.records(),
                        &operation,
                        &Record::Session(before),
                        &changes,
                        settings.naming,
                    ),
                )
            }
            SessionCommand::Directory {
                reference,
                cwd,
                checkout_id,
                within,
                repository,
            } => {
                let session = select_session(&opened.inventory()?, &reference)?;
                if session.host_id != opened.host().as_str() {
                    return Err(CliError::message(
                        "directory selection requires a Session belonging to this host",
                    ));
                }
                if within.is_some() && checkout_id.is_none() {
                    return Err(CliError::message("--within requires --checkout"));
                }
                if repository.is_some() && cwd.is_none() && checkout_id.is_none() {
                    return Err(CliError::message(
                        "--repository requires --cwd or --checkout",
                    ));
                }
                let locations = self.locations(opened.root(), opened.host().as_str())?;
                if cwd.is_some() || checkout_id.is_some() {
                    let portable_url = session
                        .checkout
                        .as_ref()
                        .and_then(|checkout| checkout.repository_url.as_deref());
                    let selected_url = if let Some(alias) = repository.as_deref() {
                        let address = RecordAddress::new(
                            &session.host_id,
                            RecordKind::Stream,
                            &session.stream_id,
                        )
                        .map_err(application::ApplicationError::from)?;
                        let stream = opened.records().with_reader(
                            |reader| -> application::Result<Stream> {
                                let dependency = reader.read(&address)?.ok_or_else(|| {
                                    ValidationError::new("Session Stream is missing")
                                })?;
                                let Record::Stream(stream) = dependency.record else {
                                    unreachable!("addressed Stream codec")
                                };
                                Ok(stream)
                            },
                        )?;
                        Some(
                            stream
                                .repositories
                                .get(alias)
                                .ok_or_else(|| {
                                    CliError::message(format!(
                                        "unknown repository alias: {alias:?}"
                                    ))
                                })?
                                .url
                                .clone(),
                        )
                    } else {
                        portable_url.map(str::to_owned)
                    };
                    if repository.is_some()
                        && portable_url.is_some()
                        && selected_url.as_deref() != portable_url
                    {
                        return Err(CliError::message(
                            "selected repository differs from the Session's portable checkout",
                        ));
                    }
                    let directory = if let Some(id) = checkout_id.as_deref() {
                        self.registry(opened.host().as_str())?
                            .directory(id, within.as_deref().unwrap_or("."))?
                    } else {
                        self.path(cwd.as_deref().expect("directory selection"))?
                    };
                    locations.remember(
                        &session.stream_id,
                        &directory,
                        Some(&session.id),
                        false,
                        selected_url.as_deref(),
                        checkout_id.as_deref(),
                    )?;
                }
                let directory = locations
                    .read(&session.stream_id, Some(&session.id))?
                    .map(|binding| binding_data(&binding));
                Ok(Output::value(
                    json!({"session_id":session.id,"host_id":session.host_id,"directory":directory}),
                ))
            }
        }
    }
}
