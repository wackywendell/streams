use super::*;
use crate::frontend::transport::launch_environment;
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
        workspace: &WorkspaceRef,
        cwd: &Path,
        environment: &BTreeMap<String, String>,
        label: Option<&str>,
    ) -> Result<(HerdrTab, HerdrPane), FrontendError> {
        check_ref(self, workspace)?;
        let (cwd, environment) = launch_parameters(cwd, environment)?;
        if let Some(label) = label {
            nonempty(label, "Herdr tab label")?;
        }
        let mut command = args(&[
            "tab",
            "create",
            "--workspace",
            &workspace.native_id,
            "--cwd",
            cwd,
        ]);
        command.extend(environment);
        command.push("--no-focus".into());
        if let Some(label) = label {
            command.extend(["--label".into(), label.into()]);
        }
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        let result = self.result(&mut op, &command, "tab_created", true)?;
        let tab = op.capture(HerdrTab::parse(&result["tab"]))?;
        let pane = op.capture(HerdrPane::parse(&result["root_pane"]))?;
        if tab.workspace_id != workspace.native_id
            || pane.workspace_id != workspace.native_id
            || pane.tab_id != tab.id
        {
            return Err(op.error("Herdr created tab/pane/workspace ID mismatch"));
        }
        op.acknowledge();
        Ok((tab, pane))
    }

    pub fn split_pane(
        &self,
        pane: &HerdrPane,
        cwd: &Path,
        environment: &BTreeMap<String, String>,
    ) -> Result<HerdrPane, FrontendError> {
        validate_pane(pane)?;
        let (cwd, environment) = launch_parameters(cwd, environment)?;
        let mut command = args(&[
            "pane",
            "split",
            "--pane",
            &pane.id,
            "--direction",
            "right",
            "--cwd",
            cwd,
        ]);
        command.extend(environment);
        command.push("--no-focus".into());
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        let result = self.result(&mut op, &command, "pane_split", true)?;
        let created = op.capture(HerdrPane::parse(&result["pane"]))?;
        if created.workspace_id != pane.workspace_id
            || created.tab_id != pane.tab_id
            || created.id == pane.id
        {
            return Err(op.error("Herdr split pane/tab/workspace ID mismatch"));
        }
        op.acknowledge();
        Ok(created)
    }

    pub fn rename_tab(&self, tab: &HerdrTab, label: &str) -> Result<(), FrontendError> {
        nonempty(&tab.id, "Herdr tab ID")?;
        nonempty(&tab.workspace_id, "Herdr workspace ID")?;
        nonempty(label, "Herdr tab label")?;
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        let result = self.result(
            &mut op,
            &args(&["tab", "rename", &tab.id, label]),
            "tab_info",
            true,
        )?;
        let renamed = op.capture(HerdrTab::parse(&result["tab"]))?;
        if renamed.id != tab.id
            || renamed.workspace_id != tab.workspace_id
            || renamed.label.as_deref() != Some(label)
        {
            return Err(op.error("Herdr renamed tab identity/label mismatch"));
        }
        op.acknowledge();
        Ok(())
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
        let mut command = args(&[
            "agent",
            "start",
            name,
            "--kind",
            agent,
            "--pane",
            &pane.id,
            "--timeout",
            "20000",
            "--",
        ]);
        command.extend_from_slice(arguments);
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        let result = self.result(&mut op, &command, "agent_started", true)?;
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

    pub fn focus_agent(&self, pane: &HerdrPane) -> Result<(), FrontendError> {
        validate_pane(pane)?;
        let mut op = Operation::default();
        self.verify_endpoint(&mut op)?;
        op.run(&self.transport, &args(&["agent", "focus", &pane.id]), true)?;
        op.acknowledge();
        Ok(())
    }
}

fn validate_pane(pane: &HerdrPane) -> Result<(), FrontendError> {
    nonempty(&pane.id, "Herdr pane ID")?;
    nonempty(&pane.tab_id, "Herdr tab ID")?;
    nonempty(&pane.workspace_id, "Herdr workspace ID")
}

fn launch_parameters<'a>(
    cwd: &'a Path,
    environment: &BTreeMap<String, String>,
) -> Result<(&'a str, Vec<String>), FrontendError> {
    let environment = launch_environment(cwd, environment)?;
    let cwd = cwd
        .to_str()
        .ok_or_else(|| FrontendError::invalid("Herdr cwd is not UTF-8"))?;
    nonempty(cwd, "Herdr cwd")?;
    if environment.iter().any(|v| v.chars().any(char::is_control)) {
        return Err(FrontendError::invalid("invalid Herdr launch environment"));
    }
    Ok((cwd, environment))
}
