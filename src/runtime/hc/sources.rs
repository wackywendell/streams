//! One helper-owned source label. No discovery, retries or I/O in Drop.
use super::{PiSourceClaim, decode_claim, labels, verify_claim_route};
use crate::{
    frontend::{
        EndpointId,
        hc::{HcClient, processes::ProcessObserver},
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
    pid: Option<u32>,
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

    /// Read the hinted terminal and verify the published SDK-to-helper route before writing.
    pub fn register(
        client: &mut HcClient,
        terminal: &str,
        conversation: ConversationId,
        sdk_pid: u32,
        helper_pid: u32,
        observer: &dyn ProcessObserver,
        cancelled: &AtomicBool,
    ) -> Effect<Self> {
        let unavailable = || Effect::NotApplied {
            reason: "Pi source registration is unavailable".into(),
        };
        if cancelled.load(Ordering::Acquire)
            || terminal.is_empty()
            || terminal.contains('/')
            || conversation.harness != "pi"
            || sdk_pid == 0
            || helper_pid == 0
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
        let Some(root_pid) = found.pid.filter(|pid| *pid != 0) else {
            return unavailable();
        };
        if found.state != "live" || found.address.is_empty() || found.address.contains('/') {
            return unavailable();
        }
        let (Ok(root), Ok(sdk), Ok(helper)) = (
            observer.observe(root_pid),
            observer.observe(sdk_pid),
            observer.observe(helper_pid),
        ) else {
            return unavailable();
        };
        if root.key.pid != root_pid || sdk.key.pid != sdk_pid || helper.key.pid != helper_pid {
            return unavailable();
        }
        let claim = PiSourceClaim {
            version: 1,
            endpoint: client.endpoint().clone(),
            conversation,
            terminal_root: root.key,
            sdk: sdk.key,
            helper: helper.key,
        };
        let Ok(key) = labels::source_key(&claim.helper) else {
            return unavailable();
        };
        let Ok(value) = serde_json::to_string(&claim) else {
            return unavailable();
        };
        if found.labels.contains_key(&key)
            || verify_claim_route(&key, &value, client.endpoint(), root_pid, observer).is_err()
        {
            return unavailable();
        }
        for (existing_key, existing_value) in found
            .labels
            .iter()
            .filter(|(key, _)| key.starts_with("streams.pi."))
        {
            let Ok(existing) =
                decode_claim(existing_key, existing_value, client.endpoint(), root_pid)
            else {
                return unavailable();
            };
            match observer.exited(&existing.helper) {
                Ok(true) => continue,
                Ok(false) => {}
                Err(_) => return unavailable(),
            }
            if existing.sdk == claim.sdk
                || verify_claim_route(
                    existing_key,
                    existing_value,
                    client.endpoint(),
                    root_pid,
                    observer,
                )
                .is_err()
            {
                return unavailable();
            }
        }
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
