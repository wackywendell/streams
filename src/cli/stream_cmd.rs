//! `streams stream` subcommand handlers (`list`, `show`, `create`, `edit`).

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};
use streams::{
    application::{
        self, OpenedRecords, StreamFilter,
        fields::{
            STREAM_FIELDS, STREAM_LIST_DEFAULTS, all_field_names, check_list_text_fields,
            project_stream, resolve_fields,
        },
    },
    file_store::records::relative_record_path,
    model::{Goal, NamingMode, Record, Session, Stream, Title},
    operations::{decay_cutoff_ms, edits::EditRequest},
    storage::records::RecordAddress,
};
use uuid::Uuid;

use super::{
    App, CliError, Output, Result,
    args::{StreamCommand, StreamCreateFieldArgs, StreamEditFieldArgs, StreamListArgs},
    select,
};

const STREAM_MEMBER_FIELDS: &[&str] = &[
    "status",
    "active_sessions",
    "total_sessions",
    "last_activity_ms",
    "sessions",
    "artifacts",
];

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

impl App {
    pub(super) fn stream(&self, command: StreamCommand, json_mode: bool) -> Result<Output> {
        match command {
            StreamCommand::List(args) => self.stream_list(args, json_mode),
            StreamCommand::Show { stream, output } => {
                self.stream_show(&stream, output.fields.as_deref())
            }
            StreamCommand::Create { fields } => self.stream_create(fields),
            StreamCommand::Edit {
                stream,
                input,
                fields,
            } => self.stream_edit(&stream, input.as_deref(), fields),
            StreamCommand::Delete { stream } => self.stream_delete(&stream),
        }
    }

    pub(super) fn stream_list(&self, args: StreamListArgs, json_mode: bool) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let selected_fields = resolve_fields(
            STREAM_FIELDS,
            "Stream",
            args.output.fields.as_deref(),
            STREAM_LIST_DEFAULTS,
        )?;
        if !json_mode {
            check_list_text_fields(STREAM_FIELDS, &selected_fields)?;
        }
        let host_filter = if args.local {
            Some(opened.host().as_str())
        } else {
            args.host.as_deref()
        };
        let cutoff = decay_cutoff_ms(opened.settings().lifecycle.decay_days, now_ms());
        let listed = application::list_streams(
            &opened,
            StreamFilter {
                query: args.query.as_deref(),
                status: &args.status,
                all: args.all,
                host: host_filter,
            },
            cutoff,
        )?;

        let total = listed.streams.len();
        let kept = match args.output.limit {
            Some(limit) => &listed.streams[..total.min(limit)],
            None => &listed.streams[..],
        };
        let items: Vec<Map<String, Value>> = kept
            .iter()
            .map(|entry| {
                let refs: Vec<&Session> = entry.members.iter().collect();
                let stream_cutoff =
                    cutoff.filter(|_| entry.stream.host_id == opened.host().as_str());
                project_stream(&entry.stream, &refs, stream_cutoff, &selected_fields)
            })
            .collect();

        Ok(self.render_list_output(
            items,
            &selected_fields,
            total,
            listed.unreadable,
            opened.host().as_str(),
        ))
    }

    fn stream_show(&self, selector: &str, requested_fields: Option<&str>) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let defaults = all_field_names(STREAM_FIELDS);
        let selected_fields = resolve_fields(STREAM_FIELDS, "Stream", requested_fields, &defaults)?;
        let stream = select::select_stream(&opened, selector)?;
        let needs_members = selected_fields
            .iter()
            .any(|field| STREAM_MEMBER_FIELDS.contains(field));
        let members = if needs_members {
            load_stream_members(&opened, &stream)?
        } else {
            Vec::new()
        };
        let refs: Vec<&Session> = members.iter().collect();
        let cutoff = (stream.host_id == opened.host().as_str())
            .then(|| decay_cutoff_ms(opened.settings().lifecycle.decay_days, now_ms()))
            .flatten();
        let projected = project_stream(&stream, &refs, cutoff, &selected_fields);
        Ok(self.render_show_output(projected, &selected_fields))
    }

    fn stream_create(&self, args: StreamCreateFieldArgs) -> Result<Output> {
        if args.goal_short.is_some() && args.goal.is_none() && args.goal_file.is_none() {
            return Err(CliError::Usage(
                "--goal-short requires --goal or --goal-file".into(),
            ));
        }
        if args.goal_mode.is_some() && args.goal.is_none() && args.goal_file.is_none() {
            return Err(CliError::Usage(
                "--goal-mode requires --goal or --goal-file when creating a Stream".into(),
            ));
        }
        let goal_text = match (args.goal, args.goal_file) {
            (Some(text), None) => Some(text),
            (None, Some(path)) => Some(self.read_text_file(&path)?),
            _ => None,
        };
        let title_manual = args.title_mode != Some(NamingMode::Automatic);
        let title = Title::new(args.title, title_manual);
        let goal = goal_text.map(|text| {
            let goal_manual = args.goal_mode != Some(NamingMode::Automatic);
            Goal::new(text, args.goal_short, goal_manual)
        });

        let opened = application::open_records(&self.config)?;
        let id = Uuid::new_v4().to_string();
        let stream = Stream::new(opened.host().as_str(), id, title, goal)?;
        let address = RecordAddress::for_stream(&stream);
        let save = application::create_stream(opened.records(), &stream)?;
        let file = opened
            .root()
            .join(relative_record_path(&address).map_err(application::ApplicationError::from)?);
        Ok(self.render_create_output(&save.target(), vec![file], save.written))
    }

    fn stream_edit(
        &self,
        selector: &str,
        input: Option<&str>,
        fields: StreamEditFieldArgs,
    ) -> Result<Output> {
        if input.is_some() && fields.any_set() {
            return Err(CliError::Usage(
                "--input cannot be combined with field flags".into(),
            ));
        }
        if input.is_none() && !fields.any_set() {
            return Err(CliError::Usage(
                "stream edit requires field flags or --input -".into(),
            ));
        }
        if fields.goal_short.is_some() && fields.goal.is_none() && fields.goal_file.is_none() {
            return Err(CliError::Usage(
                "--goal-short requires --goal or --goal-file".into(),
            ));
        }

        let opened = application::open_records(&self.config)?;
        let stream = select::select_stream(&opened, selector)?;
        if stream.host_id != opened.host().as_str() {
            return Err(CliError::message(format!(
                "cannot edit Stream {} owned by host {:?} from host {:?}",
                stream.key(),
                stream.host_id,
                opened.host().as_str()
            )));
        }

        let request = match input {
            Some(source) => self.read_edit_input(source)?,
            None => self.build_stream_flag_request(&stream, fields)?,
        };
        let target = RecordAddress::for_stream(&stream);
        let result = application::edit_stream(opened.records(), &target, &request)?;
        Ok(self.render_edit_output(result))
    }

    fn stream_delete(&self, selector: &str) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let stream = select::select_stream(&opened, selector)?;
        if stream.host_id != opened.host().as_str() {
            return Err(CliError::message(format!(
                "cannot delete Stream {} owned by host {:?} from host {:?}",
                stream.key(),
                stream.host_id,
                opened.host().as_str()
            )));
        }
        let key = stream.key().to_string();
        let target = RecordAddress::for_stream(&stream);
        opened.delete_stream(&target)?;
        let mut output = Output::plain(
            json!({ "key": key, "deleted": true }),
            format!("deleted {key}"),
        );
        output.accepted_save = true;
        output.wrote = true;
        Ok(output)
    }

    fn build_stream_flag_request(
        &self,
        current: &Stream,
        fields: StreamEditFieldArgs,
    ) -> Result<EditRequest> {
        let mut changes = Map::new();

        if let Some(text) = fields.title {
            let manual = fields.title_mode != Some(NamingMode::Automatic);
            changes.insert("title".into(), json!({ "text": text, "manual": manual }));
        } else if let Some(mode) = fields.title_mode {
            let manual = mode == NamingMode::Manual;
            changes.insert(
                "title".into(),
                json!({ "text": current.title.text, "manual": manual }),
            );
        }

        if fields.clear_goal {
            changes.insert("goal".into(), Value::Null);
        } else if fields.goal.is_some() || fields.goal_file.is_some() {
            let text = match (fields.goal, fields.goal_file) {
                (Some(text), None) => text,
                (None, Some(path)) => self.read_text_file(&path)?,
                _ => unreachable!(),
            };
            let manual = fields.goal_mode != Some(NamingMode::Automatic);
            changes.insert(
                "goal".into(),
                json!({ "text": text, "short": fields.goal_short, "manual": manual }),
            );
        } else if let Some(mode) = fields.goal_mode {
            let Some(existing) = &current.goal else {
                return Err(CliError::message(
                    "cannot set --goal-mode when the Stream has no goal",
                ));
            };
            let manual = mode == NamingMode::Manual;
            changes.insert(
                "goal".into(),
                json!({
                    "text": existing.text,
                    "short": existing.short,
                    "manual": manual,
                }),
            );
        }

        Ok(EditRequest {
            changes,
            before: None,
        })
    }
}

fn load_stream_members(opened: &OpenedRecords, stream: &Stream) -> Result<Vec<Session>> {
    let listing = opened.sessions_for_host(&stream.host_id)?;
    Ok(listing
        .records
        .into_iter()
        .filter_map(|record| match record {
            Record::Session(session) if session.stream_id == stream.id => Some(session),
            _ => None,
        })
        .collect())
}
