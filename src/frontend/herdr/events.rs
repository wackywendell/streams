//! Required structural subscriptions. Events invalidate observations; they are
//! never authority to apply native or catalog effects.

#[cfg(test)]
mod tests;

use super::socket::{SocketClient, read_frame};
use serde::Deserialize;
use serde_json::{json, value::RawValue};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::BufReader;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, strum::Display, strum::EnumString)]
pub enum EventKind {
    #[strum(serialize = "pane.moved")]
    PaneMoved,
    #[strum(serialize = "tab.moved")]
    TabMoved,
    #[strum(serialize = "pane.closed")]
    PaneClosed,
    #[strum(serialize = "pane.exited")]
    PaneExited,
    #[strum(serialize = "tab.closed")]
    TabClosed,
    #[strum(serialize = "workspace.closed")]
    WorkspaceClosed,
    #[strum(serialize = "pane.updated")]
    PaneUpdated,
    #[strum(serialize = "pane.agent_detected")]
    PaneAgentDetected,
    #[strum(serialize = "workspace.metadata_updated")]
    WorkspaceMetadataUpdated,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EventPane {
    #[serde(deserialize_with = "deserialize_id")]
    pub pane_id: String,
    #[serde(deserialize_with = "deserialize_id")]
    pub tab_id: String,
    #[serde(deserialize_with = "deserialize_id")]
    pub workspace_id: String,
    #[serde(default)]
    pub terminal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct EventWorkspace {
    #[serde(deserialize_with = "deserialize_id")]
    pub workspace_id: String,
}

/// Only fields used for invalidation are decoded. Large unused payloads (pane
/// output, workspace lists, tokens) are skipped by serde, even on matching events.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HerdrEvent {
    PaneMoved {
        #[serde(deserialize_with = "deserialize_id")]
        previous_pane_id: String,
        #[serde(deserialize_with = "deserialize_id")]
        previous_tab_id: String,
        #[serde(deserialize_with = "deserialize_id")]
        previous_workspace_id: String,
        pane: EventPane,
    },
    TabMoved {
        #[serde(deserialize_with = "deserialize_id")]
        tab_id: String,
        #[serde(deserialize_with = "deserialize_id")]
        workspace_id: String,
    },
    PaneClosed {
        #[serde(deserialize_with = "deserialize_id")]
        pane_id: String,
        #[serde(deserialize_with = "deserialize_id")]
        workspace_id: String,
    },
    PaneExited {
        #[serde(deserialize_with = "deserialize_id")]
        pane_id: String,
        #[serde(deserialize_with = "deserialize_id")]
        workspace_id: String,
    },
    TabClosed {
        #[serde(deserialize_with = "deserialize_id")]
        tab_id: String,
        #[serde(deserialize_with = "deserialize_id")]
        workspace_id: String,
    },
    WorkspaceClosed {
        #[serde(deserialize_with = "deserialize_id")]
        workspace_id: String,
    },
    PaneUpdated {
        pane: EventPane,
    },
    PaneAgentDetected {
        #[serde(deserialize_with = "deserialize_id")]
        pane_id: String,
        #[serde(deserialize_with = "deserialize_id")]
        workspace_id: String,
    },
    WorkspaceMetadataUpdated {
        workspace: EventWorkspace,
    },
}

impl From<&HerdrEvent> for EventKind {
    fn from(event: &HerdrEvent) -> Self {
        match event {
            HerdrEvent::PaneMoved { .. } => Self::PaneMoved,
            HerdrEvent::TabMoved { .. } => Self::TabMoved,
            HerdrEvent::PaneClosed { .. } => Self::PaneClosed,
            HerdrEvent::PaneExited { .. } => Self::PaneExited,
            HerdrEvent::TabClosed { .. } => Self::TabClosed,
            HerdrEvent::WorkspaceClosed { .. } => Self::WorkspaceClosed,
            HerdrEvent::PaneUpdated { .. } => Self::PaneUpdated,
            HerdrEvent::PaneAgentDetected { .. } => Self::PaneAgentDetected,
            HerdrEvent::WorkspaceMetadataUpdated { .. } => Self::WorkspaceMetadataUpdated,
        }
    }
}

/// ID sets are deliberately separate: subscribing to a workspace's metadata
/// must not cause every pane update in that workspace to pass the first filter.
#[derive(Debug, Clone, Default)]
pub struct EventFilter {
    pub kinds: BTreeSet<EventKind>,
    pub unscoped_kinds: BTreeSet<EventKind>,
    pub terminal_ids: BTreeSet<String>,
    pub pane_ids: BTreeSet<String>,
    pub tab_ids: BTreeSet<String>,
    pub workspace_ids: BTreeSet<String>,
}

#[derive(Deserialize)]
struct Envelope<'a> {
    #[serde(borrow)]
    event: Cow<'a, str>,
    #[serde(borrow)]
    data: &'a RawValue,
}

#[derive(Default, Deserialize)]
struct Ids<'a> {
    #[serde(borrow)]
    terminal_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    pane_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    workspace_id: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
struct Header<'a> {
    #[serde(rename = "type")]
    #[serde(borrow)]
    kind: Cow<'a, str>,
    #[serde(borrow)]
    pane_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    tab_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    workspace_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    previous_pane_id: Option<Cow<'a, str>>,
    #[serde(borrow)]
    pane: Option<Ids<'a>>,
    #[serde(borrow)]
    workspace: Option<Ids<'a>>,
}

impl EventFilter {
    fn decode(&self, bytes: &[u8]) -> Result<Option<HerdrEvent>, SubscriptionError> {
        let envelope: Envelope<'_> = serde_json::from_slice(bytes).map_err(invalid_event)?;
        // Wire tags are not the dotted subscribe topics. Borrow unescaped tags.
        let kind = match envelope.event.as_ref() {
            "pane_moved" => Some(EventKind::PaneMoved),
            "tab_moved" => Some(EventKind::TabMoved),
            "pane_closed" => Some(EventKind::PaneClosed),
            "pane_exited" => Some(EventKind::PaneExited),
            "tab_closed" => Some(EventKind::TabClosed),
            "workspace_closed" => Some(EventKind::WorkspaceClosed),
            "pane_updated" => Some(EventKind::PaneUpdated),
            "pane_agent_detected" => Some(EventKind::PaneAgentDetected),
            "workspace_metadata_updated" => Some(EventKind::WorkspaceMetadataUpdated),
            _ => None,
        };
        let Some(kind) = kind.filter(|kind| self.kinds.contains(kind)) else {
            return Ok(None);
        };
        let header: Header<'_> =
            serde_json::from_str(envelope.data.get()).map_err(invalid_event)?;
        if header.kind != envelope.event {
            return Err(SubscriptionError::InvalidEvent(
                "Herdr event/data type mismatch".into(),
            ));
        }
        // Missing routing IDs are malformed, not evidence of an unrelated event.
        fn required(id: Option<&str>) -> Result<&str, SubscriptionError> {
            id.ok_or_else(|| SubscriptionError::InvalidEvent("missing event routing ID".into()))
                .and_then(routing_id)
        }
        let matched = self.unscoped_kinds.contains(&kind)
            || match kind {
                EventKind::PaneMoved => {
                    let previous = required(header.previous_pane_id.as_deref())?;
                    let current =
                        required(header.pane.as_ref().and_then(|p| p.pane_id.as_deref()))?;
                    self.pane_ids.contains(previous) || self.pane_ids.contains(current)
                }
                EventKind::PaneUpdated => {
                    self.pane_ids.contains(required(
                        header.pane.as_ref().and_then(|p| p.pane_id.as_deref()),
                    )?) || header
                        .pane
                        .as_ref()
                        .and_then(|p| p.terminal_id.as_deref())
                        .is_some_and(|terminal| self.terminal_ids.contains(terminal))
                }
                EventKind::PaneClosed | EventKind::PaneExited | EventKind::PaneAgentDetected => {
                    self.pane_ids.contains(required(header.pane_id.as_deref())?)
                }
                EventKind::TabMoved | EventKind::TabClosed => {
                    self.tab_ids.contains(required(header.tab_id.as_deref())?)
                }
                EventKind::WorkspaceClosed => self
                    .workspace_ids
                    .contains(required(header.workspace_id.as_deref())?),
                EventKind::WorkspaceMetadataUpdated => self.workspace_ids.contains(required(
                    header
                        .workspace
                        .as_ref()
                        .and_then(|w| w.workspace_id.as_deref()),
                )?),
            };
        if !matched {
            return Ok(None);
        }
        serde_json::from_str(envelope.data.get())
            .map(Some)
            .map_err(invalid_event)
    }
}

fn routing_id(id: &str) -> Result<&str, SubscriptionError> {
    super::super::nonempty(id, "event routing ID")
        .map_err(|e| SubscriptionError::InvalidEvent(e.to_string()))?;
    Ok(id)
}

fn deserialize_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let id = String::deserialize(deserializer)?;
    routing_id(&id).map_err(serde::de::Error::custom)?;
    Ok(id)
}

fn invalid_event(error: serde_json::Error) -> SubscriptionError {
    SubscriptionError::InvalidEvent(error.to_string())
}

#[derive(Debug, thiserror::Error)]
pub enum SubscriptionError {
    #[error("cannot establish required Herdr subscription: {0}")]
    Establish(String),
    #[error("required Herdr subscription dropped and resubscribe failed: {0}")]
    Resubscribe(String),
    #[error("invalid Herdr event: {0}")]
    InvalidEvent(String),
    #[error("Herdr subscription stopped")]
    Stopped,
}

#[derive(Debug, PartialEq, Eq)]
pub enum SubscriptionUpdate {
    Event(HerdrEvent),
    /// Subscription is already active again. The caller must now do one full
    /// read, before consuming more events, to cover the disconnected interval.
    Resubscribed,
}

pub struct HerdrSubscription {
    client: Arc<SocketClient>,
    filter: EventFilter,
    reader: BufReader<UnixStream>,
    stopped: bool,
    resubscribed: bool,
    cancellation: Arc<Mutex<Cancellation>>,
}

struct Cancellation {
    stream: UnixStream,
    cancelled: bool,
}

pub struct SubscriptionCancel {
    state: Arc<Mutex<Cancellation>>,
}
impl SubscriptionCancel {
    pub fn cancel(&self) -> std::io::Result<()> {
        let mut state = self.state.lock().expect("subscription cancellation lock");
        if state.cancelled {
            return Ok(());
        }
        state.cancelled = true;
        match state.stream.shutdown(Shutdown::Both) {
            Err(error) if error.kind() == std::io::ErrorKind::NotConnected => Ok(()),
            result => result,
        }
    }
}

impl SocketClient {
    /// Establish and acknowledge before the caller's first read. Failure is
    /// terminal for the caller; there is deliberately no polling fallback.
    pub fn subscribe(
        self: &Arc<Self>,
        filter: EventFilter,
    ) -> Result<HerdrSubscription, SubscriptionError> {
        let reader = subscribe(self, &filter).map_err(SubscriptionError::Establish)?;
        let stream = reader
            .get_ref()
            .try_clone()
            .map_err(|e| SubscriptionError::Establish(e.to_string()))?;
        Ok(HerdrSubscription {
            client: Arc::clone(self),
            filter,
            reader,
            stopped: false,
            resubscribed: false,
            cancellation: Arc::new(Mutex::new(Cancellation {
                stream,
                cancelled: false,
            })),
        })
    }
}

fn subscribe(client: &SocketClient, filter: &EventFilter) -> Result<BufReader<UnixStream>, String> {
    if filter.kinds.is_empty() {
        return Err("at least one event type is required".into());
    }
    let subscriptions: Vec<_> = filter
        .kinds
        .iter()
        .map(|kind| json!({"type":kind.to_string()}))
        .collect();
    let (reader, result) = client
        .start("events.subscribe", json!({"subscriptions":subscriptions}))
        .map_err(|e| e.to_string())?;
    if result["type"] != "subscription_started" {
        return Err("unexpected subscription acknowledgement".into());
    }
    Ok(reader)
}

impl HerdrSubscription {
    /// A shutdown handle lets the owner's event loop cancel a blocked receive
    /// without a timer or an extra thread owned by this client.
    pub fn cancellation_handle(&self) -> SubscriptionCancel {
        SubscriptionCancel {
            state: Arc::clone(&self.cancellation),
        }
    }

    /// Update local routing without reopening the type-filtered subscription.
    pub fn set_watched_ids(
        &mut self,
        pane_ids: BTreeSet<String>,
        tab_ids: BTreeSet<String>,
        workspace_ids: BTreeSet<String>,
    ) {
        self.filter.pane_ids = pane_ids;
        self.filter.tab_ids = tab_ids;
        self.filter.workspace_ids = workspace_ids;
    }

    fn cancelled(&self) -> bool {
        self.cancellation
            .lock()
            .expect("subscription cancellation lock")
            .cancelled
    }

    pub fn receive(&mut self) -> Result<SubscriptionUpdate, SubscriptionError> {
        if self.stopped || self.cancelled() {
            return Err(SubscriptionError::Stopped);
        }
        loop {
            match read_frame(&mut self.reader, None) {
                Ok(frame) => match self.filter.decode(&frame) {
                    Ok(Some(event)) => return Ok(SubscriptionUpdate::Event(event)),
                    Ok(None) => continue,
                    Err(error) => {
                        self.stopped = true;
                        return Err(error);
                    }
                },
                Err(error) => {
                    if error.kind() == std::io::ErrorKind::InvalidData {
                        self.stopped = true;
                        return Err(SubscriptionError::InvalidEvent(error.to_string()));
                    }
                    if self.cancelled() {
                        self.stopped = true;
                        return Err(SubscriptionError::Stopped);
                    }
                    if self.resubscribed {
                        self.stopped = true;
                        return Err(SubscriptionError::Resubscribe(error.to_string()));
                    }
                    self.resubscribed = true;
                    let _ = self.reader.get_ref().shutdown(Shutdown::Both);
                    let replacement = subscribe(&self.client, &self.filter).and_then(|reader| {
                        let stream = reader.get_ref().try_clone().map_err(|e| e.to_string())?;
                        Ok((reader, stream))
                    });
                    let mut state = self
                        .cancellation
                        .lock()
                        .expect("subscription cancellation lock");
                    if state.cancelled {
                        if let Ok((reader, _)) = replacement {
                            let _ = reader.get_ref().shutdown(Shutdown::Both);
                        }
                        self.stopped = true;
                        return Err(SubscriptionError::Stopped);
                    }
                    match replacement {
                        Ok((reader, stream)) => {
                            state.stream = stream;
                            self.reader = reader;
                            return Ok(SubscriptionUpdate::Resubscribed);
                        }
                        Err(error) => {
                            self.stopped = true;
                            return Err(SubscriptionError::Resubscribe(error));
                        }
                    }
                }
            }
        }
    }
}
