use super::*;
use crate::{
    integration::{
        Access, ConversationId, ExecutionSnapshot, LaunchItem, PreparedOpen, PresentationSnapshot,
        SessionOpenResult, SpaceDecision, VerifiedLaunch, WorkScope, classify_open,
    },
    runtime::hc::{HcOperation, HcPresentation, HcWorkExecution},
};
use std::collections::BTreeSet;

impl WorkspaceRuntime<'_> {
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
            let directory = match launches.first() {
                Some(LaunchItem::Conversation(launch)) => launch.cwd().to_path_buf(),
                Some(LaunchItem::Shell { cwd }) => cwd.clone(),
                None => self.directory(
                    stream,
                    sessions.first().map(|selected| selected.session),
                    cwd,
                )?,
            };
            SpaceDecision::Create {
                title: stream.title.clone(),
                cwd: directory,
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
