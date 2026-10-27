use super::*;

impl WorkspaceRuntime<'_> {
    pub fn resume(
        &self,
        stream: &Stream,
        selected: SelectedConversation<'_>,
        cwd: Option<&Path>,
        observed: Option<&HerdrNativeSnapshot>,
    ) -> Result<ResumeCommand> {
        self.check_stream(stream)?;
        self.check_session(stream, selected.session)?;
        if let Some(client) = self.frontend.and_then(BuiltinFrontend::herdr) {
            let collected;
            let snapshot = match observed {
                Some(snapshot) => snapshot,
                None => {
                    collected = HerdrNativeSnapshot::parse(&client.session_snapshot()?)?;
                    &collected
                }
            };
            if !running(snapshot, &selected).is_empty() {
                return Err(invalid(
                    "conversation is already observed running; left in place",
                ));
            }
        } else if observed.is_some() {
            return Err(invalid("resume observation has no matching Herdr frontend"));
        }
        let locator = selected.locator.ok_or_else(|| {
            invalid("selected conversation has no retained locator; refusing transcript discovery")
        })?;
        let command = SessionCommand::verified(selected.session, Some(locator), self.roots)?;
        let directory = self.directory(stream, Some(selected.session), cwd)?;
        if cwd.is_some() {
            self.remember(stream, Some(selected.session), &directory, false)?;
        }
        Ok(ResumeCommand {
            stream_id: stream.id.clone(),
            command,
            cwd: directory,
        })
    }
}

/// Prepared exact launch inputs. No saved native address or launch-history guard.
pub struct ResumeCommand {
    stream_id: String,
    command: SessionCommand,
    cwd: PathBuf,
}
impl ResumeCommand {
    pub fn command(&self) -> &SessionCommand {
        &self.command
    }
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    pub fn exec(self) -> Result<std::convert::Infallible> {
        use std::os::unix::process::CommandExt;
        let error = std::process::Command::new(&self.command.argv()[0])
            .args(&self.command.argv()[1..])
            .current_dir(&self.cwd)
            .env("STREAMS_STREAM_ID", &self.stream_id)
            .exec();
        Err(error.into())
    }
}
