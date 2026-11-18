use super::*;

impl WorkspaceRuntime<'_> {
    /// Prepare to resume `selected` in the calling terminal. `running` says whether a
    /// frontend observes the conversation running; a running conversation is left in place.
    pub fn resume(
        &self,
        stream: &Stream,
        selected: SelectedConversation<'_>,
        cwd: Option<&Path>,
        running: impl FnOnce(&SelectedConversation<'_>) -> Result<bool>,
    ) -> Result<ResumeCommand> {
        self.check_stream(stream)?;
        self.check_session(stream, selected.session)?;
        if running(&selected)? {
            return Err(invalid(
                "conversation is already observed running; left in place",
            ));
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
            launch: crate::integration::VerifiedLaunch::from_verified(
                selected.session,
                command,
                directory,
                BTreeMap::from([("STREAMS_STREAM_ID".into(), stream.id.clone())]),
            ),
        })
    }
}

/// Prepared exact launch inputs. No saved native address or launch-history guard.
pub struct ResumeCommand {
    launch: crate::integration::VerifiedLaunch,
}
impl ResumeCommand {
    pub fn command(&self) -> &SessionCommand {
        self.launch.command()
    }
    pub fn cwd(&self) -> &Path {
        self.launch.cwd()
    }
    pub fn exec(self) -> Result<std::convert::Infallible> {
        use std::os::unix::process::CommandExt;
        let error = std::process::Command::new(&self.launch.command().argv()[0])
            .args(&self.launch.command().argv()[1..])
            .current_dir(self.launch.cwd())
            .envs(self.launch.environment())
            .exec();
        Err(error.into())
    }
}
