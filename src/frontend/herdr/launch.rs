use super::*;
use crate::frontend::transport::launch_environment;
use serde_json::json;
use std::path::Path;

/// A ready native launch receipt, not catalog ownership or session attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HerdrStartedAgent {
    /// Preserve the caller's original, parsed pane observation, including cwd.
    pub pane: HerdrPane,
    pub name: String,
    pub agent: String,
    pub terminal_id: Option<String>,
    pub interactive_ready: bool,
}

impl HerdrClient {
    pub fn create_tab(
        &self,
        workspace: &NativeRef,
        cwd: &Path,
        environment: &BTreeMap<String, String>,
        label: Option<&str>,
    ) -> Result<(HerdrTab, HerdrPane), FrontendError> {
        check_ref(self, workspace)?;
        let (cwd, environment) = launch_parameters(cwd, environment)?;
        if let Some(label) = label {
            nonempty(label, "Herdr tab label")?;
        }
        let params = json!({"workspace_id":workspace.key,"cwd":cwd,"env":environment,
            "focus":false,"label":label});
        let mut op = Operation::default();
        let result = self.result(&mut op, "tab.create", params, "tab_created", true)?;
        let tab = op.capture(HerdrTab::parse(&result["tab"]))?;
        let pane = op.capture(HerdrPane::parse(&result["root_pane"]))?;
        if tab.workspace_id != workspace.key
            || pane.workspace_id != workspace.key
            || pane.tab_id != tab.id
        {
            return Err(op.error("Herdr created tab/pane/workspace ID mismatch"));
        }
        op.acknowledge();
        Ok((tab, pane))
    }

    pub fn start_agent(
        &self,
        pane: &HerdrPane,
        name: &str,
        agent: &str,
        arguments: &[String],
    ) -> Result<HerdrStartedAgent, FrontendError> {
        validate_pane(pane)?;
        if name.is_empty()
            || name.len() > 32
            || !name.as_bytes()[0].is_ascii_lowercase()
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-'))
        {
            return Err(FrontendError::invalid("invalid Herdr agent name"));
        }
        // Agent kinds remain extensible text, not the runtime's closed harness set.
        nonempty(agent, "Herdr agent kind")?;
        if arguments.iter().any(|v| v.chars().any(char::is_control)) {
            return Err(FrontendError::invalid("invalid Herdr agent argument"));
        }
        let params = json!({"name":name,"kind":agent,"pane_id":pane.id,"timeout_ms":20000,
            "args":arguments});
        let mut op = Operation::default();
        let result = self.result(&mut op, "agent.start", params, "agent_started", true)?;
        let started = &result["agent"];
        for (key, expected) in [
            ("pane_id", pane.id.as_str()),
            ("tab_id", pane.tab_id.as_str()),
            ("workspace_id", pane.workspace_id.as_str()),
            ("name", name),
            ("agent", agent),
        ] {
            if started.get(key).and_then(Value::as_str) != Some(expected) {
                return Err(op.error("Herdr started agent identity mismatch"));
            }
        }
        if started.get("interactive_ready") != Some(&Value::Bool(true)) {
            return Err(op.error("Herdr started agent is not interactively ready"));
        }
        let terminal_id = match started.get("terminal_id") {
            None | Some(Value::Null) => None,
            Some(_) => Some(op.capture(string(started, "terminal_id"))?),
        };
        op.acknowledge();
        Ok(HerdrStartedAgent {
            pane: pane.clone(),
            name: name.into(),
            agent: agent.into(),
            terminal_id,
            interactive_ready: true,
        })
    }
}

fn validate_pane(pane: &HerdrPane) -> Result<(), FrontendError> {
    nonempty(&pane.id, "Herdr pane ID")?;
    nonempty(&pane.tab_id, "Herdr tab ID")?;
    nonempty(&pane.workspace_id, "Herdr workspace ID")
}

fn launch_parameters<'a, 'e>(
    cwd: &'a Path,
    environment: &'e BTreeMap<String, String>,
) -> Result<(&'a str, &'e BTreeMap<String, String>), FrontendError> {
    launch_environment(cwd, environment)?;
    let cwd = cwd
        .to_str()
        .ok_or_else(|| FrontendError::invalid("Herdr cwd is not UTF-8"))?;
    nonempty(cwd, "Herdr cwd")?;
    if environment
        .iter()
        .any(|(k, v)| k.chars().chain(v.chars()).any(char::is_control))
    {
        return Err(FrontendError::invalid("invalid Herdr launch environment"));
    }
    Ok((cwd, environment))
}
