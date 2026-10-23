use std::{
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};
use streams_temper::{
    frontend::herdr::{
        HerdrClient,
        events::{
            EventFilter, EventKind, EventPane, HerdrEvent, SubscriptionCancel, SubscriptionError,
            SubscriptionUpdate,
        },
    },
    tui::application::{PaneEvent, PaneEvents},
};

#[cfg(test)]
#[path = "events_tests.rs"]
mod subscription_tests;

pub(super) enum Change {
    Native,
    NativeRecovered,
    File(u8, notify::Result<notify::Event>),
}
#[derive(Clone)]
pub(super) struct Events {
    pending: Arc<Mutex<Option<Vec<Change>>>>,
    ui: PaneEvents,
}
impl Events {
    pub fn new(ui: PaneEvents) -> Self {
        Self {
            pending: Arc::new(Mutex::new(Some(Vec::new()))),
            ui,
        }
    }
    pub fn push(&self, change: Change) {
        let mut queue = self.pending.lock().expect("card event queue");
        let Some(pending) = queue.as_mut() else {
            return;
        };
        let wake = pending.is_empty();
        pending.push(change);
        drop(queue);
        if wake {
            self.ui.send(PaneEvent::Changed);
        }
    }
    pub fn drain(&self) -> Vec<Change> {
        self.pending
            .lock()
            .expect("card event queue")
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }
    pub fn stop(&self) {
        self.pending.lock().expect("card event queue").take();
    }
    pub fn closed(&self) {
        self.ui.send(PaneEvent::Exit);
    }
    pub fn failed(&self, error: impl ToString) {
        self.ui.send(PaneEvent::Failed(error.to_string()));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Routes {
    pub owner: EventPane,
    pub card: EventPane,
}
#[derive(Debug, PartialEq, Eq)]
enum Route {
    Ignore,
    Read,
    Close,
}
impl Routes {
    fn filter(&self) -> EventFilter {
        EventFilter {
            kinds: [
                EventKind::PaneMoved,
                EventKind::TabMoved,
                EventKind::PaneClosed,
                EventKind::PaneExited,
                EventKind::TabClosed,
                EventKind::WorkspaceClosed,
                EventKind::PaneUpdated,
                EventKind::PaneAgentDetected,
                EventKind::WorkspaceMetadataUpdated,
            ]
            .into(),
            // Structural events are low-volume and lack stable terminal IDs.
            // Route them locally, including moves that precede the first snapshot.
            unscoped_kinds: [
                EventKind::PaneMoved,
                EventKind::PaneClosed,
                EventKind::PaneExited,
                EventKind::TabMoved,
                EventKind::TabClosed,
                EventKind::WorkspaceClosed,
                EventKind::PaneAgentDetected,
                EventKind::WorkspaceMetadataUpdated,
            ]
            .into(),
            terminal_ids: self.owner.terminal_id.iter().cloned().collect(),
            ..EventFilter::default()
        }
    }
    // Address changes only route future invalidations. The reader separately
    // confirms both terminals, the Pi identity and the exact launch lifetime.
    fn route(&mut self, event: &HerdrEvent) -> Route {
        match event {
            HerdrEvent::PaneMoved {
                previous_pane_id,
                pane,
                ..
            } => {
                if previous_pane_id == &self.owner.pane_id
                    && pane.terminal_id == self.owner.terminal_id
                {
                    self.owner = pane.clone();
                } else if previous_pane_id == &self.card.pane_id
                    && self
                        .card
                        .terminal_id
                        .as_ref()
                        .is_none_or(|terminal| pane.terminal_id.as_ref() == Some(terminal))
                {
                    self.card = pane.clone();
                } else {
                    return Route::Ignore;
                }
                Route::Read
            }
            HerdrEvent::TabMoved {
                tab_id,
                workspace_id,
            } if tab_id == &self.owner.tab_id => {
                self.owner.workspace_id = workspace_id.clone();
                Route::Read
            }
            HerdrEvent::PaneClosed { pane_id, .. } | HerdrEvent::PaneExited { pane_id, .. }
                if pane_id == &self.owner.pane_id =>
            {
                Route::Close
            }
            HerdrEvent::TabClosed { tab_id, .. } if tab_id == &self.owner.tab_id => Route::Close,
            HerdrEvent::WorkspaceClosed { workspace_id }
                if workspace_id == &self.owner.workspace_id =>
            {
                Route::Close
            }
            HerdrEvent::PaneUpdated { pane } if pane.terminal_id == self.owner.terminal_id => {
                Route::Read
            }
            HerdrEvent::PaneAgentDetected { pane_id, .. } if pane_id == &self.owner.pane_id => {
                Route::Read
            }
            HerdrEvent::WorkspaceMetadataUpdated { workspace }
                if workspace.workspace_id == self.owner.workspace_id =>
            {
                Route::Read
            }
            _ => Route::Ignore,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Bootstrap {
    Initial,
    Recovery,
}
enum Routing {
    Buffered {
        kind: Bootstrap,
        routes: Routes,
        pending: Vec<HerdrEvent>,
    },
    Active(Routes),
}
impl Routing {
    fn routes(&self) -> &Routes {
        match self {
            Self::Buffered { routes, .. } | Self::Active(routes) => routes,
        }
    }
    fn event(&mut self, event: HerdrEvent) -> Route {
        match self {
            Self::Active(routes) => routes.route(&event),
            Self::Buffered {
                routes, pending, ..
            } => {
                let action = routes.route(&event);
                // Pane IDs are never reused. Group closures may refer to a
                // location the owner left before subscription, so defer those.
                let exit = action == Route::Close
                    && matches!(
                        event,
                        HerdrEvent::PaneClosed { .. } | HerdrEvent::PaneExited { .. }
                    );
                // Fresh snapshot IDs may be unknown to the pre-read routes.
                // Keep every event for replay; only the static filter runs early.
                pending.push(event);
                if exit { Route::Close } else { Route::Ignore }
            }
        }
    }
    fn activate(&mut self, kind: Bootstrap, routes: Routes) -> Route {
        let Self::Buffered { kind: expected, .. } = self else {
            return Route::Ignore;
        };
        if *expected != kind {
            return Route::Ignore;
        }
        let Self::Buffered { pending, .. } = std::mem::replace(self, Self::Active(routes)) else {
            unreachable!()
        };
        let mut action = Route::Ignore;
        for event in pending {
            match self.event(event) {
                Route::Close => return Route::Close,
                Route::Read => action = Route::Read,
                Route::Ignore => {}
            }
        }
        action
    }
}
pub(super) struct Native {
    routes: Arc<Mutex<Routing>>,
    events: Events,
    cancel: SubscriptionCancel,
    thread: Option<JoinHandle<()>>,
}
impl Native {
    pub fn subscribe(client: &HerdrClient, routes: Routes, events: Events) -> Result<Self, String> {
        let mut subscription = client
            .socket()
            .subscribe(routes.filter())
            .map_err(|e| e.to_string())?;
        let cancel = subscription.cancellation_handle();
        let routes = Arc::new(Mutex::new(Routing::Buffered {
            kind: Bootstrap::Initial,
            routes,
            pending: Vec::new(),
        }));
        let routing = routes.clone();
        let notifications = events.clone();
        let thread = thread::Builder::new()
            .name("card-herdr-events".into())
            .spawn(move || {
                loop {
                    match subscription.receive() {
                        Ok(SubscriptionUpdate::Event(event)) => {
                            let action = routing.lock().expect("card routes").event(event);
                            match action {
                                Route::Read => notifications.push(Change::Native),
                                Route::Close => {
                                    notifications.closed();
                                    break;
                                }
                                Route::Ignore => {}
                            }
                        }
                        Ok(SubscriptionUpdate::Resubscribed) => {
                            let mut state = routing.lock().expect("card routes");
                            *state = Routing::Buffered {
                                kind: Bootstrap::Recovery,
                                routes: state.routes().clone(),
                                pending: Vec::new(),
                            };
                            drop(state);
                            notifications.push(Change::NativeRecovered);
                        }
                        Err(SubscriptionError::Stopped) => break,
                        Err(error) => {
                            notifications.failed(error);
                            break;
                        }
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            routes,
            events,
            cancel,
            thread: Some(thread),
        })
    }
    /// Replay buffered changes after the matching complete observation. Initial
    /// completion cannot release a recovery that started during that read.
    pub fn activate(&mut self, kind: Bootstrap, routes: Routes) {
        let action = self
            .routes
            .lock()
            .expect("card routes")
            .activate(kind, routes);
        match action {
            Route::Close => self.events.closed(),
            Route::Read => self.events.push(Change::Native),
            Route::Ignore => {}
        }
    }
    pub fn stop(&mut self) {
        self.events.stop();
        let _ = self.cancel.cancel();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    fn pane(id: &str) -> EventPane {
        EventPane {
            pane_id: id.into(),
            tab_id: "tab".into(),
            workspace_id: "workspace".into(),
            terminal_id: Some(
                if id.starts_with("card") {
                    "card-terminal"
                } else {
                    "owner-terminal"
                }
                .into(),
            ),
        }
    }
    fn routes(owner: &str) -> Routes {
        Routes {
            owner: pane(owner),
            card: pane("card"),
        }
    }
    #[test]
    fn recovery_replays_moves_for_ids_not_known_before_the_gap() {
        let mut state = Routing::Buffered {
            kind: Bootstrap::Recovery,
            routes: routes("before-gap"),
            pending: Vec::new(),
        };
        assert_eq!(
            state.event(HerdrEvent::PaneMoved {
                previous_pane_id: "in-gap".into(),
                previous_tab_id: "tab".into(),
                previous_workspace_id: "workspace".into(),
                pane: pane("after-snapshot"),
            }),
            Route::Ignore
        );
        assert_eq!(
            state.activate(Bootstrap::Initial, routes("in-gap")),
            Route::Ignore
        );
        assert!(matches!(
            state,
            Routing::Buffered {
                kind: Bootstrap::Recovery,
                ..
            }
        ));
        assert_eq!(
            state.activate(Bootstrap::Recovery, routes("in-gap")),
            Route::Read
        );
        assert_eq!(state.routes().owner.pane_id, "after-snapshot");
    }
    #[test]
    fn recovery_replays_owner_closure_for_an_id_discovered_in_the_snapshot() {
        let mut state = Routing::Buffered {
            kind: Bootstrap::Recovery,
            routes: routes("before-gap"),
            pending: Vec::new(),
        };
        assert_eq!(
            state.event(HerdrEvent::PaneClosed {
                pane_id: "in-gap".into(),
                workspace_id: "workspace".into(),
            }),
            Route::Ignore
        );
        assert_eq!(
            state.activate(Bootstrap::Recovery, routes("in-gap")),
            Route::Close
        );
    }
    #[test]
    fn replay_ignores_old_group_closure_without_creating_an_observation() {
        let mut old = routes("owner");
        old.owner.tab_id = "old-tab".into();
        let mut state = Routing::Buffered {
            kind: Bootstrap::Initial,
            routes: old,
            pending: Vec::new(),
        };
        assert_eq!(
            state.event(HerdrEvent::TabClosed {
                tab_id: "old-tab".into(),
                workspace_id: "workspace".into()
            }),
            Route::Ignore
        );
        assert_eq!(
            state.activate(Bootstrap::Initial, routes("owner")),
            Route::Ignore
        );
    }
    #[test]
    fn routes_follow_chained_owner_and_card_moves_without_following_unrelated_panes() {
        let mut routes = Routes {
            owner: pane("owner"),
            card: pane("card"),
        };
        let moved = |previous: &str, next: &str| HerdrEvent::PaneMoved {
            previous_pane_id: previous.into(),
            previous_tab_id: "tab".into(),
            previous_workspace_id: "workspace".into(),
            pane: pane(next),
        };
        assert_eq!(routes.route(&moved("other", "unrelated")), Route::Ignore);
        assert_eq!(routes.route(&moved("owner", "next")), Route::Read);
        assert_eq!(routes.route(&moved("next", "last")), Route::Read);
        assert_eq!(routes.route(&moved("card", "card-next")), Route::Read);
        assert_eq!(routes.owner.pane_id, "last");
        assert_eq!(routes.card.pane_id, "card-next");
        assert_eq!(
            routes.filter().terminal_ids,
            BTreeSet::from(["owner-terminal".into()])
        );
        assert_eq!(
            routes.route(&HerdrEvent::PaneUpdated {
                pane: pane("card-next")
            }),
            Route::Ignore
        );
        assert_eq!(
            routes.route(&HerdrEvent::PaneClosed {
                pane_id: "owner".into(),
                workspace_id: "workspace".into()
            }),
            Route::Ignore
        );
        assert_eq!(
            routes.route(&HerdrEvent::PaneClosed {
                pane_id: "last".into(),
                workspace_id: "workspace".into()
            }),
            Route::Close
        );
    }
}
