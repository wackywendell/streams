use super::*;
use std::collections::BTreeSet;

impl WorkspaceRuntime<'_> {
    /// Requested current Sessions, not a remembered layout or native address.
    fn check_open(
        &self,
        cancelled: &AtomicBool,
        stream: &Stream,
        sessions: &[SelectedConversation<'_>],
        cwd: Option<&Path>,
    ) -> Result<()> {
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
        Ok(())
    }

    /// Open `stream` in Herdr, reusing the workspace its markers name. `observed` is a
    /// snapshot the caller already took through `base`.
    pub fn open_herdr(
        &self,
        base: &HerdrClient,
        cancelled: &AtomicBool,
        stream: &Stream,
        sessions: &[SelectedConversation<'_>],
        cwd: Option<&Path>,
        observed: Option<&HerdrNativeSnapshot>,
    ) -> Result<OpenResult> {
        self.check_open(cancelled, stream, sessions, cwd)?;
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
        use crate::integration::{
            Access, ConversationId, SessionOpenResult, WantedConversation, classify_open,
        };
        let wanted: Vec<_> = sessions
            .iter()
            .map(|selected| WantedConversation {
                conversation: ConversationId::from(selected.session),
                locator: selected.locator.map(Path::to_path_buf),
            })
            .collect();
        let requested: Vec<_> = sessions
            .iter()
            .zip(&wanted)
            .map(|(selected, wanted)| (selected.session.id.clone(), wanted.conversation.clone()))
            .collect();
        let scope = crate::integration::WorkScope {
            catalog: self.catalog_source.into(),
            host: self.host_id.into(),
        };
        let execution = snapshot.work_execution(
            client.endpoint().identity(),
            scope.clone(),
            &stream.id,
            &wanted,
        );
        let (_, mut classified) = classify_open(
            &client.opening_support(),
            &requested,
            Access::Available { value: &execution },
        )?;
        let mut outcomes = classified.clone();
        let mut launches = Vec::new();
        let mut skipped = Vec::new();
        for selected in sessions {
            let classification = classified.remove(&selected.session.id);
            if matches!(
                classification,
                Some(SessionOpenResult::AlreadyRunning { .. })
            ) {
                let live = running(snapshot, selected);
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
            match classification {
                Some(SessionOpenResult::Skipped { reason }) => {
                    skipped.push(SkippedSession {
                        session_id: selected.session.id.clone(),
                        reason,
                    });
                    continue;
                }
                None => {}
                _ => unreachable!("pure classification only reuses or skips"),
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
                Err(error) => {
                    outcomes.insert(
                        selected.session.id.clone(),
                        SessionOpenResult::Skipped {
                            reason: error.to_string(),
                        },
                    );
                    skipped.push(SkippedSession {
                        session_id: selected.session.id.clone(),
                        reason: error.to_string(),
                    });
                }
            }
        }
        let directory = match launches.first() {
            Some((_, _, directory)) => directory.clone(),
            None => self.directory(
                stream,
                sessions.first().map(|selected| selected.session),
                cwd,
            )?,
        };
        let reused = existing.is_some();
        let environment = if reused && launches.is_empty() {
            if cwd.is_some() {
                self.remember(
                    stream,
                    sessions.first().map(|selected| selected.session),
                    &directory,
                    false,
                )?;
            }
            BTreeMap::new()
        } else {
            native_environment(&stream.id, self.environment)?
        };
        for (session, _, directory) in &launches {
            self.remember(stream, Some(session), directory, cwd.is_none())?;
        }
        if !reused {
            self.remember(stream, None, &directory, cwd.is_none())?;
        }
        use crate::integration::{
            LaunchItem, PreparedOpen, SpaceDecision, SpaceEvidence, VerifiedLaunch, complete_open,
        };
        let restored = launches
            .iter()
            .map(|(session, _, _)| session.id.clone())
            .collect();
        let mut launches: Vec<_> = launches
            .into_iter()
            .map(|(session, command, directory)| {
                LaunchItem::Conversation(VerifiedLaunch::from_verified(
                    session,
                    command,
                    directory,
                    environment.clone(),
                ))
            })
            .collect();
        if sessions.is_empty() && !reused {
            launches.push(LaunchItem::Shell {
                cwd: directory.clone(),
            });
        }
        let space = match existing {
            Some(workspace) => SpaceDecision::Reuse {
                space: self.reference(&client, &workspace.id),
            },
            None => SpaceDecision::Create {
                title: stream.title.clone(),
                cwd: directory,
            },
        };
        let plan = PreparedOpen {
            stream: stream.id.clone(),
            space,
            launches,
            adopt: vec![],
            focus: true,
        };
        let submitted = client.execute_prepared_open(&plan, &scope, &environment, cancelled);
        let report = complete_open(plan, submitted.effects, outcomes);
        if let Some(error) = submitted.error {
            return Err(error.into());
        }
        let SpaceEvidence::Known { value: workspace } = report.presentation.space else {
            return Err(invalid("Herdr did not confirm a workspace"));
        };
        let mut result = self.result(&client, stream, &workspace.key, None, reused);
        result.restored = restored;
        result.unavailable = skipped.clone();
        result.skipped = skipped;
        Ok(result)
    }

    /// Open one Session, or a shell, in a new cmux workspace.
    pub fn open_cmux(
        &self,
        client: &CmuxClient,
        cancelled: &AtomicBool,
        stream: &Stream,
        sessions: &[SelectedConversation<'_>],
        cwd: Option<&Path>,
    ) -> Result<OpenResult> {
        self.check_open(cancelled, stream, sessions, cwd)?;
        if sessions.len() > 1 {
            return Err(invalid(
                "CMUX supports one selected Session; use Herdr for Stream reopen",
            ));
        }
        let requested: Vec<_> = sessions
            .iter()
            .map(|selected| {
                (
                    selected.session.id.clone(),
                    crate::integration::ConversationId::from(selected.session),
                )
            })
            .collect();
        let (_, classified) = crate::integration::classify_open(
            &client.opening_support(),
            &requested,
            crate::integration::Access::Unsupported,
        )?;
        if let Some(crate::integration::SessionOpenResult::Skipped { reason }) =
            classified.into_values().next()
        {
            return Err(invalid(reason));
        }
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
            cwd.is_none(),
        )?;
        let environment = native_environment(&stream.id, self.environment)?;
        use crate::integration::{
            LaunchItem, PreparedOpen, SpaceDecision, SpaceEvidence, VerifiedLaunch, complete_open,
        };
        let launches = match (selected, command) {
            (Some(selected), Some(command)) => {
                vec![LaunchItem::Conversation(VerifiedLaunch::from_verified(
                    selected.session,
                    command,
                    directory.clone(),
                    environment.clone(),
                ))]
            }
            _ => vec![LaunchItem::Shell {
                cwd: directory.clone(),
            }],
        };
        let plan = PreparedOpen {
            stream: stream.id.clone(),
            space: SpaceDecision::Create {
                title: stream.title.clone(),
                cwd: directory,
            },
            launches,
            adopt: vec![],
            focus: true,
        };
        let submitted = client.execute_prepared_open(&plan, &environment, cancelled);
        let report = complete_open(plan, submitted.effects, BTreeMap::new());
        if let Some(error) = submitted.error {
            return Err(error.into());
        }
        let SpaceEvidence::Known { value: workspace } = report.presentation.space else {
            return Err(invalid("CMUX did not confirm a workspace"));
        };
        let mut result = self.result(
            client,
            stream,
            &workspace.key,
            selected.map(|selected| selected.session.id.as_str()),
            false,
        );
        result.restored = selected
            .map(|selected| vec![selected.session.id.clone()])
            .unwrap_or_default();
        result.arrangement_support = Some("single-session-only");
        Ok(result)
    }
}
