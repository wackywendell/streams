use super::*;
use args::StreamFields;
use streams::{
    model::NamingMode,
    operations::{edits::StreamEdit, patches::Channel},
};

impl App {
    pub(super) fn create_stream(
        &self,
        title: String,
        fields: StreamFields,
        cwd: Option<PathBuf>,
    ) -> Result<Output> {
        let opened = application::open_records(&self.config)?;
        let stream = self.proposed_stream(opened.host().as_str(), title, &fields)?;
        let saved = application::create_record(opened.records(), &Record::Stream(stream))?;
        let Record::Stream(stream) = saved.current.clone() else {
            unreachable!("Stream creation target")
        };
        let mut output = catalog::finish_created(saved);
        if !output.accepted_save {
            return Ok(output);
        }
        let completion = (|| -> Result<()> {
            let directory = self.path(cwd.as_deref().unwrap_or(&self.cwd))?;
            let locations = opened.locations()?;
            locations.remember(
                streams::runtime::BindingKey::Stream(&stream.id),
                &directory,
                false,
                None,
                None,
            )?;
            Ok(())
        })();
        if let Err(error) = completion {
            output.code = 2;
            output.value["completion_error"] = json!({
                "message": error.to_string(),
                "detail": "Stream saved; select a valid directory"
            });
        }
        Ok(output)
    }

    fn proposed_stream(&self, host: &str, title: String, intent: &StreamFields) -> Result<Stream> {
        let mut stream = Stream::new(uuid::Uuid::new_v4().to_string(), title, None, host)?;
        stream.title_mode = TitleMode::Manual;
        let patch = fields::stream(&stream, intent, &mut |path| self.read_text(path))?;
        let edit = StreamEdit::new(
            &stream,
            patch,
            Actor::User,
            Channel::Cli,
            None,
            NamingMode::default(),
        )?;
        Ok(edit.after().clone())
    }
}
