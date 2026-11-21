use serde_json::Value;

/// Native effect state, never proof of catalog ownership or pane attestation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CommandOutcome {
    ReadOnly,
    Uncertain,
    Acknowledged,
}

/// Recovery evidence includes malformed replies and partial output on timeout.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CommandEvidence {
    #[serde(flatten)]
    pub request: NativeRequest,
    pub outcome: CommandOutcome,
}

/// Socket request and raw reply frame.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(untagged)]
pub enum NativeRequest {
    Socket {
        method: String,
        params: Value,
        /// Raw reply frame; empty when none arrived.
        response: Vec<u8>,
    },
}

/// Errors preserve all commands from a multi-effect operation. Inspect uncertain
/// commands before retrying; no adapter retries or rolls back native resources.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct FrontendError {
    pub message: String,
    pub commands: Vec<CommandEvidence>,
}

impl FrontendError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            commands: Vec::new(),
        }
    }
}

#[derive(Default)]
pub(super) struct Operation {
    pub commands: Vec<CommandEvidence>,
}

impl Operation {
    pub fn error(&self, message: impl Into<String>) -> FrontendError {
        FrontendError {
            message: message.into(),
            commands: self.commands.clone(),
        }
    }

    pub fn capture<T>(&self, result: Result<T, FrontendError>) -> Result<T, FrontendError> {
        result.map_err(|e| self.error(e.message))
    }

    pub fn acknowledge(&mut self) {
        if let Some(command) = self.commands.last_mut() {
            command.outcome = CommandOutcome::Acknowledged;
        }
    }
}

pub(super) fn string(value: &Value, key: &str) -> Result<String, FrontendError> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| FrontendError::invalid(format!("invalid native {key}")))?;
    super::nonempty(text, key)?;
    Ok(text.into())
}
