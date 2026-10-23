//! Pure owner routing. Events select a read target; only the fresh sample can adopt it.

use std::collections::{BTreeMap, BTreeSet};
use streams_temper::frontend::herdr::events::{EventFilter, EventKind, HerdrEvent};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Owner {
    pub pane: String,
    pub tab: String,
    pub workspace: String,
    pub terminal: Option<String>,
    pub refusal: Option<String>,
}

pub(super) struct Sample {
    pub pane: String,
    pub tab: String,
    pub workspace: String,
    pub terminal: String,
    pub session: Option<String>,
}

impl Owner {
    pub fn inherited(env: &BTreeMap<String, String>) -> Self {
        let get = |key| env.get(key).cloned().unwrap_or_default();
        Self {
            pane: get("HERDR_PANE_ID"),
            tab: get("HERDR_TAB_ID"),
            workspace: get("HERDR_WORKSPACE_ID"),
            terminal: None,
            refusal: None,
        }
    }

    pub fn filter(&self) -> EventFilter {
        EventFilter {
            kinds: BTreeSet::from([
                EventKind::PaneMoved,
                EventKind::TabMoved,
                EventKind::PaneClosed,
                EventKind::PaneExited,
                EventKind::TabClosed,
                EventKind::WorkspaceClosed,
                EventKind::PaneUpdated,
                EventKind::PaneAgentDetected,
                EventKind::WorkspaceMetadataUpdated,
            ]),
            pane_ids: BTreeSet::from([self.pane.clone()]),
            tab_ids: BTreeSet::from([self.tab.clone()]),
            workspace_ids: BTreeSet::from([self.workspace.clone()]),
            ..EventFilter::default()
        }
    }

    pub fn event_target(&self, event: &HerdrEvent) -> Option<Self> {
        let mut target = self.clone();
        match event {
            HerdrEvent::PaneMoved {
                previous_pane_id,
                pane,
                ..
            } => {
                // A new ID that happens to equal ours is not evidence of our move.
                if previous_pane_id != &self.pane {
                    return None;
                }
                target.pane = pane.pane_id.clone();
                target.tab = pane.tab_id.clone();
                target.workspace = pane.workspace_id.clone();
            }
            HerdrEvent::TabMoved {
                tab_id,
                workspace_id,
            } if tab_id == &self.tab => {
                target.workspace = workspace_id.clone();
            }
            HerdrEvent::TabMoved { .. } => return None,
            _ => {}
        }
        Some(target)
    }

    pub fn confirm(
        &self,
        target: &Self,
        sample: Option<&Sample>,
        session: &str,
    ) -> Result<Self, String> {
        if let Some(reason) = &self.refusal {
            return Err(reason.clone());
        }
        let sample = sample.ok_or(
            "Inherited owner pane is missing; caller origin cannot be established after a move",
        )?;
        if sample.pane != target.pane {
            return Err("Fresh owner does not match the exact caller read target".into());
        }
        if sample.session.as_deref() != Some(session) {
            return Err("Owner pane no longer has the exact Pi session".into());
        }
        if self
            .terminal
            .as_ref()
            .is_some_and(|terminal| terminal != &sample.terminal)
        {
            return Err("Owner pane terminal identity changed".into());
        }
        if sample.pane != self.pane && self.terminal.is_none() {
            return Err("Cannot adopt a moved pane without its previous terminal identity".into());
        }
        Ok(Self {
            pane: sample.pane.clone(),
            tab: sample.tab.clone(),
            workspace: sample.workspace.clone(),
            terminal: Some(sample.terminal.clone()),
            refusal: None,
        })
    }

    pub fn apply(&self, env: &mut BTreeMap<String, String>) {
        env.insert("HERDR_PANE_ID".into(), self.pane.clone());
        env.insert("HERDR_TAB_ID".into(), self.tab.clone());
        env.insert("HERDR_WORKSPACE_ID".into(), self.workspace.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use streams_temper::frontend::herdr::events::EventPane;

    fn owner() -> Owner {
        Owner {
            pane: "old".into(),
            tab: "tab".into(),
            workspace: "work".into(),
            terminal: Some("terminal".into()),
            refusal: None,
        }
    }
    fn moved(previous: &str) -> HerdrEvent {
        HerdrEvent::PaneMoved {
            previous_pane_id: previous.into(),
            previous_tab_id: "tab".into(),
            previous_workspace_id: "work".into(),
            pane: EventPane {
                pane_id: "new".into(),
                tab_id: "next-tab".into(),
                workspace_id: "next-work".into(),
                terminal_id: None,
            },
        }
    }
    fn sample(terminal: &str, session: &str) -> Sample {
        Sample {
            pane: "new".into(),
            tab: "next-tab".into(),
            workspace: "next-work".into(),
            terminal: terminal.into(),
            session: Some(session.into()),
        }
    }
    #[test]
    fn only_exact_previous_pane_can_select_a_new_id() {
        assert!(owner().event_target(&moved("unrelated")).is_none());
        let target = owner().event_target(&moved("old")).unwrap();
        assert_eq!(target.pane, "new");
        assert_eq!(
            owner()
                .confirm(&target, Some(&sample("terminal", "pi")), "pi")
                .unwrap()
                .pane,
            "new"
        );
    }
    #[test]
    fn inherited_origin_cannot_bootstrap_from_a_matching_session_elsewhere() {
        let mut inherited = owner();
        inherited.terminal = None;
        assert!(
            inherited
                .confirm(&inherited, Some(&sample("terminal", "pi")), "pi")
                .is_err()
        );
        let moved = inherited.event_target(&moved("old")).unwrap();
        assert!(
            inherited
                .confirm(&moved, Some(&sample("terminal", "pi")), "pi")
                .is_err()
        );
        assert!(
            inherited
                .confirm(&inherited, None, "pi")
                .unwrap_err()
                .contains("origin")
        );
    }

    #[test]
    fn matching_move_cannot_replace_terminal_or_agent_session() {
        let before = owner();
        let target = before.event_target(&moved("old")).unwrap();
        assert!(
            before
                .confirm(&target, Some(&sample("reused", "pi")), "pi")
                .is_err()
        );
        assert!(
            before
                .confirm(&target, Some(&sample("terminal", "other")), "pi")
                .is_err()
        );
        assert_eq!(before, owner());
        let mut refused = before;
        refused.refusal = Some("origin refused".into());
        assert_eq!(
            refused
                .confirm(&target, Some(&sample("terminal", "pi")), "pi")
                .unwrap_err(),
            "origin refused"
        );
    }
}
