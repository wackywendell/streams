use super::*;
use std::collections::BTreeSet;

impl WorkspaceRuntime<'_> {
    /// Requested current Sessions, not a remembered layout or native address.
    pub fn open(
        &self,
        cancelled: &AtomicBool,
        stream: &Stream,
        sessions: &[SelectedConversation<'_>],
        cwd: Option<&Path>,
        observed: Option<&HerdrNativeSnapshot>,
    ) -> Result<OpenResult> {
        self.check_stream(stream)?;
        check_cancelled(cancelled)?;
        let mut seen = BTreeSet::new();
        for selected in sessions {
            self.check_session(stream, selected.session)?;
            if !seen.insert(&selected.session.id) {
                return Err(invalid("duplicate requested Session"));
            }
        }
        if cwd.is_some() && sessions.len() > 1 {
            return Err(invalid(
                "--cwd requires a single selected Session or an empty Stream",
            ));
        }
        match self.frontend()? {
            BuiltinFrontend::Herdr(_) => {
                self.open_herdr(cancelled, stream, sessions, cwd, observed)
            }
            BuiltinFrontend::Cmux(_) => {
                if observed.is_some() {
                    return Err(invalid("CMUX cannot consume a Herdr observation"));
                }
                self.open_cmux(cancelled, stream, sessions, cwd)
            }
        }
    }

    fn open_herdr(
        &self,
        cancelled: &AtomicBool,
        stream: &Stream,
        sessions: &[SelectedConversation<'_>],
        cwd: Option<&Path>,
        observed: Option<&HerdrNativeSnapshot>,
    ) -> Result<OpenResult> {
        let base = self.frontend()?.herdr().expect("selected Herdr");
        let client = if observed.is_none() {
            base.observing()?
        } else {
            base.clone()
        };
        let collected;
        let snapshot = match observed {
            Some(snapshot) => snapshot,
            None => {
                collected = HerdrNativeSnapshot::parse(&client.session_snapshot()?)?;
                &collected
            }
        };
        let tokens = stream_tokens(self.catalog_source, self.host_id, &stream.id);
        let existing = snapshot.topology().workspaces.iter().find(|workspace| {
            snapshot
                .workspace_tokens(&workspace.id)
                .is_some_and(|observed| {
                    tokens
                        .iter()
                        .all(|(key, value)| observed.get(key) == Some(value))
                })
        });
        let mut launches = Vec::new();
        let mut skipped = Vec::new();
        for selected in sessions {
            let live = running(snapshot, selected);
            if !live.is_empty() {
                let locations = live
                    .iter()
                    .map(|agent| {
                        format!("workspace {}, pane {}", agent.workspace_id, agent.pane_id,)
                    })
                    .collect::<Vec<_>>()
                    .join("; ");
                skipped.push(SkippedSession {
                    session_id: selected.session.id.clone(),
                    reason: format!("already running at {locations}; left in place"),
                });
                continue;
            }
            let prepared = (|| {
                let locator = selected
                    .locator
                    .ok_or_else(|| invalid("no retained transcript locator; refusing discovery"))?;
                let command =
                    SessionCommand::verified(selected.session, Some(locator), self.roots)?;
                let directory = self.directory(stream, Some(selected.session), cwd)?;
                Ok::<_, super::super::RuntimeError>((selected.session, command, directory))
            })();
            match prepared {
                Ok(launch) => launches.push(launch),
                Err(error) => skipped.push(SkippedSession {
                    session_id: selected.session.id.clone(),
                    reason: error.to_string(),
                }),
            }
        }
        if let Some(workspace) = existing.filter(|_| launches.is_empty()) {
            if cwd.is_some() {
                let selected = sessions.first().map(|selected| selected.session);
                self.remember(stream, selected, &self.directory(stream, selected, cwd)?)?;
            }
            check_cancelled(cancelled)?;
            client.focus(&self.reference(&workspace.id)?)?;
            let mut result = self.result(stream, &workspace.id, None, true)?;
            result.unavailable = skipped.clone();
            result.skipped = skipped;
            return Ok(result);
        }
        let directory = match launches.first() {
            Some((_, _, directory)) => directory.clone(),
            None => self.directory(
                stream,
                sessions.first().map(|selected| selected.session),
                cwd,
            )?,
        };
        let environment = native_environment(&stream.id, self.environment)?;
        for (session, _, directory) in &launches {
            self.remember(stream, Some(session), directory)?;
        }
        let mut root = None;
        let workspace = match existing {
            Some(workspace) => workspace.id.clone(),
            None => {
                self.remember(stream, None, &directory)?;
                check_cancelled(cancelled)?;
                let created = client.create_native(&CreateWorkspace {
                    title: stream.title.clone(),
                    cwd: directory,
                    environment: environment.clone(),
                })?;
                check_cancelled(cancelled)?;
                client.card_request(
                    "workspace.report_metadata",
                    serde_json::json!({
                        "workspace_id":created.workspace.reference.native_id,
                        "source":"streams", "tokens":tokens,
                    }),
                    Some("ok"),
                    true,
                )?;
                root = Some((created.tab, created.root_pane));
                created.workspace.reference.native_id
            }
        };
        let mut restored = Vec::new();
        for (session, command, directory) in launches {
            check_cancelled(cancelled)?;
            let (_, pane) = match root.take() {
                Some(root) => root,
                None => client.create_tab(
                    &self.reference(&workspace)?,
                    &directory,
                    &environment,
                    None,
                )?,
            };
            check_cancelled(cancelled)?;
            let name = format!(
                "streams-{}",
                &uuid::Uuid::new_v4().simple().to_string()[..24]
            );
            client.start_agent(
                &pane,
                &name,
                command.harness().as_ref(),
                &command.argv()[1..],
            )?;
            restored.push(session.id.clone());
        }
        check_cancelled(cancelled)?;
        client.focus(&self.reference(&workspace)?)?;
        let mut result = self.result(stream, &workspace, None, existing.is_some())?;
        result.restored = restored;
        result.unavailable = skipped.clone();
        result.skipped = skipped;
        Ok(result)
    }

    fn open_cmux(
        &self,
        cancelled: &AtomicBool,
        stream: &Stream,
        sessions: &[SelectedConversation<'_>],
        cwd: Option<&Path>,
    ) -> Result<OpenResult> {
        if sessions.len() > 1 {
            return Err(invalid(
                "CMUX supports one selected Session; use Herdr for Stream reopen",
            ));
        }
        let BuiltinFrontend::Cmux(client) = self.frontend()? else {
            unreachable!("selected CMUX")
        };
        let selected = sessions.first();
        let command = selected
            .map(|selected| {
                SessionCommand::verified(
                    selected.session,
                    Some(selected.locator.ok_or_else(|| {
                        invalid("no retained transcript locator; refusing discovery")
                    })?),
                    self.roots,
                )
            })
            .transpose()?;
        let directory = self.directory(stream, selected.map(|selected| selected.session), cwd)?;
        self.remember(
            stream,
            selected.map(|selected| selected.session),
            &directory,
        )?;
        check_cancelled(cancelled)?;
        // CMUX has no observable Stream marker. Create explicitly rather than
        // remembering an address or inferring a match from its title.
        let workspace = client.create_session(
            &CreateWorkspace {
                title: stream.title.clone(),
                cwd: directory,
                environment: native_environment(&stream.id, self.environment)?,
            },
            command
                .as_ref()
                .map(|command| command.command_text())
                .as_deref(),
        )?;
        let mut result = self.result(
            stream,
            &workspace.reference.native_id,
            selected.map(|selected| selected.session.id.as_str()),
            false,
        )?;
        result.restored = selected
            .map(|selected| vec![selected.session.id.clone()])
            .unwrap_or_default();
        result.arrangement_support = Some("single-session-only");
        Ok(result)
    }
}
