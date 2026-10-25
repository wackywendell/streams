//! Identity-verified observations from the current native snapshot, never saved UI state.

use std::collections::BTreeSet;

use crate::frontend::herdr::{HerdrAgent, HerdrTopology};

use super::conversations::{Harness, NativeIdentity, NativeSessionMetadata, native_identity};
use super::{Result, decode, invalid};

/// A snapshot observation only; this does not authorize native effects.
#[derive(Debug)]
pub struct VerifiedAgent<'a> {
    pub native: &'a HerdrAgent,
    pub harness: Harness,
    pub identity: NativeIdentity,
}

/// Verify the requested scope. `None` checks every supported host conversation.
pub fn verified_inventory<'a>(
    topology: &'a HerdrTopology,
    workspace: Option<&str>,
) -> Result<Vec<VerifiedAgent<'a>>> {
    let mut verified = Vec::new();
    let mut seen = BTreeSet::new();
    for native in &topology.agents {
        if workspace.is_some_and(|id| native.workspace_id != id) {
            continue;
        }
        let Some(harness) = native
            .native
            .get("agent")
            .and_then(|value| value.as_str())
            .and_then(|name| name.parse::<Harness>().ok())
        else {
            continue;
        };
        let identity = (|| {
            let metadata = native
                .native
                .get("agent_session")
                .ok_or_else(|| invalid("missing native agent_session"))?;
            let metadata: NativeSessionMetadata =
                decode(&serde_json::to_vec(metadata).map_err(|error| invalid(error.to_string()))?)?;
            native_identity(&metadata, harness)
        })()
        .map_err(|error| {
            invalid(format!(
                "Herdr pane {} (workspace {}, tab {}): {error}",
                native.pane_id, native.workspace_id, native.tab_id,
            ))
        })?;
        if !seen.insert((harness.to_string(), identity.id.clone())) {
            return Err(invalid("ambiguous live native conversation identity"));
        }
        verified.push(VerifiedAgent {
            native,
            harness,
            identity,
        });
    }
    Ok(verified)
}
