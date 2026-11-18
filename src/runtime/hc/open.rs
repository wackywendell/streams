//! One-shot application of a prepared hc opening. No work selection or retries occur here.
use std::collections::BTreeMap;

use super::{
    HcOperation, HcPresentation, HcWorkExecution,
    launch::{PreparedAdoption, PreparedTerminal, StartContext},
};
use crate::{
    frontend::{FrontendId, hc::HcEndpoint},
    integration::{
        AdoptionEffect, ConversationId, Effect, NativeRef, OpenEffects, PreparedOpen,
        PresentationResult, PresentationSnapshot, PresentationState, SpaceDecision, SpaceEvidence,
        ViewerAction, WorkScope,
    },
};

pub struct OpenContext<'a> {
    pub scope: &'a WorkScope,
    pub socket: &'a HcEndpoint,
    pub environment: &'a BTreeMap<String, String>,
    pub selected: &'a [ConversationId],
    pub execution: Option<&'a HcWorkExecution>,
    pub presentation: Option<&'a HcPresentation>,
}

impl HcOperation {
    /// Apply fixed inputs once; each launch and adoption receives its own outcome.
    pub fn execute_prepared_open(
        &mut self,
        plan: &PreparedOpen,
        context: OpenContext<'_>,
    ) -> OpenEffects {
        let unavailable = |reason: &str| rejected(plan, reason);
        if context.scope.validate().is_err()
            || crate::model::check_text(&plan.stream, "Stream", false).is_err()
            || self.client.socket().socket() != context.socket.socket()
            || context.execution.is_some_and(|value| {
                value.scope() != context.scope || value.sources.endpoint != *self.endpoint()
            })
            || context.presentation.is_some_and(|value| {
                value.scope() != context.scope || value.published.endpoint != *self.endpoint()
            })
        {
            return unavailable("hc opening context does not match this operation");
        }
        let (space, requested) = match &plan.space {
            SpaceDecision::Reuse { space } => {
                if !context.presentation.is_some_and(|value| {
                    value.contains_space(space)
                        && value.stream_for_space(space).as_deref() == Some(&plan.stream)
                }) {
                    return unavailable("hc space is not associated with this work");
                }
                (Some(space.clone()), false)
            }
            SpaceDecision::Create { .. } if !plan.launches.is_empty() || !plan.adopt.is_empty() => {
                (Some(new_space(self)), true)
            }
            SpaceDecision::Unavailable { .. }
                if !plan.launches.is_empty() && plan.adopt.is_empty() =>
            {
                (Some(new_space(self)), true)
            }
            SpaceDecision::None | SpaceDecision::Unavailable { .. }
                if plan.launches.is_empty() && plan.adopt.is_empty() =>
            {
                (None, false)
            }
            _ => return unavailable("hc opening has no valid placement"),
        };
        let Some(space) = space else {
            let state = match &plan.space {
                SpaceDecision::Unavailable { .. } => PresentationState::Unavailable {
                    reason: "hc presentation is unavailable".into(),
                },
                _ => {
                    if let Some(snapshot) = context.presentation {
                        let _ =
                            self.finish_associations(&snapshot.before, &snapshot.published, None);
                    }
                    PresentationState::Incomplete {
                        reason: "no terminal can carry a new placement".into(),
                    }
                }
            };
            return OpenEffects {
                launches: vec![],
                adoptions: vec![],
                presentation: PresentationResult {
                    space: SpaceEvidence::None,
                    state,
                },
                viewer_actions: vec![],
            };
        };
        let prepared_adoptions: Vec<PreparedAdoption> = if plan.adopt.is_empty() {
            vec![]
        } else if let (Some(execution), Some(presentation)) =
            (context.execution, context.presentation)
        {
            match execution.prepare_adoptions(
                context.selected,
                presentation,
                &space,
                context.socket,
            ) {
                Ok(prepared)
                    if prepared
                        .iter()
                        .map(|item| item.terminal())
                        .eq(plan.adopt.iter()) =>
                {
                    prepared
                }
                _ => return unavailable("hc adoption does not match the retained observation"),
            }
        } else {
            return unavailable("hc adoption observation is unavailable");
        };
        let prepared_launches: Vec<_> = plan
            .launches
            .iter()
            .map(|item| {
                PreparedTerminal::prepare(
                    item,
                    StartContext {
                        scope: context.scope,
                        stream: &plan.stream,
                        space: &space,
                        socket: context.socket,
                        environment: context.environment,
                    },
                )
            })
            .collect();
        let mut launches = Vec::with_capacity(prepared_launches.len());
        let mut adoptions = Vec::with_capacity(prepared_adoptions.len());
        let mut stopped = false;
        let mut uncertain = false;
        let mut attempted = false;
        for prepared in prepared_launches {
            let effect = if stopped {
                not_attempted()
            } else if let Ok(prepared) = prepared {
                let before = self.commands.len();
                let effect = self.start_terminal(prepared);
                attempted |= self.commands.len() != before;
                if !matches!(effect, Effect::Applied { .. }) {
                    stopped = true;
                    uncertain = matches!(effect, Effect::Uncertain { .. });
                }
                effect
            } else {
                Effect::NotApplied {
                    reason: "hc launch inputs are unavailable".into(),
                }
            };
            launches.push(effect);
        }
        for prepared in prepared_adoptions {
            let effect = if stopped {
                AdoptionEffect {
                    terminal: prepared.terminal().clone(),
                    result: not_attempted(),
                }
            } else {
                let before = self.commands.len();
                let effect = self.adopt_terminal(prepared);
                attempted |= self.commands.len() != before;
                if !matches!(effect.result, Effect::Applied { .. }) {
                    stopped = true;
                    uncertain = matches!(effect.result, Effect::Uncertain { .. });
                }
                effect
            };
            adoptions.push(effect);
        }
        let presentation = if !requested {
            let state = match context.presentation {
                Some(snapshot) if !uncertain => {
                    match self.finish_associations(&snapshot.before, &snapshot.published, None) {
                        Effect::Applied { .. } => PresentationState::Complete,
                        Effect::NotApplied { reason } => PresentationState::Incomplete { reason },
                        Effect::Uncertain { reason } => PresentationState::Uncertain { reason },
                    }
                }
                _ => PresentationState::Complete,
            };
            PresentationResult {
                space: SpaceEvidence::Known {
                    value: space.clone(),
                },
                state,
            }
        } else if !attempted {
            PresentationResult {
                space: SpaceEvidence::None,
                state: PresentationState::Incomplete {
                    reason: "no placement request was submitted".into(),
                },
            }
        } else if stopped {
            PresentationResult {
                space: SpaceEvidence::Requested {
                    value: space.clone(),
                },
                state: if uncertain {
                    PresentationState::Uncertain {
                        reason: "hc placement was not confirmed".into(),
                    }
                } else {
                    PresentationState::Incomplete {
                        reason: "hc placement did not complete".into(),
                    }
                },
            }
        } else if let Some(snapshot) = context.presentation {
            self.finish_requested_space(&snapshot.before, context.scope, &plan.stream, &space)
        } else {
            PresentationResult {
                space: SpaceEvidence::Requested {
                    value: space.clone(),
                },
                state: PresentationState::Unavailable {
                    reason: "hc presentation is unavailable".into(),
                },
            }
        };
        let viewer_actions = if plan.focus
            && matches!(presentation.state, PresentationState::Complete)
            && matches!(presentation.space, SpaceEvidence::Known { .. })
        {
            vec![ViewerAction::SelectSpace { space }]
        } else {
            vec![]
        };
        OpenEffects {
            launches,
            adoptions,
            presentation,
            viewer_actions,
        }
    }
}

fn new_space(operation: &HcOperation) -> NativeRef {
    NativeRef {
        adapter: FrontendId::Hc,
        endpoint: operation.endpoint().clone(),
        key: uuid::Uuid::new_v4().to_string(),
    }
}
fn not_attempted() -> Effect<NativeRef> {
    Effect::NotApplied {
        reason: "not attempted after an earlier hc failure".into(),
    }
}
fn rejected(plan: &PreparedOpen, reason: &str) -> OpenEffects {
    let result = || Effect::NotApplied {
        reason: reason.into(),
    };
    OpenEffects {
        launches: plan.launches.iter().map(|_| result()).collect(),
        adoptions: plan
            .adopt
            .iter()
            .map(|terminal| AdoptionEffect {
                terminal: terminal.clone(),
                result: result(),
            })
            .collect(),
        presentation: PresentationResult {
            space: SpaceEvidence::None,
            state: PresentationState::Unavailable {
                reason: reason.into(),
            },
        },
        viewer_actions: vec![],
    }
}

pub fn hc_needs_attention(
    scope: &WorkScope,
    report: &crate::integration::OpenReport,
) -> Vec<String> {
    use crate::integration::{OpenReport, SessionOpenResult, ViewerAction};
    let OpenReport {
        stream,
        sessions,
        adoptions,
        initial_shell,
        presentation,
        viewer_actions,
    } = report;
    let mut attention = Vec::new();
    for (id, result) in sessions {
        match result {
            SessionOpenResult::AlreadyRunning { .. } | SessionOpenResult::Launched { .. } => {}
            SessionOpenResult::Skipped { reason } | SessionOpenResult::Uncertain { reason } => {
                attention.push(format!("Session {id}: {reason}"));
            }
        }
    }
    for adoption in adoptions {
        match &adoption.result {
            Effect::Applied { .. } => {}
            Effect::NotApplied { reason } | Effect::Uncertain { reason } => {
                attention.push(format!("adoption of {}: {reason}", adoption.terminal.key));
            }
        }
    }
    if let Some(effect) = initial_shell {
        match effect {
            Effect::Applied { .. } => {}
            Effect::NotApplied { reason } | Effect::Uncertain { reason } => {
                attention.push(format!("initial shell: {reason}"));
            }
        }
    }
    match &presentation.state {
        PresentationState::Complete => {}
        PresentationState::Unsupported => {
            attention.push("presentation is unsupported".into());
        }
        PresentationState::Incomplete { reason }
        | PresentationState::Unavailable { reason }
        | PresentationState::Uncertain { reason } => match &presentation.space {
            SpaceEvidence::None => attention.push(format!("presentation: {reason}")),
            SpaceEvidence::Requested { value } | SpaceEvidence::Known { value } => {
                let space_json = serde_json::to_string(value).unwrap_or_default();
                let scope_json = serde_json::to_string(scope).unwrap_or_default();
                attention.push(format!(
                    "presentation for space {}: {reason}; after publication run: streams integration bind-space --space-ref {} --stream {} --scope-json {}",
                    value.key,
                    shell_quote(&space_json),
                    shell_quote(stream),
                    shell_quote(&scope_json)
                ));
            }
        },
    }
    for action in viewer_actions {
        match action {
            ViewerAction::SelectSpace { space } => {
                attention.push(format!(
                    "desktop action pending: select space {}",
                    space.key
                ));
            }
            ViewerAction::FocusTerminal { terminal } => {
                attention.push(format!(
                    "desktop action pending: focus terminal {}",
                    terminal.key
                ));
            }
            ViewerAction::RenameSpace { space, .. } => {
                attention.push(format!(
                    "desktop action pending: rename space {}",
                    space.key
                ));
            }
            ViewerAction::RenameTab { tab, .. } => {
                attention.push(format!("desktop action pending: rename tab {}", tab.key));
            }
            ViewerAction::OpenUrl { url } => {
                attention.push(format!(
                    "desktop action pending: open {}",
                    url.as_url().as_str()
                ));
            }
        }
    }
    attention
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
