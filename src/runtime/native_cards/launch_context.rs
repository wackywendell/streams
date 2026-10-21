//! A complete launched-card envelope, parsed before it can select a caller role.
use super::*;
use crate::tui::NativeOwnerObservation;
use serde::{Deserialize, de::DeserializeOwned};

#[derive(Clone)]
pub struct LaunchedCardContext {
    scope: Scope,
    owner: NativeOwnerObservation,
    nonce: String,
    owner_terminal: String,
    state_directory: PathBuf,
    plugin_root: PathBuf,
    reader: SourceLaunch,
}
impl LaunchedCardContext {
    pub fn from_environment(environment: &BTreeMap<String, String>) -> Result<Option<Self>> {
        if ![
            "STREAMS_CARDS_SCOPE",
            "STREAMS_CARDS_OWNER",
            "STREAMS_CARDS_NONCE",
            "STREAMS_CARDS_OWNER_TERMINAL",
            "STREAMS_CARDS_READ_ENV",
            "STREAMS_CARDS_READ_CWD",
            "STREAMS_CARDS_PLUGIN_ROOT",
        ]
        .iter()
        .any(|key| environment.contains_key(*key))
        {
            return Ok(None);
        }
        let required = |key: &str| -> Result<String> {
            let value = environment.get(key).ok_or_else(|| {
                invalid(format!("incomplete launched-card context: missing {key}"))
            })?;
            super::super::text(value)?;
            if value.trim().is_empty() {
                return Err(invalid(format!("empty launched-card {key}")));
            }
            Ok(value.clone())
        };
        let wire: ScopeInput = object(&required("STREAMS_CARDS_SCOPE")?)?;
        for value in [
            &wire.catalog_source,
            &wire.host_id,
            &wire.stream_id,
            &wire.session_id,
            &wire.native_session_id,
        ] {
            super::super::text(value)?;
        }
        let owner: OwnerInput = object(&required("STREAMS_CARDS_OWNER")?)?;
        for value in [&owner.pane_id, &owner.tab_id, &owner.workspace_id] {
            super::super::text(value)?;
        }
        let captured: BTreeMap<String, String> = object(&required("STREAMS_CARDS_READ_ENV")?)?;
        if captured
            .keys()
            .any(|key| !SOURCE_ENV_KEYS.contains(&key.as_str()))
        {
            return Err(invalid("unsupported captured reader environment key"));
        }
        let absolute = |key: &str| -> Result<PathBuf> {
            let path = PathBuf::from(required(key)?);
            if !path.is_absolute() {
                return Err(invalid(format!("{key} must be absolute")));
            }
            Ok(path)
        };
        Ok(Some(Self {
            scope: Scope {
                catalog_source: wire.catalog_source,
                host_id: wire.host_id,
                stream_id: wire.stream_id,
                session_id: wire.session_id,
                harness: "pi".into(),
                native_session_id: wire.native_session_id,
            },
            owner: NativeOwnerObservation {
                pane_id: owner.pane_id,
                tab_id: owner.tab_id,
                workspace_id: owner.workspace_id,
            },
            nonce: required("STREAMS_CARDS_NONCE")?,
            owner_terminal: required("STREAMS_CARDS_OWNER_TERMINAL")?,
            state_directory: absolute("STREAMS_CARDS_STATE_DIR")?,
            plugin_root: absolute("STREAMS_CARDS_PLUGIN_ROOT")?,
            reader: SourceLaunch {
                environment: captured,
                cwd: absolute("STREAMS_CARDS_READ_CWD")?,
            },
        }))
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn owner(&self) -> &NativeOwnerObservation {
        &self.owner
    }
    pub fn nonce(&self) -> &str {
        &self.nonce
    }
    pub fn owner_terminal(&self) -> &str {
        &self.owner_terminal
    }
    pub fn state_directory(&self) -> &Path {
        &self.state_directory
    }
    pub fn plugin_root(&self) -> &Path {
        &self.plugin_root
    }
    pub fn reader(&self) -> &SourceLaunch {
        &self.reader
    }
}
fn object<T: DeserializeOwned>(input: &str) -> Result<T> {
    let value = crate::codec::load_json(input).map_err(|e| invalid(e.to_string()))?;
    if !value.is_object() {
        return Err(invalid("launched-card envelope fields must be objects"));
    }
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct ScopeInput {
    catalog_source: String,
    host_id: String,
    stream_id: String,
    session_id: String,
    native_session_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerInput {
    pane_id: String,
    tab_id: String,
    workspace_id: String,
}
