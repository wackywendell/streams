//! CMUX CLI contracts. No topology or pane-attestation claims are made here.

use super::transport::{Operation, Transport, args, launch_args, string};
use super::*;
use crate::process::ProcessRunner;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

/// The exact socket selected for this CMUX transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmuxEndpoint {
    pub socket: PathBuf,
}

impl CmuxEndpoint {
    pub fn identity(&self) -> EndpointId {
        EndpointId(format!("cmux:{}", self.socket.display()))
    }
}

#[derive(Clone)]
pub struct CmuxClient {
    transport: Transport,
    endpoint: CmuxEndpoint,
    context: BTreeMap<String, String>,
}

struct NativeWorkspace {
    workspace: Workspace,
    display_ref: String,
}

impl CmuxClient {
    pub fn opening_support(&self) -> crate::integration::OpeningSupport {
        use crate::integration::{LaunchLimit, OpeningMode, OpeningSupport, Placement};
        OpeningSupport::known_harnesses(
            OpeningMode::CreateOnly,
            LaunchLimit::One,
            Placement::RequiresSpace,
        )
    }
    /// Captures caller context once. Inventory is read only on explicit operations.
    pub fn connect(
        runner: Arc<dyn ProcessRunner>,
        context: BTreeMap<String, String>,
        timeout: f64,
    ) -> Result<Self, FrontendError> {
        crate::process::deadline(timeout, "CMUX").map_err(|e| FrontendError::invalid(e.message))?;
        let socket = if let Some(socket) = context.get("CMUX_SOCKET_PATH").filter(|v| !v.is_empty())
        {
            PathBuf::from(socket)
        } else {
            PathBuf::from(
                context
                    .get("HOME")
                    .ok_or_else(|| FrontendError::invalid("CMUX default socket requires HOME"))?,
            )
            .join(".local/state/cmux/cmux.sock")
        };
        if !socket.is_absolute() {
            return Err(FrontendError::invalid("CMUX socket must be absolute"));
        }
        let endpoint = CmuxEndpoint { socket };
        let mut environment: BTreeMap<OsString, OsString> =
            context.iter().map(|(k, v)| (k.into(), v.into())).collect();
        environment.insert(
            "CMUX_SOCKET_PATH".into(),
            endpoint.socket.as_os_str().to_owned(),
        );
        Ok(Self {
            transport: Transport {
                program: "cmux".into(),
                environment,
                timeout,
                runner,
            },
            endpoint,
            context,
        })
    }

    pub fn endpoint(&self) -> &CmuxEndpoint {
        &self.endpoint
    }

    pub fn create_session(
        &self,
        request: &CreateWorkspace,
        session_command: Option<&str>,
    ) -> Result<Workspace, FrontendError> {
        self.create_session_with_focus(request, session_command, true)
    }

    fn create_session_with_focus(
        &self,
        request: &CreateWorkspace,
        session_command: Option<&str>,
        focus: bool,
    ) -> Result<Workspace, FrontendError> {
        let environment = launch_args(request)?;
        let cwd = request
            .cwd
            .to_str()
            .ok_or_else(|| FrontendError::invalid("CMUX cwd is not UTF-8"))?;
        let mut command = args(&[
            "--json",
            "--id-format",
            "uuids",
            "new-workspace",
            "--name",
            &request.title,
            "--cwd",
            cwd,
        ]);
        command.extend(environment);
        command.extend(args(&["--focus", if focus { "true" } else { "false" }]));
        if let Some(session_command) = session_command {
            nonempty(session_command, "CMUX session command")?;
            command.extend(["--command".into(), session_command.into()]);
        }
        let mut op = Operation::default();
        let data = op.json(&self.transport, &command, true, true)?;
        let id = op.capture(string(&data, "workspace_id"))?;
        op.acknowledge();
        Ok(Workspace {
            reference: self.reference(id),
            title: request.title.clone(),
            cwd: cwd.into(),
        })
    }

    /// CMUX submits its single prepared command together with workspace creation.
    pub fn execute_prepared_open(
        &self,
        plan: &crate::integration::PreparedOpen,
        environment: &BTreeMap<String, String>,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> PreparedOpenOutcome {
        use crate::integration::*;
        let mut effects = unattempted_open(plan);
        let result = (|| -> Result<(), FrontendError> {
            if plan.launches.len() > 1 || !plan.adopt.is_empty() {
                return Err(FrontendError::invalid(
                    "CMUX supports one prepared launch and no adoption",
                ));
            }
            let SpaceDecision::Create { title, cwd } = &plan.space else {
                return Err(FrontendError::invalid("CMUX requires a new workspace"));
            };
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                return Err(FrontendError::invalid("native operation cancelled"));
            }
            let launch = plan.launches.first();
            let (cwd, environment, command) = match launch {
                Some(LaunchItem::Conversation(launch)) => (
                    launch.cwd(),
                    launch.environment(),
                    Some(launch.command().command_text()),
                ),
                Some(LaunchItem::Shell { cwd }) => (cwd.as_path(), environment, None),
                None => (cwd.as_path(), environment, None),
            };
            let workspace = self
                .create_session_with_focus(
                    &CreateWorkspace {
                        title: title.clone(),
                        cwd: cwd.to_path_buf(),
                        environment: environment.clone(),
                    },
                    command.as_deref(),
                    plan.focus,
                )
                .inspect_err(|error| {
                    effects.presentation.state = failed_presentation(error);
                    if launch.is_some() {
                        effects.launches[0] = failed_launch(error);
                    }
                })?;
            let location = workspace.reference;
            effects.presentation = PresentationResult {
                space: SpaceEvidence::Known {
                    value: location.clone(),
                },
                state: PresentationState::Complete,
            };
            if launch.is_some() {
                effects.launches[0] = Effect::Applied { value: location };
            }
            Ok(())
        })();
        if let Err(error) = &result
            && let PresentationState::Incomplete { reason } = &mut effects.presentation.state
        {
            reason.clone_from(&error.message);
        }
        PreparedOpenOutcome {
            effects,
            error: result.err(),
        }
    }

    fn reference(&self, id: String) -> NativeRef {
        NativeRef {
            adapter: self.frontend_id(),
            endpoint: self.endpoint_id(),
            key: id,
        }
    }

    fn inventory(&self, op: &mut Operation) -> Result<Vec<NativeWorkspace>, FrontendError> {
        let data = op.json(
            &self.transport,
            &args(&["rpc", "workspace.list", "{}"]),
            false,
            true,
        )?;
        let values = data
            .get("workspaces")
            .and_then(Value::as_array)
            .filter(|v| v.len() <= 10000)
            .ok_or_else(|| op.error("unexpected CMUX workspace inventory shape"))?;
        let mut ids = BTreeSet::new();
        let mut refs = BTreeSet::new();
        let mut result = Vec::new();
        for value in values {
            let id = op.capture(string(value, "id"))?;
            if !ids.insert(id.clone()) {
                return Err(op.error("duplicate CMUX workspace ID"));
            }
            let title = value
                .get("title")
                .and_then(Value::as_str)
                .ok_or_else(|| op.error("invalid CMUX workspace title"))?
                .into();
            let native_ref = match value.get("ref") {
                None | Some(Value::Null) => "",
                Some(Value::String(v)) => v.as_str(),
                _ => return Err(op.error("invalid CMUX workspace ref")),
            };
            if !native_ref.is_empty() {
                op.capture(nonempty(native_ref, "CMUX workspace ref"))?;
                if !refs.insert(native_ref.to_owned()) {
                    return Err(op.error("duplicate CMUX workspace ref"));
                }
            }
            if value
                .get("index")
                .is_some_and(|v| !v.is_null() && v.as_i64().is_none())
            {
                return Err(op.error("invalid CMUX workspace index"));
            }
            let cwd = match value.get("current_directory") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(v)) => v.clone(),
                _ => return Err(op.error("invalid CMUX workspace cwd")),
            };
            let display_ref = if native_ref.is_empty() {
                id.clone()
            } else {
                native_ref.into()
            };
            result.push(NativeWorkspace {
                workspace: Workspace {
                    reference: self.reference(id),
                    title,
                    cwd,
                },
                display_ref,
            });
        }
        Ok(result)
    }

    fn target(
        &self,
        op: &mut Operation,
        reference: &NativeRef,
    ) -> Result<NativeWorkspace, FrontendError> {
        op.capture(check_ref(self, reference))?;
        self.inventory(op)?
            .into_iter()
            .find(|v| v.workspace.reference == *reference)
            .ok_or_else(|| op.error("exact CMUX workspace ID not found"))
    }
}

impl WorkspaceFrontend for CmuxClient {
    fn frontend_id(&self) -> FrontendId {
        FrontendId::Cmux
    }
    fn endpoint_id(&self) -> EndpointId {
        self.endpoint.identity()
    }
    fn workspaces(&self) -> Result<Vec<Workspace>, FrontendError> {
        Ok(self
            .inventory(&mut Operation::default())?
            .into_iter()
            .map(|v| v.workspace)
            .collect())
    }
    fn resolve(&self, selector: WorkspaceSelector<'_>) -> Result<Workspace, FrontendError> {
        let (text, exact) = match selector {
            WorkspaceSelector::Caller => (self.context.get("CMUX_WORKSPACE_ID").ok_or_else(|| FrontendError::invalid("current CMUX targeting requires exact caller context; no focused-workspace fallback"))?.as_str(), true),
            WorkspaceSelector::Explicit(text) => (text, false),
            WorkspaceSelector::Exact(reference) => { check_ref(self, reference)?; (reference.key.as_str(), true) }
        };
        nonempty(text, "CMUX workspace selector")?;
        let mut op = Operation::default();
        let matches: Vec<_> = self
            .inventory(&mut op)?
            .into_iter()
            .filter(|v| {
                v.workspace.reference.key == text
                    || (!exact && (v.display_ref == text || v.workspace.title == text))
            })
            .collect();
        if matches.len() != 1 {
            return Err(op.error("CMUX workspace not found or ambiguous"));
        }
        Ok(matches.into_iter().next().unwrap().workspace)
    }
    fn create(&self, request: &CreateWorkspace) -> Result<Workspace, FrontendError> {
        self.create_session(request, None)
    }
    fn focus(&self, reference: &NativeRef) -> Result<(), FrontendError> {
        let mut op = Operation::default();
        self.target(&mut op, reference)?;
        op.run(
            &self.transport,
            &args(&["select-workspace", "--workspace", &reference.key]),
            true,
        )?;
        op.acknowledge();
        Ok(())
    }
    fn rename(&self, reference: &NativeRef, title: &str) -> Result<(), FrontendError> {
        nonempty(title, "workspace title")?;
        let mut op = Operation::default();
        let target = self.target(&mut op, reference)?;
        op.run(
            &self.transport,
            &args(&[
                "workspace-action",
                "--workspace",
                &target.display_ref,
                "--action",
                "rename",
                "--title",
                title,
            ]),
            true,
        )?;
        op.acknowledge();
        Ok(())
    }
}
