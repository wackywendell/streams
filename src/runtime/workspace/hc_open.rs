use super::*;
use crate::{
    frontend::hc::{HcEndpoint, processes::OwnerProcesses},
    integration::{
        Access, ConversationId, ExecutionSnapshot, LaunchItem, OpenReport, PreparedOpen,
        PresentationSnapshot, SessionOpenResult, SpaceDecision, VerifiedLaunch, WorkScope,
        classify_open, complete_open,
    },
    runtime::hc::{HcOperation, HcPresentation, HcWorkExecution, open::OpenContext},
};
use std::collections::BTreeSet;

impl WorkspaceRuntime<'_> {
    /// Observe hc once, then prepare, execute and report one opening of `sessions`.
    /// `presentation` reuses an observation the caller already holds.
    pub fn open_hc(
        &self,
        operation: &mut HcOperation,
        socket: &HcEndpoint,
        stream: &Stream,
        sessions: &[Session],
        cwd: Option<&Path>,
        presentation: Option<HcPresentation>,
    ) -> Result<OpenReport> {
        let scope = WorkScope {
            catalog: self.catalog_source.into(),
            host: self.host_id.into(),
        };
        let execution = if operation.supports("terminal.list") {
            OwnerProcesses::new().ok().and_then(|observer| {
                operation
                    .registered_sources(&observer)
                    .ok()?
                    .for_scope(scope.clone())
                    .ok()
            })
        } else {
            None
        };
        let presentation = match presentation {
            Some(presentation) => Some(presentation),
            None if operation.supports("doc.get") => operation.presentation(&scope, None).ok(),
            None => None,
        };
        let mut locator_failures = BTreeSet::new();
        let locators: Vec<_> = sessions
            .iter()
            .map(
                |session| match self.locations.transcript_file(&stream.id, &session.id) {
                    Ok(path) => path,
                    Err(_) => {
                        locator_failures.insert(session.id.clone());
                        None
                    }
                },
            )
            .collect();
        let selected: Vec<_> = sessions
            .iter()
            .zip(&locators)
            .map(|(session, locator)| SelectedConversation {
                session,
                locator: locator.as_deref(),
            })
            .collect();
        let (plan, mut outcomes) = self.prepare_hc_open(
            &AtomicBool::new(false),
            stream,
            &selected,
            cwd,
            available(execution.as_ref(), "hc Pi presence is unavailable"),
            available(presentation.as_ref(), "hc presentation is unavailable"),
        )?;
        for id in locator_failures {
            if matches!(outcomes.get(&id), Some(SessionOpenResult::Skipped { .. })) {
                outcomes.insert(
                    id,
                    SessionOpenResult::Skipped {
                        reason: "saved Session locator is unavailable".into(),
                    },
                );
            }
        }
        let conversations: Vec<_> = sessions.iter().map(ConversationId::from).collect();
        let effects = operation.execute_prepared_open(
            &plan,
            OpenContext {
                scope: &scope,
                socket,
                environment: self.environment,
                selected: &conversations,
                execution: execution.as_ref(),
                presentation: presentation.as_ref(),
            },
        );
        Ok(complete_open(plan, effects, outcomes))
    }

    /// Build an hc opening from one scoped observation. This performs no native or catalog writes.
    pub fn prepare_hc_open(
        &self,
        cancelled: &AtomicBool,
        stream: &Stream,
        sessions: &[SelectedConversation<'_>],
        cwd: Option<&Path>,
        execution: Access<&HcWorkExecution>,
        presentation: Access<&HcPresentation>,
    ) -> Result<(PreparedOpen, BTreeMap<String, SessionOpenResult>)> {
        self.check_stream(stream)?;
        check_cancelled(cancelled)?;
        let scope = WorkScope {
            catalog: self.catalog_source.into(),
            host: self.host_id.into(),
        };
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
        if matches!(&execution, Access::Available { value } if value.scope() != &scope)
            || matches!(&presentation, Access::Available { value } if value.scope() != &scope)
        {
            return Err(invalid("hc observation belongs to another work scope"));
        }
        let requested: Vec<_> = sessions
            .iter()
            .map(|selected| {
                (
                    selected.session.id.clone(),
                    ConversationId::from(selected.session),
                )
            })
            .collect();
        let facts = match &execution {
            Access::Available { value } => Access::Available {
                value: *value as &dyn ExecutionSnapshot,
            },
            Access::Unsupported => Access::Unsupported,
            Access::Unavailable { .. } => Access::Unavailable {
                reason: "hc Pi presence is unavailable".into(),
            },
        };
        let (candidates, mut outcomes) =
            classify_open(&HcOperation::opening_support(), &requested, facts)?;
        let candidates: BTreeSet<_> = candidates.into_iter().collect();
        let mut launches = Vec::new();
        let environment = if candidates.is_empty() {
            BTreeMap::new()
        } else {
            native_environment(&stream.id, self.environment)?
        };
        for selected in sessions {
            if !candidates.contains(&selected.session.id) {
                continue;
            }
            let prepared = (|| {
                let locator = selected
                    .locator
                    .ok_or_else(|| invalid("no retained transcript locator; refusing discovery"))?;
                let command =
                    SessionCommand::verified(selected.session, Some(locator), self.roots)?;
                let directory = self.directory(stream, Some(selected.session), cwd)?;
                Ok::<_, super::super::RuntimeError>(VerifiedLaunch::from_verified(
                    selected.session,
                    command,
                    directory,
                    environment.clone(),
                ))
            })();
            match prepared {
                Ok(launch) => launches.push(LaunchItem::Conversation(launch)),
                Err(_) => {
                    let reason = if selected.locator.is_none() {
                        "no retained transcript locator; refusing discovery"
                    } else {
                        "saved conversation or Session directory is unavailable"
                    };
                    outcomes.insert(
                        selected.session.id.clone(),
                        SessionOpenResult::Skipped {
                            reason: reason.into(),
                        },
                    );
                }
            }
        }
        let existing = match &presentation {
            Access::Available { value } => value.associated_spaces(&stream.id).into_iter().next(),
            _ => None,
        };
        let adopt = match (&execution, &presentation) {
            (
                Access::Available { value: execution },
                Access::Available {
                    value: presentation,
                },
            ) => {
                let running: Vec<_> = requested
                    .iter()
                    .filter_map(|(id, conversation)| {
                        matches!(
                            outcomes.get(id),
                            Some(SessionOpenResult::AlreadyRunning { .. })
                        )
                        .then_some(conversation.clone())
                    })
                    .collect();
                execution.unplaced_selected(&running, presentation)?
            }
            _ => vec![],
        };
        let no_work = match &execution {
            Access::Available { value } => {
                matches!(value.stream_terminals(&stream.id), Access::Available {value} if value.is_empty())
            }
            _ => false,
        };
        if sessions.is_empty()
            && existing.is_none()
            && no_work
            && matches!(presentation, Access::Available { .. })
        {
            launches.push(LaunchItem::Shell {
                cwd: self.directory(stream, None, cwd)?,
            });
        }
        let directory = match launches.first() {
            Some(LaunchItem::Conversation(launch)) => Some(launch.cwd().to_path_buf()),
            Some(LaunchItem::Shell { cwd }) => Some(cwd.clone()),
            None if existing.is_some() || !adopt.is_empty() || cwd.is_some() => {
                Some(self.directory(
                    stream,
                    sessions.first().map(|selected| selected.session),
                    cwd,
                )?)
            }
            None => None,
        };
        let reused = existing.is_some();
        if reused
            && launches.is_empty()
            && cwd.is_some()
            && let Some(directory) = &directory
        {
            self.remember(
                stream,
                sessions.first().map(|selected| selected.session),
                directory,
                false,
            )?;
        }
        for launch in &launches {
            if let LaunchItem::Conversation(launch) = launch
                && let Some(selected) = sessions
                    .iter()
                    .find(|selected| selected.session.id == launch.session())
            {
                self.remember(stream, Some(selected.session), launch.cwd(), cwd.is_none())?;
            }
        }
        if !reused && let Some(directory) = &directory {
            self.remember(stream, None, directory, cwd.is_none())?;
        }
        let space = if let Some(space) = existing {
            SpaceDecision::Reuse { space }
        } else if matches!(presentation, Access::Unavailable { .. }) {
            SpaceDecision::Unavailable {
                reason: "hc presentation is unavailable".into(),
            }
        } else if matches!(presentation, Access::Unsupported) {
            SpaceDecision::Unavailable {
                reason: "hc presentation is unsupported".into(),
            }
        } else if launches.is_empty() && adopt.is_empty() {
            SpaceDecision::None
        } else {
            SpaceDecision::Create {
                title: stream.title.clone(),
                cwd: directory.expect("create requires a launch or adoption directory"),
            }
        };
        Ok((
            PreparedOpen {
                stream: stream.id.clone(),
                space,
                launches,
                adopt,
                focus: true,
            },
            outcomes,
        ))
    }
}

fn available<T>(value: Option<T>, reason: &str) -> Access<T> {
    match value {
        Some(value) => Access::Available { value },
        None => Access::Unavailable {
            reason: reason.into(),
        },
    }
}
