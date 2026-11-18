//! One helper-owned source label. No discovery, retries or I/O in Drop.
use super::{PiSourceClaim, labels};
use crate::{
    frontend::{
        EndpointId,
        hc::{HcClient, processes::ProcessKey},
    },
    integration::{ConversationId, Effect},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Deserialize)]
struct Terminal {
    address: String,
    state: String,
    labels: BTreeMap<String, String>,
}

/// The exact label owned by a successfully registered helper.
pub struct SourceRegistration {
    endpoint: EndpointId,
    terminal: String,
    key: String,
}

impl SourceRegistration {
    pub fn terminal(&self) -> &str {
        &self.terminal
    }

    /// Publish `conversation` and the helper's own process on the hinted terminal.
    /// Other claims on the terminal are left alone; readers treat competing claims as unknown.
    pub fn register(
        client: &mut HcClient,
        terminal: &str,
        conversation: ConversationId,
        helper: ProcessKey,
        cancelled: &AtomicBool,
    ) -> Effect<Self> {
        let unavailable = || Effect::NotApplied {
            reason: "Pi source registration is unavailable".into(),
        };
        if cancelled.load(Ordering::Acquire)
            || terminal.is_empty()
            || terminal.contains('/')
            || conversation.harness != "pi"
            || crate::model::check_text(&conversation.native_id, "conversation", false).is_err()
        {
            return unavailable();
        }
        let Ok(value) = client.request("terminal.get", &json!({"terminal":terminal})) else {
            return unavailable();
        };
        let Ok(found) = serde_json::from_value::<Terminal>(value) else {
            return unavailable();
        };
        if found.state != "live" || found.address.is_empty() || found.address.contains('/') {
            return unavailable();
        }
        let claim = PiSourceClaim {
            version: PiSourceClaim::VERSION,
            conversation,
            helper,
        };
        let key = labels::source_key(&claim.helper);
        let Ok(value) = serde_json::to_string(&claim) else {
            return unavailable();
        };
        if cancelled.load(Ordering::Acquire) {
            return unavailable();
        }
        let registration = Self {
            endpoint: client.endpoint().clone(),
            terminal: found.address,
            key,
        };
        match registration.change(client, &value) {
            Effect::Applied { .. } => Effect::Applied {
                value: registration,
            },
            Effect::NotApplied { reason } => Effect::NotApplied { reason },
            Effect::Uncertain { reason } => Effect::Uncertain { reason },
        }
    }

    /// Remove only this helper's label; a replacement has a different key.
    pub fn retire(self, client: &mut HcClient) -> Effect<()> {
        self.change(client, "")
    }

    fn change(&self, client: &mut HcClient, value: &str) -> Effect<()> {
        if client.endpoint() != &self.endpoint {
            return Effect::NotApplied {
                reason: "Pi source belongs to another core".into(),
            };
        }
        let params = json!({"terminal":self.terminal,"labels":{self.key.clone():value}});
        let (_, result) = client.exchange("terminal.label", &params, true);
        match result {
            Ok(reply) => {
                let valid =
                    serde_json::from_value::<Terminal>(reply)
                        .ok()
                        .is_some_and(|terminal| {
                            terminal.address == self.terminal
                                && terminal.state == "live"
                                && if value.is_empty() {
                                    !terminal.labels.contains_key(&self.key)
                                } else {
                                    terminal
                                        .labels
                                        .get(&self.key)
                                        .is_some_and(|saved| saved == value)
                                }
                        });
                if valid {
                    Effect::Applied { value: () }
                } else {
                    Effect::Uncertain {
                        reason: "Pi source label write was not confirmed".into(),
                    }
                }
            }
            Err(failure) if failure.error.commands.is_empty() => Effect::NotApplied {
                reason: "Pi source label write did not start".into(),
            },
            Err(_) => Effect::Uncertain {
                reason: "Pi source label write was not confirmed".into(),
            },
        }
    }
}
