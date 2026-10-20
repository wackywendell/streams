use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

use crate::process::{self, ProcessRequest, ProcessRunner};
use serde_json::Value;

/// Native effect state, never proof of catalog ownership or pane attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    ReadOnly,
    Uncertain,
    Acknowledged,
}

/// Recovery evidence includes malformed replies and partial output on timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEvidence {
    pub args: Vec<String>,
    pub outcome: CommandOutcome,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
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

#[derive(Clone)]
pub(super) struct Transport {
    pub program: PathBuf,
    pub environment: BTreeMap<OsString, OsString>,
    pub timeout: f64,
    pub runner: Arc<dyn ProcessRunner>,
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

    pub fn run(
        &mut self,
        client: &Transport,
        args: &[String],
        effect: bool,
    ) -> Result<Vec<u8>, FrontendError> {
        process::deadline(client.timeout, "native command").map_err(|e| self.error(e.message))?;
        let result = client.runner.run(&ProcessRequest {
            program: client.program.clone(),
            args: args.iter().map(OsString::from).collect(),
            cwd: None,
            input: None,
            env: client.environment.clone(),
            allowed: vec![0],
            timeout: client.timeout,
        });
        let outcome = if effect {
            CommandOutcome::Uncertain
        } else {
            CommandOutcome::ReadOnly
        };
        match result {
            Ok(output) => {
                let code = output.status.code();
                let stdout = output.stdout.clone();
                self.commands.push(CommandEvidence {
                    args: args.to_vec(),
                    outcome,
                    code,
                    stdout: output.stdout,
                    stderr: output.stderr,
                });
                if code != Some(0) {
                    return Err(self.error("native command failed; inspect command evidence"));
                }
                Ok(stdout)
            }
            Err(error) => {
                self.commands.push(CommandEvidence {
                    args: args.to_vec(),
                    outcome,
                    code: error.code,
                    stdout: error.stdout,
                    stderr: error.stderr,
                });
                Err(self.error(error.message))
            }
        }
    }

    pub fn json(
        &mut self,
        client: &Transport,
        args: &[String],
        effect: bool,
        banner: bool,
    ) -> Result<Value, FrontendError> {
        let bytes = self.run(client, args, effect)?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| self.error(format!("native response is not UTF-8: {e}")))?;
        let text = if banner {
            let start = text
                .find(['{', '['])
                .ok_or_else(|| self.error("native command did not return JSON"))?;
            &text[start..]
        } else {
            text
        };
        crate::codec::load_json(text)
            .map_err(|e| self.error(format!("native command did not return valid JSON: {e}")))
    }

    pub fn acknowledge(&mut self) {
        if let Some(command) = self.commands.last_mut() {
            command.outcome = CommandOutcome::Acknowledged;
        }
    }
}

pub(super) fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

pub(super) fn string(value: &Value, key: &str) -> Result<String, FrontendError> {
    let text = value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| FrontendError::invalid(format!("invalid native {key}")))?;
    super::nonempty(text, key)?;
    Ok(text.into())
}

pub(super) fn launch_args(request: &super::CreateWorkspace) -> Result<Vec<String>, FrontendError> {
    super::nonempty(&request.title, "workspace title")?;
    if !request.cwd.is_absolute() {
        return Err(FrontendError::invalid("workspace cwd must be absolute"));
    }
    let mut result = Vec::new();
    for (key, value) in &request.environment {
        if key.is_empty()
            || key.contains('=')
            || key.chars().any(char::is_control)
            || value.contains('\0')
        {
            return Err(FrontendError::invalid("invalid launch environment"));
        }
        result.extend(["--env".into(), format!("{key}={value}")]);
    }
    Ok(result)
}
