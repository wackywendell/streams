use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use strum::{EnumString, VariantNames};
use unicode_general_category::{GeneralCategory, get_general_category};

use super::super::{Result, invalid};

#[derive(Debug, Default, Clone, Copy, Serialize, EnumString, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
enum Startup {
    #[default]
    Automatic,
    Manual,
}
crate::codec::deserialize_string_enum!(Startup);

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Wire {
    workspace_startup: Startup,
    workspace_naming: bool,
    tab_naming: bool,
    session_naming: bool,
    document_command: Option<String>,
    herdr_timeout_ms: u32,
    pane_height_fraction: f64,
}
impl Default for Wire {
    fn default() -> Self {
        Self {
            workspace_startup: Startup::Automatic,
            workspace_naming: true,
            tab_naming: true,
            session_naming: true,
            document_command: None,
            herdr_timeout_ms: 3000,
            pane_height_fraction: 0.2,
        }
    }
}

#[derive(Clone)]
pub struct NativeCardsConfig {
    startup: Startup,
    pub(super) workspace_naming: bool,
    pub(super) tab_naming: bool,
    pub(super) session_naming: bool,
    pub(super) document_command: Option<String>,
    pub(super) timeout_seconds: f64,
    pub(super) pane_height_fraction: f64,
    pub(super) plugin_root: PathBuf,
    pub(super) source_path: PathBuf,
}
impl NativeCardsConfig {
    /// Ordinary user configuration may be Stow-managed; unlike launch evidence,
    /// its symlink target is trusted. A FIFO must still never block the UI worker.
    pub fn load(
        environment: &BTreeMap<String, String>,
        cwd: &Path,
        plugin_root: &Path,
    ) -> Result<Self> {
        if !cwd.is_absolute() {
            return Err(invalid("native reader cwd must be absolute"));
        }
        let source_path = if let Some(path) = environment.get("WORK_CARDS_CONFIG_PATH") {
            PathBuf::from(path)
        } else if let Some(root) = environment.get("PI_CODING_AGENT_DIR") {
            PathBuf::from(root).join("work-cards.json")
        } else {
            PathBuf::from(
                environment
                    .get("HOME")
                    .ok_or_else(|| invalid("HOME is required"))?,
            )
            .join(".pi/agent/work-cards.json")
        };
        let source_path = if source_path.is_absolute() {
            source_path
        } else {
            cwd.join(source_path)
        };
        let plugin_root = if plugin_root.is_absolute() {
            plugin_root.to_owned()
        } else {
            cwd.join(plugin_root)
        };
        let wire = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&source_path)
        {
            Ok(mut file) => {
                if !file.metadata()?.is_file() {
                    return Err(invalid("native configuration must be a regular file"));
                }
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)?;
                let value = crate::file_io::decode_json::<serde_json::Value>(&bytes)?;
                if !value.is_object() {
                    return Err(invalid("native configuration must be an object"));
                }
                serde_json::from_value::<Wire>(value).map_err(|e| invalid(e.to_string()))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Wire::default(),
            Err(error) => return Err(error.into()),
        };
        if !(1..=2_147_483_647).contains(&wire.herdr_timeout_ms)
            || !wire.pane_height_fraction.is_finite()
            || !(0.0..1.0).contains(&wire.pane_height_fraction)
            || wire.pane_height_fraction == 0.0
        {
            return Err(invalid("invalid native timer or pane height configuration"));
        }
        if let Some(command) = &wire.document_command {
            validate_title(command)?;
        }
        Ok(Self {
            startup: wire.workspace_startup,
            workspace_naming: wire.workspace_naming,
            tab_naming: wire.tab_naming,
            session_naming: wire.session_naming,
            document_command: wire.document_command,
            timeout_seconds: f64::from(wire.herdr_timeout_ms) / 1000.0,
            pane_height_fraction: wire.pane_height_fraction,
            plugin_root,
            source_path,
        })
    }
    pub fn session_naming(&self) -> bool {
        self.session_naming
    }
    pub fn timeout_seconds(&self) -> f64 {
        self.timeout_seconds
    }
    pub fn startup_automatic(&self) -> bool {
        matches!(self.startup, Startup::Automatic)
    }
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
    pub(super) fn document_command(
        &self,
        environment: &BTreeMap<String, String>,
    ) -> Result<String> {
        let command = self
            .document_command
            .as_deref()
            .or_else(|| {
                environment
                    .get("EDITOR")
                    .map(String::as_str)
                    .filter(|s| !s.trim().is_empty())
            })
            .unwrap_or("less");
        validate_title(command)?;
        Ok(command.into())
    }
}

pub(super) fn validate_title(value: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.starts_with('-')
        || value.chars().any(|c| {
            matches!(
                get_general_category(c),
                GeneralCategory::Control | GeneralCategory::Format
            )
        })
    {
        return Err(invalid("unsafe or blank native title/command"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retired_pane_poll_setting_is_rejected() {
        let error =
            serde_json::from_value::<Wire>(serde_json::json!({"pane_poll_interval_ms": 60000}))
                .err()
                .expect("retired polling setting must not be accepted");
        assert!(
            error
                .to_string()
                .contains("unknown field `pane_poll_interval_ms`")
        );
    }
    #[test]
    fn defaults_and_strict_configuration() {
        let root = tempfile::tempdir().unwrap();
        let env = BTreeMap::from([("HOME".into(), root.path().to_str().unwrap().into())]);
        let cfg = NativeCardsConfig::load(&env, root.path(), root.path()).unwrap();
        assert!(cfg.startup_automatic());
        assert_eq!(cfg.timeout_seconds(), 3.0);
        assert!(!root.path().join(".pi").exists());
        for input in [
            "[]",
            r#"{"workspace_startup":{"automatic":null}}"#,
            r#"{"herdr_timeout_ms":0}"#,
            r#"{"pane_height_fraction":1}"#,
            r#"{"workspace_naming":null}"#,
            r#"{"unknown":true}"#,
        ] {
            let path = root.path().join("config.json");
            std::fs::write(&path, input).unwrap();
            let mut env = env.clone();
            env.insert(
                "WORK_CARDS_CONFIG_PATH".into(),
                path.to_str().unwrap().into(),
            );
            assert!(
                NativeCardsConfig::load(&env, root.path(), root.path()).is_err(),
                "{input}"
            );
        }
    }
}
