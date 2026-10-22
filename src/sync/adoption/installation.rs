//! Filesystem installation of an already-verified, durably staged Git directory.

use super::repair;
use crate::{
    file_io::sync_directory,
    sync::state::{RepairRequired, State},
};
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

/// Retains staging before the first possible fence effect, then moves only .git.
/// Git history and identity verification belong to the caller, not this phase.
pub(super) fn fence_and_move(
    state: &State<'_>,
    root: &Path,
    candidate: TempDir,
    evidence: Map<String, Value>,
) -> Result<PathBuf, RepairRequired> {
    // A failed write can already have replaced save.json. Disable Drop cleanup
    // BEFORE that first possible marker effect, not after write returns success.
    let retained = candidate.keep();
    state.write("save.json", evidence)?;
    fs::rename(retained.join(".git"), root.join(".git")).map_err(repair)?;
    sync_directory(root).map_err(repair)?;
    sync_directory(&retained).map_err(repair)?;
    Ok(retained)
}

#[cfg(test)]
mod tests;
