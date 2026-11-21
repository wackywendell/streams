//! Minimal Phase-1 frontend adapter for `--current` and save notices; Phase 2 deletes this file.

use std::{collections::BTreeMap, path::PathBuf};

use super::{Result, RuntimeError, native_markers::selected_stream};
use crate::frontend::{
    FrontendError, FrontendId,
    herdr::{HerdrClient, HerdrNativeSnapshot},
};

/// A frontend as seen from one caller during Phase 1 (before `--current` and `agent context` go).
pub trait Frontend {
    /// The Stream the caller's workspace names (Herdr only; hc returns `None`).
    fn caller_stream(&mut self) -> Result<Option<String>>;
    /// Cached workspace Stream hint for an unenrolled conversation's context read.
    fn context_stream(&mut self) -> Option<String>;
    /// Tell the caller's views that this command saved records.
    fn notify_saved(&mut self) -> Result<()>;
}

/// The caller a frontend serves: its records' scope, environment and working directory.
pub struct Caller {
    pub catalog_source: String,
    pub host: String,
    pub environment: BTreeMap<String, String>,
    pub cwd: PathBuf,
    /// Seconds each native request may take.
    pub timeout: f64,
}

pub fn connect(id: FrontendId, caller: Caller) -> Box<dyn Frontend> {
    match id {
        FrontendId::Herdr => Box::new(HerdrFrontend::new(caller)),
        FrontendId::Hc => Box::new(HcFrontend { caller }),
        FrontendId::Cmux => Box::new(NoopFrontend),
    }
}

struct NoopFrontend;
impl Frontend for NoopFrontend {
    fn caller_stream(&mut self) -> Result<Option<String>> {
        Ok(None)
    }
    fn context_stream(&mut self) -> Option<String> {
        None
    }
    fn notify_saved(&mut self) -> Result<()> {
        Ok(())
    }
}

pub struct HerdrFrontend {
    caller: Caller,
    client: Option<HerdrClient>,
    workspace_stream: Option<Option<String>>,
}

impl HerdrFrontend {
    pub fn new(caller: Caller) -> Self {
        Self {
            caller,
            client: None,
            workspace_stream: None,
        }
    }

    pub fn observed(caller: Caller, client: HerdrClient, workspace_stream: Option<String>) -> Self {
        Self {
            caller,
            client: Some(client),
            workspace_stream: Some(workspace_stream),
        }
    }

    fn client(&mut self) -> Result<&HerdrClient> {
        if self.client.is_none() {
            let client = HerdrClient::connect(self.caller.environment.clone(), self.caller.timeout)
                .and_then(|client| client.observing())?;
            self.client = Some(client);
        }
        Ok(self.client.as_ref().expect("connected Herdr client"))
    }
}

impl Frontend for HerdrFrontend {
    fn caller_stream(&mut self) -> Result<Option<String>> {
        if let Some(stream) = &self.workspace_stream {
            return Ok(stream.clone());
        }
        let client = self.client()?;
        let snapshot = client.session_snapshot()?;
        let pane = client.current_pane_from(&snapshot)?;
        let parsed = HerdrNativeSnapshot::parse(&snapshot)?;
        let stream = selected_stream(
            &parsed,
            &pane.workspace_id,
            &self.caller.catalog_source,
            &self.caller.host,
        )
        .map_err(|error| RuntimeError::Frontend(FrontendError::invalid(error.to_string())))?;
        self.workspace_stream = Some(stream.clone());
        Ok(stream)
    }

    fn context_stream(&mut self) -> Option<String> {
        self.workspace_stream.clone().flatten()
    }

    fn notify_saved(&mut self) -> Result<()> {
        Ok(())
    }
}

pub struct HcFrontend {
    caller: Caller,
}

impl Frontend for HcFrontend {
    fn caller_stream(&mut self) -> Result<Option<String>> {
        Ok(None)
    }

    fn context_stream(&mut self) -> Option<String> {
        None
    }

    fn notify_saved(&mut self) -> Result<()> {
        crate::runtime::hc::notify_saved(
            &self.caller.environment,
            &self.caller.cwd,
            self.caller.timeout,
        )
    }
}

#[cfg(test)]
mod tests;
