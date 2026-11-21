//! Herdr endpoint discovery, exact native identities and workspace effects.
//!
//! Observations select pass-local targets. The subscription-ended check is not
//! an atomic frontend precondition.

pub mod events;
mod snapshot;
pub mod socket;
mod topology;
pub use snapshot::HerdrNativeSnapshot;
pub use topology::{HerdrAgent, HerdrInventoryPane, HerdrLayout, HerdrTab, HerdrTopology};

use super::transport::{NativeRequest, Operation, string};
use super::*;
use serde_json::{Value, json};
use socket::SocketClient;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// The server's reported socket and optional native session namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrEndpoint {
    pub socket: PathBuf,
    pub session: Option<String>,
}

impl HerdrEndpoint {
    /// Herdr's own resolution order: `HERDR_SOCKET_PATH`, then the
    /// `HERDR_SESSION` namespace, then the default session socket.
    pub fn resolve(context: &BTreeMap<String, String>) -> Result<Self, FrontendError> {
        let value = |key: &str| context.get(key).filter(|v| !v.is_empty());
        let session = value("HERDR_SESSION").cloned();
        if let Some(name) = &session
            && (matches!(name.as_str(), "." | "..")
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
        {
            return Err(FrontendError::invalid("invalid HERDR_SESSION"));
        }
        let socket = match value("HERDR_SOCKET_PATH") {
            Some(path) => PathBuf::from(path),
            None => {
                let config = match value("XDG_CONFIG_HOME") {
                    Some(path) => PathBuf::from(path),
                    None => PathBuf::from(
                        value("HOME").ok_or_else(|| FrontendError::invalid("HOME is required"))?,
                    )
                    .join(".config"),
                }
                .join("herdr");
                match &session {
                    Some(name) => config.join("sessions").join(name).join("herdr.sock"),
                    None => config.join("herdr.sock"),
                }
            }
        };
        if !socket.is_absolute() {
            return Err(FrontendError::invalid("invalid Herdr server socket path"));
        }
        Ok(Self { socket, session })
    }

    pub fn identity(&self) -> EndpointId {
        EndpointId(format!(
            "herdr:{}",
            serde_json::json!({"socket": self.socket, "session": self.session})
        ))
    }
}

/// Herdr's workspace record; caller-derived cwd is supplied only by exact pane lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrWorkspace {
    pub id: String,
    pub label: String,
    pub cwd: String,
}

impl HerdrWorkspace {
    pub fn parse(value: &Value) -> Result<Self, FrontendError> {
        Ok(Self {
            id: string(value, "workspace_id")?,
            label: value
                .get("label")
                .and_then(Value::as_str)
                .ok_or_else(|| FrontendError::invalid("invalid Herdr workspace label"))?
                .into(),
            cwd: String::new(),
        })
    }
}

/// Exact pane/tab/workspace tuple, never a generic frontend reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrPane {
    pub id: String,
    pub tab_id: String,
    pub workspace_id: String,
    pub cwd: String,
}

impl HerdrPane {
    pub fn parse(value: &Value) -> Result<Self, FrontendError> {
        let mut directories = Vec::new();
        for key in ["foreground_cwd", "cwd"] {
            match value.get(key) {
                None | Some(Value::Null) => {}
                Some(Value::String(v)) => directories.push(v.as_str()),
                _ => return Err(FrontendError::invalid("invalid Herdr pane cwd")),
            }
        }
        Ok(Self {
            id: string(value, "pane_id")?,
            tab_id: string(value, "tab_id")?,
            workspace_id: string(value, "workspace_id")?,
            cwd: directories
                .into_iter()
                .find(|v| !v.is_empty())
                .unwrap_or_default()
                .into(),
        })
    }
}

/// One pinned protocol-22 endpoint per process. Herdr closes ordinary request
/// sockets after each reply, so each request connects; only subscriptions persist.
#[derive(Clone)]
pub struct HerdrClient {
    socket: Arc<SocketClient>,
    endpoint: HerdrEndpoint,
    context: BTreeMap<String, String>,
    subscription_ended: Option<Arc<AtomicBool>>,
    _pass_subscription: Option<Arc<events::PassSubscription>>,
}

impl HerdrClient {
    /// The single protocol check rejects incompatible servers without
    /// updating or restarting them, and pins the endpoint for this process.
    pub fn connect(context: BTreeMap<String, String>, timeout: f64) -> Result<Self, FrontendError> {
        Self::connect_inner(context, timeout, None)
    }

    pub fn connect_cancellable(
        context: BTreeMap<String, String>,
        timeout: f64,
        cancellation: crate::cancellation::Cancellation,
    ) -> Result<Self, FrontendError> {
        Self::connect_inner(context, timeout, Some(cancellation))
    }

    fn connect_inner(
        context: BTreeMap<String, String>,
        timeout: f64,
        cancellation: Option<crate::cancellation::Cancellation>,
    ) -> Result<Self, FrontendError> {
        let endpoint = HerdrEndpoint::resolve(&context)?;
        let socket = Arc::new(match cancellation {
            Some(cancel) => {
                SocketClient::connect_cancellable(endpoint.socket.clone(), timeout, cancel)?
            }
            None => SocketClient::connect(endpoint.socket.clone(), timeout)?,
        });
        Ok(Self {
            socket,
            endpoint,
            context,
            subscription_ended: None,
            _pass_subscription: None,
        })
    }

    /// Start a bounded foreground observation without an event controller.
    /// Register before taking its snapshot; an ended pass is never revived here.
    pub fn observing(&self) -> Result<Self, FrontendError> {
        if self.subscription_ended.is_some() {
            return Ok(self.clone());
        }
        let subscription = Arc::new(
            events::PassSubscription::start(&self.socket)
                .map_err(|error| FrontendError::invalid(error.to_string()))?,
        );
        Ok(Self {
            subscription_ended: Some(subscription.ended()),
            _pass_subscription: Some(subscription),
            ..self.clone()
        })
    }

    /// Bind this pass to the current subscription. This cheap disconnect check
    /// does not atomically protect against a restart after the check.
    pub fn with_subscription(&self, subscription: &events::SubscriptionCancel) -> Self {
        Self {
            subscription_ended: Some(subscription.ended()),
            ..self.clone()
        }
    }

    /// A delivered disconnect ends this observation, including native-derived catalog actions.
    pub fn check_subscription(&self) -> Result<(), FrontendError> {
        if self
            .subscription_ended
            .as_ref()
            .is_some_and(|ended| ended.load(Ordering::Acquire))
        {
            Err(FrontendError::invalid(
                "Herdr subscription ended; native pass abandoned",
            ))
        } else {
            Ok(())
        }
    }

    /// Persistent event subscriptions share the pinned endpoint.
    pub fn socket(&self) -> &Arc<SocketClient> {
        &self.socket
    }

    /// Bounded native transport. Selection and checked replies stay in the
    /// native-card engine; this adapter never retries an operation.
    pub(crate) fn card_request(
        &self,
        method: &str,
        params: Value,
        expected: Option<&str>,
        effect: bool,
    ) -> Result<(Value, Vec<super::CommandEvidence>), FrontendError> {
        let mut op = Operation::default();
        let value = if let Some(expected) = expected {
            self.result(&mut op, method, params, expected, effect)?
        } else {
            self.call(&mut op, method, params, effect)?;
            Value::Null
        };
        if effect {
            op.acknowledge();
        }
        Ok((value, op.commands))
    }

    pub fn endpoint(&self) -> &HerdrEndpoint {
        &self.endpoint
    }

    /// Select Pi's observation context without changing the endpoint.
    pub fn with_context(&self, context: BTreeMap<String, String>) -> Result<Self, FrontendError> {
        if HerdrEndpoint::resolve(&context)? != self.endpoint {
            return Err(FrontendError::invalid("Herdr reader endpoint changed"));
        }
        Ok(Self {
            context,
            ..self.clone()
        })
    }

    pub fn current_pane_from(&self, snapshot: &Value) -> Result<HerdrPane, FrontendError> {
        if self.context.get("HERDR_ENV").map(String::as_str) != Some("1") {
            return Err(FrontendError::invalid(
                "current Herdr targeting requires HERDR_ENV=1 and HERDR_PANE_ID",
            ));
        }
        let id = self
            .context
            .get("HERDR_PANE_ID")
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                FrontendError::invalid(
                    "current Herdr targeting requires HERDR_ENV=1 and HERDR_PANE_ID",
                )
            })?;
        let panes = snapshot["panes"]
            .as_array()
            .ok_or_else(|| FrontendError::invalid("missing Herdr pane inventory"))?;
        let mut matches = panes
            .iter()
            .filter(|pane| pane["pane_id"].as_str() == Some(id));
        let pane = HerdrPane::parse(
            matches
                .next()
                .ok_or_else(|| FrontendError::invalid("Herdr caller missing from snapshot"))?,
        )?;
        if matches.next().is_some() {
            return Err(FrontendError::invalid("ambiguous Herdr caller"));
        }
        let workspaces = snapshot["workspaces"]
            .as_array()
            .ok_or_else(|| FrontendError::invalid("missing Herdr workspace inventory"))?;
        let mut matches = workspaces
            .iter()
            .filter(|workspace| workspace["workspace_id"].as_str() == Some(&pane.workspace_id));
        HerdrWorkspace::parse(
            matches
                .next()
                .ok_or_else(|| FrontendError::invalid("Herdr caller workspace missing"))?,
        )?;
        if matches.next().is_some() {
            return Err(FrontendError::invalid("ambiguous Herdr caller workspace"));
        }
        Ok(pane)
    }

    /// One socket request, recorded with its raw reply before it is decoded.
    fn call(
        &self,
        op: &mut Operation,
        method: &str,
        params: Value,
        effect: bool,
    ) -> Result<Value, FrontendError> {
        if effect {
            self.check_subscription()
                .map_err(|error| op.error(error.message))?;
        }
        let (response, result) = self.socket.exchange(method, params.clone());
        op.commands.push(CommandEvidence {
            request: NativeRequest::Socket {
                method: method.into(),
                params,
                response,
            },
            outcome: if effect {
                CommandOutcome::Uncertain
            } else {
                CommandOutcome::ReadOnly
            },
        });
        op.capture(result)
    }

    fn result(
        &self,
        op: &mut Operation,
        method: &str,
        params: Value,
        expected: &str,
        effect: bool,
    ) -> Result<Value, FrontendError> {
        let result = self.call(op, method, params, effect)?;
        if result.get("type").and_then(Value::as_str) != Some(expected) {
            return Err(op.error(format!("unexpected Herdr result; expected {expected}")));
        }
        Ok(result)
    }

    pub fn session_snapshot(&self) -> Result<Value, FrontendError> {
        self.read_snapshot(&mut Operation::default())
    }

    fn read_snapshot(&self, op: &mut Operation) -> Result<Value, FrontendError> {
        let result = self.result(op, "session.snapshot", json!({}), "session_snapshot", false)?;
        result
            .get("snapshot")
            .filter(|v| v.is_object())
            .cloned()
            .ok_or_else(|| op.error("invalid Herdr snapshot"))
    }

    /// Complete structural topology for the later arrangement/attestation layer.
    /// Missing collections, duplicate IDs and contradictory ownership fail closed.
    pub fn topology(&self) -> Result<HerdrTopology, FrontendError> {
        let mut op = Operation::default();
        let snapshot = self.read_snapshot(&mut op)?;
        op.capture(HerdrTopology::parse(&snapshot))
    }
}
