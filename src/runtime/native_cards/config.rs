use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use unicode_general_category::{GeneralCategory, get_general_category};

use super::super::{Result, invalid};
use crate::config::{Config, Settings};

#[derive(Clone)]
pub struct NativeCardsConfig {
    automatic_open: bool,
    pub(super) workspace_naming: bool,
    pub(super) tab_naming: bool,
    pub(super) session_naming: bool,
    pub(super) timeout_seconds: f64,
    pub(super) pane_height_fraction: f64,
    pub(super) plugin_root: PathBuf,
    pub(super) source_path: PathBuf,
}

impl NativeCardsConfig {
    /// Load Herdr card settings from the unified TOML configuration.
    pub fn load(
        environment: &BTreeMap<String, String>,
        cwd: &Path,
        plugin_root: &Path,
    ) -> Result<Self> {
        if !cwd.is_absolute() {
            return Err(invalid("native reader cwd must be absolute"));
        }
        let config = Config::new(environment, cwd).map_err(|e| invalid(e.to_string()))?;
        let settings = config.load().map_err(|e| invalid(e.to_string()))?;
        Self::from_settings(&settings, config.config_path(), cwd, plugin_root)
    }

    /// Build Herdr card settings from settings that have already been loaded.
    pub fn from_settings(
        settings: &Settings,
        source_path: &Path,
        cwd: &Path,
        plugin_root: &Path,
    ) -> Result<Self> {
        if !cwd.is_absolute() {
            return Err(invalid("native reader cwd must be absolute"));
        }
        let plugin_root = if plugin_root.is_absolute() {
            plugin_root.to_owned()
        } else {
            cwd.join(plugin_root)
        };
        Ok(Self {
            automatic_open: settings.herdr.cards_auto_open != crate::config::CardMode::Manual,
            workspace_naming: settings.herdr.workspace_naming,
            tab_naming: settings.herdr.tab_naming,
            session_naming: settings.pi.session_naming,
            timeout_seconds: f64::from(settings.herdr.timeout_ms) / 1000.0,
            pane_height_fraction: settings.herdr.pane_height_fraction,
            plugin_root,
            source_path: source_path.to_path_buf(),
        })
    }

    pub fn session_naming(&self) -> bool {
        self.session_naming
    }
    pub fn tab_naming(&self) -> bool {
        self.tab_naming
    }
    pub fn timeout_seconds(&self) -> f64 {
        self.timeout_seconds
    }
    pub fn startup_automatic(&self) -> bool {
        self.automatic_open
    }
    pub fn with_auto_open(mut self, mode: crate::config::CardMode) -> Self {
        self.automatic_open = mode == crate::config::CardMode::Automatic;
        self
    }
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }
    pub(super) fn document_command(
        &self,
        environment: &BTreeMap<String, String>,
    ) -> Result<String> {
        let command = environment
            .get("EDITOR")
            .map(String::as_str)
            .filter(|s| !s.trim().is_empty())
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
    fn defaults_and_strict_configuration() {
        let root = tempfile::tempdir().unwrap();
        let env = BTreeMap::from([("HOME".into(), root.path().to_str().unwrap().into())]);
        let cfg = NativeCardsConfig::load(&env, root.path(), root.path()).unwrap();
        assert!(cfg.startup_automatic());
        assert_eq!(cfg.timeout_seconds(), 3.0);
        assert!(!root.path().join(".pi").exists());
        let config_dir = root.path().join(".config/streams");
        std::fs::create_dir_all(&config_dir).unwrap();
        for input in [
            "herdr = 1\n",
            "[herdr]\ntimeout_ms = 0\n",
            "[herdr]\npane_height_fraction = 1.0\n",
            "[herdr.naming]\nworkspace = \"yes\"\n",
            "[herdr]\nunknown = true\n",
        ] {
            std::fs::write(config_dir.join("config.toml"), input).unwrap();
            assert!(
                NativeCardsConfig::load(&env, root.path(), root.path()).is_err(),
                "{input}"
            );
        }
    }
}
