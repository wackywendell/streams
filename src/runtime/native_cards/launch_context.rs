//! Plain launch inputs; no native addresses, nonce or launch receipt.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchedCardContext {
    pub scope: Scope,
    pub owner: PiOwner,
    pub reader: SourceLaunch,
    pub lock_directory: PathBuf,
    pub plugin_root: PathBuf,
}
impl LaunchedCardContext {
    pub fn from_environment(environment: &BTreeMap<String, String>) -> Result<Option<Self>> {
        let Some(raw) = environment.get("STREAMS_CARDS_CONTEXT") else {
            return Ok(None);
        };
        let context: Self = crate::file_io::decode_json(raw.as_bytes())?;
        for value in [
            &context.scope.catalog_source,
            &context.scope.host_id,
            &context.scope.native_session_id,
            &context.scope.stream_id,
            &context.scope.session_id,
            &context.owner.terminal_id,
            &context.owner.native_session_id,
        ] {
            super::super::text(value)?;
        }
        if context.scope.harness != "pi"
            || context.owner.native_session_id != context.scope.native_session_id
            || context.scope.session_id != context.scope.native_session_id
            || !context.reader.cwd.is_absolute()
            || !context.lock_directory.is_absolute()
            || !context.plugin_root.is_absolute()
            || context
                .reader
                .environment
                .keys()
                .any(|key| !SOURCE_ENV_KEYS.contains(&key.as_str()))
        {
            return Err(invalid("invalid card launch inputs"));
        }
        Ok(Some(context))
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn owner_terminal(&self) -> &str {
        &self.owner.terminal_id
    }
    pub fn plugin_root(&self) -> &Path {
        &self.plugin_root
    }
    pub fn reader(&self) -> &SourceLaunch {
        &self.reader
    }
}
