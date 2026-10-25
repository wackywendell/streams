//! Address the existing host-local directory bindings. No native resource state.
use super::{Result, digest, invalid, text};
use crate::codec::ascii_json;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct RuntimeState {
    root: PathBuf,
    host_id: String,
}
impl RuntimeState {
    pub fn new(catalog_runtime_base: impl AsRef<Path>, host_id: impl Into<String>) -> Result<Self> {
        let host_id = host_id.into();
        text(&host_id)?;
        let base = std::path::absolute(catalog_runtime_base.as_ref())?;
        let base = if let Some(parent) = base.parent() {
            crate::config::resolve_path(parent)
                .map_err(|error| invalid(error.to_string()))?
                .join(
                    base.components()
                        .next_back()
                        .expect("absolute base component"),
                )
        } else {
            base
        };
        let key = digest(format!("[{}]", ascii_json(&host_id)).as_bytes());
        Ok(Self {
            root: base.join(key),
            host_id,
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn host_id(&self) -> &str {
        &self.host_id
    }
}
