//! Stream-addressed desired placement. Native metadata is its observed projection.

use super::*;
use crate::model::Stream;
use crate::runtime::native_state::NativeStateGuard;

/// The backend's actual binding capability, not a claim of Pi origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementBinding {
    workspace_id: String,
    kind: BindingKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum BindingKind {
    Herdr { nonce: String },
    Cmux { launch: Option<LaunchReference> },
}

impl PlacementBinding {
    pub fn herdr(workspace: impl Into<String>, nonce: impl Into<String>) -> Result<Self> {
        let workspace_id = workspace.into();
        let nonce = nonce.into();
        super::super::text(&workspace_id)?;
        super::super::text(&nonce)?;
        Ok(Self {
            workspace_id,
            kind: BindingKind::Herdr { nonce },
        })
    }

    pub fn cmux(workspace: impl Into<String>, launch: Option<LaunchReference>) -> Result<Self> {
        let workspace_id = workspace.into();
        super::super::text(&workspace_id)?;
        Ok(Self {
            workspace_id,
            kind: BindingKind::Cmux { launch },
        })
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace_id
    }

    pub fn nonce(&self) -> Option<&str> {
        match &self.kind {
            BindingKind::Herdr { nonce } => Some(nonce),
            BindingKind::Cmux { .. } => None,
        }
    }

    pub fn launch(&self) -> Option<&LaunchReference> {
        match &self.kind {
            BindingKind::Cmux { launch } => launch.as_ref(),
            BindingKind::Herdr { .. } => None,
        }
    }
}

/// One Stream-global row, including any active unsafe workspace attempt.
/// Presence is not readiness: native observations must still agree with its binding.
#[derive(Debug, Clone, PartialEq)]
pub struct NativePlacement {
    stream_id: String,
    scope: String,
    binding: Option<PlacementBinding>,
    pending: Option<PendingAttempt>,
}

impl NativePlacement {
    pub fn new(
        stream: impl Into<String>,
        scope: impl Into<String>,
        binding: PlacementBinding,
    ) -> Result<Self> {
        let stream_id = stream.into();
        let scope = scope.into();
        super::super::text(&stream_id)?;
        scope_text(&scope)?;
        Ok(Self {
            stream_id,
            scope,
            binding: Some(binding),
            pending: None,
        })
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn binding(&self) -> Option<&PlacementBinding> {
        self.binding.as_ref()
    }
    pub fn pending(&self) -> Option<&PendingAttempt> {
        self.pending.as_ref()
    }

    fn wire(&self) -> PlacementWire {
        let binding = self.binding.as_ref().map(|binding| match &binding.kind {
            BindingKind::Herdr { nonce } => BindingWire::Herdr {
                workspace_id: binding.workspace_id.clone(),
                nonce: nonce.clone(),
            },
            BindingKind::Cmux { launch } => BindingWire::Cmux {
                workspace_id: binding.workspace_id.clone(),
                session_id: launch.as_ref().map(|l| l.session_id.clone()),
                command: launch.as_ref().map(|l| l.command.clone()),
            },
        });
        PlacementWire {
            version: 2,
            stream_id: self.stream_id.clone(),
            scope: self.scope.clone(),
            binding,
            pending: self.pending.as_ref().map(|pending| PendingWire {
                session_id: pending.launch.as_ref().map(|l| l.session_id.clone()),
                command: pending.launch.as_ref().map(|l| l.command.clone()),
                cwd: pending.cwd.clone(),
                effects: pending.effects.clone(),
                launches: pending.launches.clone(),
            }),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlacementWire {
    version: u32,
    stream_id: String,
    scope: String,
    #[serde(deserialize_with = "required_nullable")]
    binding: Option<BindingWire>,
    #[serde(deserialize_with = "required_nullable")]
    pending: Option<PendingWire>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "frontend", rename_all = "lowercase", deny_unknown_fields)]
enum BindingWire {
    Herdr {
        workspace_id: String,
        nonce: String,
    },
    Cmux {
        workspace_id: String,
        #[serde(deserialize_with = "required_nullable")]
        session_id: Option<String>,
        #[serde(deserialize_with = "required_nullable")]
        command: Option<String>,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingWire {
    #[serde(deserialize_with = "required_nullable")]
    session_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    command: Option<String>,
    cwd: String,
    effects: Vec<Value>,
    launches: Vec<Value>,
}

impl PlacementWire {
    fn validated(self, path: &Path, stream: &str) -> Result<NativePlacement> {
        if self.version != 2 {
            return Err(invalid("unsupported native placement version"));
        }
        super::super::text(&self.stream_id)?;
        scope_text(&self.scope)?;
        filename(path, &self.stream_id)?;
        if self.stream_id != stream {
            return Err(invalid("native placement Stream differs from address"));
        }
        let binding = self
            .binding
            .map(|binding| match binding {
                BindingWire::Herdr {
                    workspace_id,
                    nonce,
                } => PlacementBinding::herdr(workspace_id, nonce),
                BindingWire::Cmux {
                    workspace_id,
                    session_id,
                    command,
                } => PlacementBinding::cmux(workspace_id, launch(session_id, command)?),
            })
            .transpose()?;
        let pending = self
            .pending
            .map(|pending| {
                state_text(&pending.cwd)?;
                Ok::<_, RuntimeError>(PendingAttempt {
                    stream_id: self.stream_id.clone(),
                    launch: launch(pending.session_id, pending.command)?,
                    cwd: pending.cwd,
                    workspace_id: binding.as_ref().map(|binding| binding.workspace_id.clone()),
                    effects: pending.effects,
                    launches: pending.launches,
                })
            })
            .transpose()?;
        if binding.is_none() && pending.is_none() {
            return Err(invalid(
                "native placement has neither binding nor pending attempt",
            ));
        }
        Ok(NativePlacement {
            stream_id: self.stream_id,
            scope: self.scope,
            binding,
            pending,
        })
    }
}

impl RuntimeState {
    /// Global within this admitted catalog/host, never partitioned by endpoint.
    pub fn placement_path(&self, stream: &str) -> Result<PathBuf> {
        super::super::text(stream)?;
        let path = self
            .root
            .join("placements")
            .join(format!("{}.json", key(&[stream])));
        self.check_directories(path.parent().expect("placement parent"))?;
        Ok(path)
    }

    fn read_placement(&self, stream: &str) -> Result<Option<NativePlacement>> {
        let path = self.placement_path(stream)?;
        read_json::<PlacementWire>(&path)?
            .map(|wire| wire.validated(&path, stream))
            .transpose()
    }

    /// Exact information only. A different endpoint is a fence, not a missing row.
    pub fn placement(&self, scope: &str, stream: &str) -> Result<Option<NativePlacement>> {
        scope_text(scope)?;
        let placement = self.read_placement(stream)?;
        if placement
            .as_ref()
            .is_some_and(|placement| placement.scope != scope)
        {
            return Err(invalid(
                "native placement belongs to another endpoint; fence retained",
            ));
        }
        Ok(placement)
    }

    /// Install an ordinary desired binding under the already-held native writer.
    /// This cannot retarget a placement or retire a loaded unsafe attempt.
    pub fn save_placement_binding(
        &self,
        _writer: &NativeStateGuard<'_>,
        stream: &Stream,
        expected: Option<&NativePlacement>,
        desired: &NativePlacement,
    ) -> Result<()> {
        if stream.owner_host_id != self.host_id {
            return Err(invalid(
                "native placement requires the Stream's fixed local writer",
            ));
        }
        if desired.stream_id != stream.id || expected.is_some_and(|old| old.stream_id != stream.id)
        {
            return Err(invalid("native placement Stream differs from saved record"));
        }
        if desired.pending.is_some()
            || desired.binding.is_none()
            || expected.is_some_and(|old| old.pending.is_some())
        {
            return Err(invalid(
                "pending native attempt cannot be replaced by binding; fence retained",
            ));
        }
        let current = self.read_placement(&stream.id)?;
        if current.as_ref() != expected {
            return Err(invalid("native placement changed; write refused"));
        }
        if let Some(current) = &current {
            if current.scope != desired.scope || current.binding != desired.binding {
                return Err(invalid(
                    "existing native placement cannot be retargeted by binding",
                ));
            }
            return Ok(());
        }
        let path = self.placement_path(&stream.id)?;
        self.writable(&path)?;
        write_json(&path, &desired.wire())
    }
}

#[cfg(test)]
mod tests;
