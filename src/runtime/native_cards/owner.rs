use super::*;
use crate::runtime::conversations::{
    Harness, NativeIdentityKind, NativeSessionMetadata, native_identity,
};
use serde::{Deserialize, Serialize};

/// Terminal identity locates Pi after a move; the native Session ID detects replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PiOwner {
    pub terminal_id: String,
    pub native_session_id: String,
}
impl PiOwner {
    /// Bootstrap from reported identity or an exact retained locator, without transcript discovery.
    pub fn select_initial(
        snapshot: &HerdrNativeSnapshot,
        caller: Option<&str>,
        native_id: &str,
        transcript: Option<&Path>,
    ) -> Result<Self> {
        let candidates: Vec<_> = snapshot
            .topology()
            .agents
            .iter()
            .filter(|agent| {
                let metadata = &agent.native["agent_session"];
                agent.native["agent"] == "pi"
                    && metadata["agent"] == "pi"
                    && ((metadata["kind"] == "id" && metadata["value"] == native_id)
                        || (metadata["kind"] == "path"
                            && transcript.and_then(Path::to_str) == metadata["value"].as_str()
                            && transcript.is_some()))
            })
            .collect();
        let pane = match candidates.as_slice() {
            [pane] => pane.pane_id.as_str(),
            [] => caller.ok_or_else(|| {
                invalid("Pi conversation has no reported identity or current caller")
            })?,
            panes => panes
                .iter()
                .find(|pane| Some(pane.pane_id.as_str()) == caller)
                .map(|pane| pane.pane_id.as_str())
                .ok_or_else(|| invalid("Pi conversation is reported by multiple terminals"))?,
        };
        Self::from_snapshot(snapshot, pane, native_id)
    }
    pub fn from_snapshot(
        snapshot: &HerdrNativeSnapshot,
        pane_id: &str,
        native_id: &str,
    ) -> Result<Self> {
        let pane = snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| pane.pane.id == pane_id)
            .ok_or_else(|| invalid("Pi pane is missing from the snapshot"))?;
        super::super::text(native_id)?;
        let owner = Self {
            terminal_id: pane.terminal_id.clone(),
            native_session_id: native_id.into(),
        };
        owner
            .locate(snapshot)?
            .ok_or_else(|| invalid("Pi pane reports a different conversation"))?;
        Ok(owner)
    }
    // Pure consumers reuse the top-level observation, without opening its transcript again.
    pub fn locate<'a>(
        &self,
        snapshot: &'a HerdrNativeSnapshot,
    ) -> Result<Option<&'a HerdrInventoryPane>> {
        let Some(pane) = snapshot
            .topology()
            .panes
            .iter()
            .find(|pane| pane.terminal_id == self.terminal_id)
        else {
            return Ok(None);
        };
        let Some(identity) = identity(snapshot, pane)? else {
            return Ok(None);
        };
        Ok(
            (identity.kind == NativeIdentityKind::Path || identity.value == self.native_session_id)
                .then_some(pane),
        )
    }
    /// Check an observed exact locator once, never search transcript directories.
    pub fn observe<'a>(
        &self,
        snapshot: &'a HerdrNativeSnapshot,
    ) -> Result<Option<(&'a HerdrInventoryPane, Option<PathBuf>)>> {
        let Some(pane) = self.locate(snapshot)? else {
            return Ok(None);
        };
        let identity = native_identity(
            &identity(snapshot, pane)?.expect("located Pi metadata"),
            Harness::Pi,
        )?;
        Ok((identity.id == self.native_session_id).then_some((pane, identity.locator)))
    }
}
fn identity(
    snapshot: &HerdrNativeSnapshot,
    pane: &HerdrInventoryPane,
) -> Result<Option<NativeSessionMetadata>> {
    let Some(agent) = snapshot
        .topology()
        .agents
        .iter()
        .find(|agent| agent.terminal_id == pane.terminal_id)
    else {
        return Ok(None);
    };
    if agent.native["agent"] != "pi" {
        return Ok(None);
    }
    let metadata: NativeSessionMetadata =
        crate::codec::object(agent.native["agent_session"].clone())
            .map_err(|error| invalid(format!("invalid Pi conversation metadata: {error}")))?;
    super::super::text(&metadata.value)?;
    if metadata.agent != Harness::Pi
        || (metadata.kind == NativeIdentityKind::Path && !Path::new(&metadata.value).is_absolute())
    {
        return Err(invalid("invalid reported Pi conversation"));
    }
    Ok(Some(metadata))
}
pub(super) fn location(pane: &HerdrInventoryPane) -> crate::tui::NativeOwnerObservation {
    crate::tui::NativeOwnerObservation {
        pane_id: pane.pane.id.clone(),
        tab_id: pane.pane.tab_id.clone(),
        workspace_id: pane.pane.workspace_id.clone(),
    }
}
