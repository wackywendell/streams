//! Bookkeeping after a verified physical import; journal failures never imply rollback.

#[cfg(test)]
mod tests;

use super::repair;
use crate::config::{HostIdentity, Marker, Strategy};
use crate::sync::state::{GitConfig, RepairRequired, State};
use serde_json::{Map, Value};
use std::fmt::Display;

pub(super) fn finish(
    state: &State<'_>,
    config: &GitConfig,
    evidence: Map<String, Value>,
    retained: impl Display,
    current_config: impl FnOnce() -> Result<GitConfig, RepairRequired>,
) -> Result<(), RepairRequired> {
    state.write_config(config)?;
    if current_config()? != *config {
        return Err(repair("installed Git configuration changed"));
    }
    Marker {
        strategy: Strategy::Git,
        host: HostIdentity::parse(config.host()).map_err(repair)?,
    }
    .write(state)?;
    if let Err(error) = state.clear("save.json") {
        // Unlink may already have happened. Reassert the original fence, not a rollback.
        if let Err(reassert) = state.write("save.json", evidence) {
            return Err(repair(format!(
                "{error}; cannot reassert import fence: {reassert}; inspect retained staging at {retained}"
            )));
        }
        return Err(error);
    }
    Ok(())
}
