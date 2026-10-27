use std::{
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};
use streams::{
    frontend::herdr::{
        HerdrClient,
        events::{
            EventFilter, EventKind, EventPane, HerdrEvent, SubscriptionCancel, SubscriptionError,
            SubscriptionUpdate,
        },
    },
    tui::application::{PaneEvent, PaneEvents},
};

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

#[derive(Clone)]
pub(super) struct Routes {
    pub owner: EventPane,
    pub card: EventPane,
}
impl Routes {
    fn filter(&self) -> EventFilter {
        let structural = [
            EventKind::PaneMoved,
            EventKind::PaneClosed,
            EventKind::PaneExited,
            EventKind::TabMoved,
            EventKind::TabClosed,
            EventKind::WorkspaceClosed,
            EventKind::PaneAgentDetected,
            EventKind::WorkspaceMetadataUpdated,
        ];
        EventFilter {
            kinds: structural
                .into_iter()
                .chain([EventKind::PaneUpdated])
                .collect(),
            unscoped_kinds: structural.into(),
            terminal_ids: self.owner.terminal_id.iter().cloned().collect(),
            pane_ids: std::iter::once(self.owner.pane_id.clone())
                .filter(|id| !id.is_empty())
                .collect(),
            ..EventFilter::default()
        }
    }
    // Routing changes only select the next read. Closure events never close the card.
    fn relevant(&mut self, event: &HerdrEvent) -> bool {
        match event {
            HerdrEvent::PaneMoved {
                previous_pane_id,
                pane,
                ..
            } => {
                if previous_pane_id == &self.owner.pane_id {
                    self.owner = pane.clone();
                    true
                } else if previous_pane_id == &self.card.pane_id {
                    self.card = pane.clone();
                    true
                } else {
                    false
                }
            }
            HerdrEvent::PaneClosed { pane_id, .. }
            | HerdrEvent::PaneExited { pane_id, .. }
            | HerdrEvent::PaneAgentDetected { pane_id, .. } => {
                pane_id == &self.owner.pane_id || pane_id == &self.card.pane_id
            }
            HerdrEvent::TabMoved { tab_id, .. } | HerdrEvent::TabClosed { tab_id, .. } => {
                tab_id == &self.owner.tab_id || tab_id == &self.card.tab_id
            }
            HerdrEvent::WorkspaceClosed { workspace_id } => {
                workspace_id == &self.owner.workspace_id || workspace_id == &self.card.workspace_id
            }
            HerdrEvent::PaneUpdated { pane } => {
                pane.pane_id == self.owner.pane_id
                    || pane.pane_id == self.card.pane_id
                    || pane.terminal_id.as_ref().is_some_and(|terminal| {
                        self.owner.terminal_id.as_ref() == Some(terminal)
                            || self.card.terminal_id.as_ref() == Some(terminal)
                    })
            }
            HerdrEvent::WorkspaceMetadataUpdated { workspace } => {
                workspace.workspace_id == self.owner.workspace_id
            }
        }
    }
}
pub(super) struct Native {
    routes: Arc<Mutex<Option<Routes>>>,
    events: Events,
    cancel: SubscriptionCancel,
    thread: Option<JoinHandle<()>>,
}
impl Native {
    pub fn subscribe(client: &HerdrClient, routes: Routes, events: Events) -> Result<Self, String> {
        let mut subscription = client
            .socket()
            .subscribe(routes.filter())
            .map_err(|error| error.to_string())?;
        let cancel = subscription.cancellation_handle();
        let shared = Arc::new(Mutex::new(None::<Routes>));
        let routing = shared.clone();
        let notifications = events.clone();
        let thread = thread::Builder::new()
            .name("card-herdr-events".into())
            .spawn(move || {
                loop {
                    match subscription.receive() {
                        Ok(SubscriptionUpdate::Event(event)) => {
                            if routing
                                .lock()
                                .expect("card routes")
                                .as_mut()
                                .is_none_or(|routes| routes.relevant(&event))
                            {
                                notifications.push(Change::Native);
                            }
                        }
                        Ok(SubscriptionUpdate::Resubscribed) => {
                            *routing.lock().expect("card routes") = None;
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
            .map_err(|error| error.to_string())?;
        Ok(Self {
            routes: shared,
            events,
            cancel,
            thread: Some(thread),
        })
    }
    pub fn pass_client(&self, client: &HerdrClient) -> HerdrClient {
        client.with_subscription(&self.cancel)
    }
    pub fn activate(&mut self, routes: Routes) {
        let filter = routes.filter();
        self.cancel
            .set_watched_ids(filter.pane_ids, filter.tab_ids, filter.workspace_ids);
        *self.routes.lock().expect("card routes") = Some(routes);
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
