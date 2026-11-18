use super::args::ArtifactCommand;
use super::{App, CliError, Output, Result, application, json, runtime};
use streams::{
    artifacts::{ArtifactKey, ArtifactRequest},
    model::Session,
};

impl App {
    pub(super) fn artifact_cli(&self, command: ArtifactCommand) -> Result<Output> {
        let request = match command {
            ArtifactCommand::Write {
                name,
                session_id,
                stream_id,
                description,
                file,
                description_only,
            } => {
                let (text, file) = if description_only {
                    (None, None)
                } else if let Some(path) = file.filter(|path| path != std::path::Path::new("-")) {
                    (None, Some(self.path(&path)?))
                } else {
                    (Some(self.input.read()?), None)
                };
                ArtifactRequest::Write {
                    name,
                    description,
                    session_id: Some(session_id),
                    stream_id,
                    text,
                    file,
                }
            }
            ArtifactCommand::Read { reference } => ArtifactRequest::Read {
                reference,
                offset: None,
                limit: None,
            },
            ArtifactCommand::Delete { reference } => ArtifactRequest::Delete { reference },
            ArtifactCommand::List {
                host,
                stream_id,
                session_id: Some(session_id),
            } => ArtifactRequest::ListSession {
                host,
                stream_id,
                session_id: Some(session_id),
            },
            ArtifactCommand::List {
                host,
                stream_id: Some(stream_id),
                session_id: None,
            } => ArtifactRequest::ListStream { host, stream_id },
            ArtifactCommand::List { .. } => {
                return Err(CliError::message(
                    "artifact list requires --stream-id or --session-id",
                ));
            }
            ArtifactCommand::View { reference } => return self.artifact_view(&reference),
        };
        self.artifact_request(request, None)
    }

    /// The child supplies its native conversation; callers cannot write for another Session.
    pub(super) fn artifact_request(
        &self,
        request: ArtifactRequest,
        native_id: Option<&str>,
    ) -> Result<Output> {
        use application::artifacts as operations;
        let opened = application::open_records(&self.config)?;
        let host = opened.host().as_str();
        let bound = native_id.map(|id| Session::id_for(host, "pi", id));
        let producing_session = |requested: Option<String>| -> Result<String> {
            match (requested, bound.as_deref()) {
                (Some(requested), Some(bound)) if requested != bound => Err(CliError::message(
                    "Pi may write artifacts only for its bound Session",
                )),
                (Some(requested), _) => Ok(requested),
                (None, Some(bound)) => Ok(bound.into()),
                (None, None) => Err(CliError::message("artifact write requires --session-id")),
            }
        };
        let saved = match request {
            ArtifactRequest::Write {
                name,
                description,
                session_id,
                stream_id,
                text,
                file,
            } => {
                if text.is_some() && file.is_some() {
                    return Err(CliError::message(
                        "artifact write accepts text or a file, not both",
                    ));
                }
                let session_id = producing_session(session_id)?;
                let content = match file {
                    Some(file) => Some(operations::artifact_file(&self.path(&file)?)?),
                    None => text,
                };
                operations::write_artifact(
                    opened.artifacts(),
                    host,
                    &operations::WriteArtifact {
                        session_id,
                        stream_id,
                        name,
                        description,
                        content,
                    },
                )?
            }
            ArtifactRequest::Delete { reference } => {
                let key = ArtifactKey::parse(&reference)?;
                if bound
                    .as_deref()
                    .is_some_and(|bound| key.session != bound || key.host != host)
                {
                    return Err(CliError::message(
                        "Pi may delete artifacts only for its bound Session",
                    ));
                }
                operations::delete_artifact(opened.artifacts(), &key)?
            }
            ArtifactRequest::Read {
                reference,
                offset,
                limit,
            } => {
                let mut output = operations::read_artifact(
                    opened.artifacts(),
                    &ArtifactKey::parse(&reference)?,
                )?;
                let mut value = serde_json::to_value(&output)?;
                if native_id.is_some() || offset.is_some() || limit.is_some() {
                    let offset = offset.unwrap_or(0);
                    if !output.content.is_char_boundary(offset) {
                        return Err(CliError::message(
                            "artifact offset must be a UTF-8 byte boundary within the content",
                        ));
                    }
                    let mut end = offset
                        .saturating_add(limit.unwrap_or(32768).clamp(4, 32768))
                        .min(output.content.len());
                    while !output.content.is_char_boundary(end) {
                        end -= 1;
                    }
                    if let Some((index, _)) =
                        output.content[offset..end].match_indices('\n').nth(1999)
                    {
                        end = offset + index + 1;
                    }
                    value["total_bytes"] = json!(output.content.len());
                    value["next_offset"] = if end < output.content.len() {
                        json!(end)
                    } else {
                        serde_json::Value::Null
                    };
                    output.content = output.content[offset..end].into();
                    value["content"] = json!(output.content);
                }
                return Ok(Output::plain(value, output.content));
            }
            ArtifactRequest::ListSession {
                host: selected_host,
                session_id,
                stream_id,
            } => {
                let host = selected_host.as_deref().unwrap_or(host);
                let session_id = session_id
                    .or(bound)
                    .ok_or_else(|| CliError::message("artifact list requires a Session ID"))?;
                let artifacts = operations::list_session_artifacts(
                    opened.artifacts(),
                    host,
                    &session_id,
                    stream_id.as_deref(),
                )?;
                return Ok(Output::value(json!({"artifacts":artifacts})));
            }
            ArtifactRequest::ListStream {
                host: selected_host,
                stream_id,
            } => {
                let artifacts = operations::list_stream_artifacts(
                    opened.artifacts(),
                    selected_host.as_deref().unwrap_or(host),
                    &stream_id,
                )?;
                return Ok(Output::value(json!({"artifacts":artifacts})));
            }
        };
        let mut output = Output::value(json!(saved.save.outcome));
        output.accepted_save = true;
        output.notices = saved.warnings;
        if !output.notices.is_empty() {
            output.value["warnings"] = json!(output.notices);
            output.code = 2;
        }
        Ok(output)
    }

    fn artifact_view(&self, reference: &str) -> Result<Output> {
        use std::io::IsTerminal;
        if !std::io::stdout().is_terminal() {
            return Err(CliError::message(
                "artifact view requires a terminal; use artifact read for stdout",
            ));
        }
        let opened = application::open_records(&self.config)?;
        if self
            .environment
            .get("STREAMS_ARTIFACT_CATALOG_SOURCE")
            .is_some_and(|source| source != opened.source_id())
        {
            return Err(CliError::message("artifact viewer catalog changed"));
        }
        let output = application::artifacts::read_artifact(
            opened.artifacts(),
            &ArtifactKey::parse(reference)?,
        )?;
        let program = runtime::launch::resolve_executable("less", &self.environment, &self.cwd)
            .map_err(|error| CliError::message(error.to_string()))?;
        super::pager::show(&program, output.content.as_bytes(), "artifact")?;
        Ok(Output::plain(json!({"viewed":reference}), String::new()))
    }
}
