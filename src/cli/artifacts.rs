//! `streams artifact` subcommand handlers (`list`, `read`, `view`, `write`, `delete`).

use std::path::PathBuf;

use serde_json::{Map, Value};
use streams::{
    application::{
        self,
        artifacts::{
            WriteArtifact, artifact_file, delete_artifact, list_session_artifacts,
            list_stream_artifacts, read_artifact, write_artifact,
        },
        fields::{ARTIFACT_FIELDS, ARTIFACT_LIST_DEFAULTS, resolve_fields},
    },
    artifacts::MAX_CONTENT_BYTES,
    file_store::records::{relative_artifact_path, relative_record_path},
    runtime,
    storage::records::RecordAddress,
};

use super::{
    App, CliError, Output, Result,
    args::{ArtifactCommand, ListOutputArgs},
    pager, select,
};

impl App {
    pub(super) fn artifact(&self, command: ArtifactCommand, json_mode: bool) -> Result<Output> {
        match command {
            ArtifactCommand::List {
                session,
                stream,
                output,
            } => self.artifact_list(session, stream, output),
            ArtifactCommand::Read { artifact } => self.artifact_read(&artifact, json_mode),
            ArtifactCommand::View { artifact } => self.artifact_view(&artifact, json_mode),
            ArtifactCommand::Write {
                name,
                session,
                description,
                file,
                description_only,
            } => self.artifact_write(name, &session, description, file, description_only),
            ArtifactCommand::Delete { artifact, confirm } => {
                self.artifact_delete(&artifact, confirm)
            }
        }
    }

    fn artifact_list(
        &self,
        session_selector: Option<String>,
        stream_selector: Option<String>,
        output: ListOutputArgs,
    ) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let selected_fields = resolve_fields(
            ARTIFACT_FIELDS,
            "Artifact",
            output.fields.as_deref(),
            ARTIFACT_LIST_DEFAULTS,
        )?;

        let (rows, unreadable) = match (session_selector, stream_selector) {
            (Some(selector), None) => {
                let session = select::select_session(&opened, &selector)?;
                let rows = list_session_artifacts(
                    opened.artifacts(),
                    &session.host_id,
                    &session.harness,
                    &session.id,
                )?;
                (rows, Vec::new())
            }
            (None, Some(selector)) => {
                let stream = select::select_stream(&opened, &selector)?;
                list_stream_artifacts(&opened, &stream.host_id, &stream.id)?
            }
            _ => {
                return Err(CliError::Usage(
                    "artifact list requires exactly one of --session or --stream".into(),
                ));
            }
        };

        let total = rows.len();
        let kept = match output.limit {
            Some(limit) => &rows[..total.min(limit)],
            None => &rows[..],
        };
        let items: Vec<Map<String, Value>> = kept
            .iter()
            .map(|row| row.project(&selected_fields))
            .collect();

        Ok(self.render_list_output(
            items,
            &selected_fields,
            total,
            unreadable,
            opened.host().as_str(),
        ))
    }

    fn artifact_read(&self, selector: &str, json_mode: bool) -> Result<Output> {
        if json_mode {
            return Err(CliError::Usage(
                "artifact read prints raw file bytes and does not support --json".into(),
            ));
        }
        let opened = application::open_records(&self.config)?;
        let key = select::select_artifact(&opened, selector)?;
        let read = read_artifact(opened.artifacts(), &key)?;
        Ok(Output::raw_bytes(read.content.into_bytes()))
    }

    fn artifact_view(&self, selector: &str, json_mode: bool) -> Result<Output> {
        use std::io::IsTerminal;
        if json_mode {
            return Err(CliError::Usage(
                "artifact view takes over the terminal and does not support --json".into(),
            ));
        }
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
        let key = select::select_artifact(&opened, selector)?;
        let read = read_artifact(opened.artifacts(), &key)?;
        let program = runtime::launch::resolve_executable("less", &self.environment, &self.cwd)
            .map_err(|_| CliError::message("less is unavailable"))?;
        pager::show(&program, read.content.as_bytes(), "artifact")?;
        Ok(Output::silent())
    }

    fn artifact_write(
        &self,
        name: String,
        session_selector: &str,
        description: String,
        file: Option<PathBuf>,
        description_only: bool,
    ) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let session = select::select_session(&opened, session_selector)?;
        let host = opened.host().as_str();
        if session.host_id != host {
            return Err(CliError::message(format!(
                "cannot write an artifact on Session {} owned by host {:?} from host {:?}",
                session.key(),
                session.host_id,
                host
            )));
        }

        let content = if description_only {
            None
        } else if let Some(path) = file {
            let resolved = self.resolve_cwd_path(&path);
            Some(artifact_file(&resolved)?)
        } else {
            Some(self.read_stdin_bounded(MAX_CONTENT_BYTES)?)
        };

        let intent = WriteArtifact {
            harness: session.harness.clone(),
            session_id: session.id.clone(),
            name: name.clone(),
            description,
            content,
        };
        let saved = write_artifact(opened.artifacts(), host, &intent)?;
        let key = streams::artifacts::ArtifactKey {
            host: host.to_owned(),
            harness: session.harness.clone(),
            session: session.id.clone(),
            name,
        };
        let session_file = opened.root().join(
            relative_record_path(&RecordAddress::for_session(&session))
                .map_err(application::ApplicationError::from)?,
        );
        let mut files = vec![session_file];
        if !description_only {
            files.push(
                opened.root().join(
                    relative_artifact_path(&key).map_err(application::ApplicationError::from)?,
                ),
            );
        }
        let wrote = saved.save.written || saved.content_written;
        let mut output = self.render_create_output(&key.to_string(), files, wrote);
        output.notices.extend(saved.warnings);
        Ok(output)
    }

    fn artifact_delete(&self, selector: &str, confirm: bool) -> Result<Output> {
        if !confirm {
            return Err(CliError::Usage("artifact delete requires --confirm".into()));
        }
        let opened = application::open_records(&self.config)?;
        let key = select::select_artifact(&opened, selector)?;
        let host = opened.host().as_str();
        if key.host != host {
            return Err(CliError::message(format!(
                "cannot delete Artifact {key} owned by host {:?} from host {:?}",
                key.host, host
            )));
        }
        let saved = delete_artifact(opened.artifacts(), &key)?;
        let session_file = opened.root().join(
            relative_record_path(
                &key.session_address()
                    .map_err(application::ApplicationError::from)?,
            )
            .map_err(application::ApplicationError::from)?,
        );
        let wrote = saved.save.written || saved.content_written;
        let mut output = self.render_create_output(&key.to_string(), vec![session_file], wrote);
        output.notices.extend(saved.warnings);
        Ok(output)
    }
}
