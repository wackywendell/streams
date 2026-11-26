//! `streams session` subcommand handlers (`list`, `show`, `create`, `edit`, `resume`, `document`).

use std::{
    fs,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Map, Value, json};
use streams::{
    application::{
        self, SessionFilter, documents,
        fields::{
            SESSION_FIELDS, SESSION_LIST_DEFAULTS, all_field_names, check_list_text_fields,
            project_session, resolve_fields,
        },
    },
    file_store::records::relative_record_path,
    model::{
        NamingMode, ReferenceEntry, ReferenceKind, ReferenceStatus, Session, SessionDirectory,
        Status, TextPair, Title,
    },
    operations::edits::EditRequest,
    runtime::launch::SessionCommand as LaunchSessionCommand,
    storage::records::RecordAddress,
};

use super::{
    App, CliError, Output, Result,
    args::{
        DocumentCommand, ListOutputArgs, SessionCommand, SessionProgressArgs, SessionReferenceArgs,
    },
    pager, select,
};

impl App {
    pub(super) fn session(&self, command: SessionCommand, json_mode: bool) -> Result<Output> {
        match command {
            SessionCommand::List {
                query,
                stream,
                keys,
                status,
                all,
                output,
            } => self.session_list(query, stream, keys, status, all, output, json_mode),
            SessionCommand::Show { session, output } => {
                self.session_show(&session, output.fields.as_deref())
            }
            SessionCommand::Create {
                session,
                stream,
                title,
                title_mode,
                status,
                directory,
                progress,
            } => self.session_create(
                &session, &stream, title, title_mode, status, directory, progress,
            ),
            SessionCommand::Edit {
                session,
                input,
                title,
                title_mode,
                status,
                stream,
                directory,
                progress,
                references,
            } => self.session_edit(
                &session,
                input.as_deref(),
                title,
                title_mode,
                status,
                stream,
                directory,
                progress,
                references,
            ),
            SessionCommand::Resume { session } => self.session_resume(&session, json_mode),
            SessionCommand::Document { command } => self.session_document(command, json_mode),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn session_list(
        &self,
        query: Option<String>,
        stream: Option<String>,
        keys: Vec<String>,
        status: Vec<Status>,
        all: bool,
        output: ListOutputArgs,
        json_mode: bool,
    ) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let selected_fields = resolve_fields(
            SESSION_FIELDS,
            "Session",
            output.fields.as_deref(),
            SESSION_LIST_DEFAULTS,
        )?;
        if !json_mode {
            check_list_text_fields(SESSION_FIELDS, &selected_fields)?;
        }
        let stream_filter = match stream {
            Some(selector) => {
                let resolved = select::select_stream(&opened, &selector)?;
                Some((resolved.host_id, resolved.id))
            }
            None => None,
        };
        let key_addresses = keys
            .iter()
            .map(|raw| select::parse_session_full_key(raw))
            .collect::<Result<Vec<_>>>()?;

        let listed = application::list_sessions(
            &opened,
            &SessionFilter {
                query: query.as_deref(),
                stream: stream_filter,
                keys: key_addresses,
                status: &status,
                all,
            },
        )?;

        let total = listed.sessions.len();
        let kept = match output.limit {
            Some(limit) => &listed.sessions[..total.min(limit)],
            None => &listed.sessions[..],
        };
        let mut items = Vec::with_capacity(kept.len());
        for entry in kept {
            let host_label = listed
                .host_labels
                .get(&entry.session.host_id)
                .map(String::as_str);
            items.push(project_session(
                opened.host().as_str(),
                &entry.session,
                host_label,
                &selected_fields,
            )?);
        }

        Ok(self.render_list_output(
            items,
            &selected_fields,
            total,
            listed.unreadable,
            opened.host().as_str(),
        ))
    }

    fn session_show(&self, selector: &str, requested_fields: Option<&str>) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let defaults = all_field_names(SESSION_FIELDS);
        let selected_fields =
            resolve_fields(SESSION_FIELDS, "Session", requested_fields, &defaults)?;
        let session = select::select_session(&opened, selector)?;
        let host_label = if selected_fields.contains(&"host_label") {
            application::read_host_label(&opened, &session.host_id)?
        } else {
            None
        };
        let projected = project_session(
            opened.host().as_str(),
            &session,
            host_label.as_deref(),
            &selected_fields,
        )?;
        Ok(self.render_show_output(projected, &selected_fields))
    }

    #[allow(clippy::too_many_arguments)]
    fn session_create(
        &self,
        raw_session: &str,
        stream_selector: &str,
        title: String,
        title_mode: Option<NamingMode>,
        status: Option<Status>,
        directory: Option<PathBuf>,
        progress: SessionProgressArgs,
    ) -> Result<Output> {
        check_progress_short_flags(&progress)?;
        let (harness, id) = select::parse_exact_harness_id(raw_session)?;
        let opened = application::open_records(&self.config)?;
        let host = opened.host().as_str();
        let stream = select::select_stream(&opened, stream_selector)?;
        if stream.host_id != host {
            return Err(CliError::message(format!(
                "cannot attach a local Session to Stream {} owned by host {:?}",
                stream.key(),
                stream.host_id
            )));
        }

        let title_manual = title_mode != Some(NamingMode::Automatic);
        let mut session = Session::new(
            host,
            harness,
            id,
            &stream.id,
            Title::new(title, title_manual),
        )?;
        if let Some(status) = status {
            session.status = status;
        }
        if let Some(dir_path) = directory {
            let resolved = self.resolve_cwd_path(&dir_path);
            session.directory = Some(documents::inspect_directory(&resolved)?);
        }
        session.recap = self.build_text_pair(
            progress.recap,
            progress.recap_file,
            progress.clear_recap,
            progress.recap_short,
        )?;
        session.next = self.build_text_pair(
            progress.next,
            progress.next_file,
            progress.clear_next,
            progress.next_short,
        )?;

        let address = RecordAddress::for_session(&session);
        let save = application::create_session(opened.records(), &session)?;
        let file = opened
            .root()
            .join(relative_record_path(&address).map_err(application::ApplicationError::from)?);
        Ok(self.render_create_output(&save.target(), vec![file], save.written))
    }

    #[allow(clippy::too_many_arguments)]
    fn session_edit(
        &self,
        selector: &str,
        input: Option<&str>,
        title: Option<String>,
        title_mode: Option<NamingMode>,
        status: Option<Status>,
        stream_selector: Option<String>,
        directory: Option<PathBuf>,
        progress: SessionProgressArgs,
        references: SessionReferenceArgs,
    ) -> Result<Output> {
        let any_flag = title.is_some()
            || title_mode.is_some()
            || status.is_some()
            || stream_selector.is_some()
            || directory.is_some()
            || progress.any_set()
            || references.any_set();
        if input.is_some() && any_flag {
            return Err(CliError::Usage(
                "--input cannot be combined with field or reference flags".into(),
            ));
        }
        if input.is_none() && !any_flag {
            return Err(CliError::Usage(
                "session edit requires field flags, reference flags, or --input -".into(),
            ));
        }
        check_progress_short_flags(&progress)?;
        check_reference_flags(&references)?;

        let opened = application::open_records(&self.config)?;
        let session = select::select_session(&opened, selector)?;
        let host = opened.host().as_str();
        if session.host_id != host {
            return Err(CliError::message(format!(
                "cannot edit Session {} owned by host {:?} from host {:?}",
                session.key(),
                session.host_id,
                host
            )));
        }

        let request = match input {
            Some(source) => self.read_edit_input(source)?,
            None => self.build_session_flag_request(
                &opened,
                &session,
                title,
                title_mode,
                status,
                stream_selector,
                directory,
                progress,
                references,
            )?,
        };
        let target = RecordAddress::for_session(&session);
        let result = application::edit_session(opened.records(), &target, &request)?;
        Ok(self.render_edit_output(result))
    }

    #[allow(clippy::too_many_arguments)]
    fn build_session_flag_request(
        &self,
        opened: &application::OpenedRecords,
        current: &Session,
        title: Option<String>,
        title_mode: Option<NamingMode>,
        status: Option<Status>,
        stream_selector: Option<String>,
        directory: Option<PathBuf>,
        progress: SessionProgressArgs,
        references: SessionReferenceArgs,
    ) -> Result<EditRequest> {
        let mut changes = Map::new();

        if let Some(text) = title {
            let manual = title_mode != Some(NamingMode::Automatic);
            changes.insert("title".into(), json!({ "text": text, "manual": manual }));
        } else if let Some(mode) = title_mode {
            let manual = mode == NamingMode::Manual;
            changes.insert(
                "title".into(),
                json!({ "text": current.title.text, "manual": manual }),
            );
        }

        if let Some(status) = status {
            changes.insert(
                "status".into(),
                serde_json::to_value(status).expect("status json"),
            );
        }

        if let Some(selector) = stream_selector {
            let dest = select::select_stream(opened, &selector)?;
            if dest.host_id != current.host_id {
                return Err(CliError::message(format!(
                    "cannot move Session {} to Stream {} on another host {:?}",
                    current.key(),
                    dest.key(),
                    dest.host_id
                )));
            }
            changes.insert("stream_id".into(), Value::String(dest.id));
        }

        let new_directory: Option<SessionDirectory> = match directory {
            Some(dir_path) => {
                let resolved = self.resolve_cwd_path(&dir_path);
                let inspected = documents::inspect_directory(&resolved)?;
                changes.insert(
                    "directory".into(),
                    serde_json::to_value(&inspected).expect("directory json"),
                );
                Some(inspected)
            }
            None => None,
        };
        let effective_directory = new_directory.as_ref().or(current.directory.as_ref());

        if progress.clear_recap {
            changes.insert("recap".into(), Value::Null);
        } else if progress.recap.is_some() || progress.recap_file.is_some() {
            let pair = self.build_text_pair(
                progress.recap,
                progress.recap_file,
                false,
                progress.recap_short,
            )?;
            changes.insert(
                "recap".into(),
                serde_json::to_value(pair).expect("recap json"),
            );
        }

        if progress.clear_next {
            changes.insert("next".into(), Value::Null);
        } else if progress.next.is_some() || progress.next_file.is_some() {
            let pair = self.build_text_pair(
                progress.next,
                progress.next_file,
                false,
                progress.next_short,
            )?;
            changes.insert(
                "next".into(),
                serde_json::to_value(pair).expect("next json"),
            );
        }

        if references.any_set() {
            let mut ref_changes = Map::new();
            if let Some((kind, raw_key)) = added_reference(&references) {
                let key = if kind == ReferenceKind::Document {
                    documents::normalize_document_key(effective_directory, raw_key)?
                } else {
                    raw_key.to_owned()
                };
                let entry = ReferenceEntry {
                    kind,
                    name: references.name.expect("checked by check_reference_flags"),
                    purpose: references
                        .purpose
                        .expect("checked by check_reference_flags"),
                    role: references.role.expect("checked by check_reference_flags"),
                    status: ReferenceStatus::Active,
                    manual: true,
                };
                ref_changes.insert(
                    key,
                    serde_json::to_value(entry).expect("reference entry json"),
                );
            }
            for drop_key in references.drop_reference {
                let Some(existing) = current.references.get(&drop_key) else {
                    return Err(CliError::NotFound(format!(
                        "no reference {drop_key:?} on Session {}",
                        current.key()
                    )));
                };
                let mut dropped = existing.clone();
                dropped.status = ReferenceStatus::Dropped;
                dropped.manual = true;
                ref_changes.insert(
                    drop_key,
                    serde_json::to_value(dropped).expect("reference entry json"),
                );
            }
            changes.insert("references".into(), Value::Object(ref_changes));
        }

        Ok(EditRequest {
            changes,
            before: None,
        })
    }

    pub(super) fn session_resume(&self, selector: &str, json_mode: bool) -> Result<Output> {
        if json_mode {
            return Err(CliError::Usage(
                "session resume takes over the terminal and does not support --json".into(),
            ));
        }
        let opened = application::open_records(&self.config)?;
        let session = select::select_session(&opened, selector)?;
        if session.host_id != opened.host().as_str() {
            return Err(CliError::message(format!(
                "cannot resume Session {} owned by host {:?} on host {:?}",
                session.key(),
                session.host_id,
                opened.host().as_str()
            )));
        }
        let Some(directory) = &session.directory else {
            return Err(CliError::message(format!(
                "Session {} has no recorded directory; set one with `streams session edit {} --directory DIR`",
                session.key(),
                session.key()
            )));
        };
        let workdir = Path::new(&directory.path);
        if !workdir.is_dir() {
            return Err(CliError::message(format!(
                "Session directory {} does not exist",
                workdir.display()
            )));
        }
        let launch = LaunchSessionCommand::for_session(&session)?;
        let argv = launch.argv();
        let error = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(workdir)
            .exec();
        Err(CliError::message(format!(
            "failed to run {:?} in {}: {error}",
            launch.command_text(),
            workdir.display()
        )))
    }

    fn session_document(&self, command: DocumentCommand, json_mode: bool) -> Result<Output> {
        match command {
            DocumentCommand::Read { session, path } => {
                if json_mode {
                    return Err(CliError::Usage(
                        "session document read prints raw file bytes and does not support --json"
                            .into(),
                    ));
                }
                let opened = application::open_records(&self.config)?;
                let session = select::select_session(&opened, &session)?;
                let file = locate_available_document(opened.host().as_str(), &session, &path)?;
                let bytes = fs::read(&file).map_err(|error| {
                    CliError::message(format!("cannot read {}: {error}", file.display()))
                })?;
                Ok(Output::raw_bytes(bytes))
            }
            DocumentCommand::View { session, path } => {
                if json_mode {
                    return Err(CliError::Usage(
                        "session document view takes over the terminal and does not support --json"
                            .into(),
                    ));
                }
                let opened = application::open_records(&self.config)?;
                let session = select::select_session(&opened, &session)?;
                let file = locate_available_document(opened.host().as_str(), &session, &path)?;
                pager::view_file(
                    opened.settings().herdr.viewer_mode,
                    &self.environment,
                    &self.cwd,
                    &file,
                )?;
                Ok(Output::silent())
            }
        }
    }

    fn build_text_pair(
        &self,
        text: Option<String>,
        file: Option<PathBuf>,
        clear: bool,
        short: Option<String>,
    ) -> Result<Option<TextPair>> {
        if clear {
            return Ok(None);
        }
        let body = match (text, file) {
            (Some(text), None) => text,
            (None, Some(path)) => self.read_text_file(&path)?,
            _ => return Ok(None),
        };
        Ok(Some(TextPair::new(body, short)))
    }
}

fn locate_available_document(local_host: &str, session: &Session, path: &str) -> Result<PathBuf> {
    match documents::locate(local_host, session, path)? {
        documents::DocumentLocation::Path { path } => Ok(path),
        documents::DocumentLocation::Unavailable { reason } => Err(CliError::NotFound(format!(
            "document {path:?} on Session {} is not available here: {reason}",
            session.key()
        ))),
    }
}

fn check_progress_short_flags(progress: &SessionProgressArgs) -> Result<()> {
    if progress.recap_short.is_some() && progress.recap.is_none() && progress.recap_file.is_none() {
        return Err(CliError::Usage(
            "--recap-short requires --recap or --recap-file".into(),
        ));
    }
    if progress.next_short.is_some() && progress.next.is_none() && progress.next_file.is_none() {
        return Err(CliError::Usage(
            "--next-short requires --next or --next-file".into(),
        ));
    }
    Ok(())
}

fn added_reference(refs: &SessionReferenceArgs) -> Option<(ReferenceKind, &str)> {
    if let Some(url) = refs.add_pr.as_deref() {
        Some((ReferenceKind::Pr, url))
    } else if let Some(url) = refs.add_issue.as_deref() {
        Some((ReferenceKind::Issue, url))
    } else if let Some(url) = refs.add_link.as_deref() {
        Some((ReferenceKind::Link, url))
    } else if let Some(path) = refs.add_document.as_deref() {
        Some((ReferenceKind::Document, path))
    } else {
        None
    }
}

fn check_reference_flags(refs: &SessionReferenceArgs) -> Result<()> {
    let any_meta = refs.name.is_some() || refs.purpose.is_some() || refs.role.is_some();
    if refs.any_add() {
        if refs.name.is_none() || refs.purpose.is_none() || refs.role.is_none() {
            return Err(CliError::Usage(
                "adding a reference requires --name, --purpose and --role".into(),
            ));
        }
    } else if any_meta {
        return Err(CliError::Usage(
            "--name, --purpose and --role require --add-pr, --add-issue, --add-link or --add-document"
                .into(),
        ));
    }
    Ok(())
}
