//! Strict host-local settings and identity primitives. No backend is opened here.
//!
//! Environment and working directory are explicit inputs; reads never create state.
//! Settings follow Stow-managed symlinks; host and lock files reject symlinks.
//! Writers share a non-reentrant durable config lock.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::{CStr, CString};
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use strum::{AsRefStr, Display, EnumIter, EnumString, VariantNames};
use unicode_general_category::{GeneralCategory, get_general_category};
use uuid::Uuid;

use crate::file_io;
use crate::frontend::{FrontendId, FrontendMode};
use crate::model::{NamingMode, TitleDefaults};
use crate::storage::StorageError;

#[cfg(test)]
mod tests;

/// Settable and unsettable dotted paths in `config.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString, Display, AsRefStr, EnumIter)]
pub enum ConfigKey {
    #[strum(serialize = "catalog.root")]
    CatalogRoot,
    #[strum(serialize = "frontend.mode")]
    FrontendMode,
    #[strum(serialize = "naming.stream")]
    StreamNaming,
    #[strum(serialize = "naming.session")]
    SessionNaming,
    #[strum(serialize = "lifecycle.decay_days")]
    LifecycleDecayDays,
    #[strum(serialize = "herdr.viewer.mode")]
    HerdrViewerMode,
    #[strum(serialize = "herdr.viewer.placement")]
    HerdrViewerPlacement,
    #[strum(serialize = "herdr.naming.tab")]
    HerdrNamingTab,
    #[strum(serialize = "herdr.naming.workspace")]
    HerdrNamingWorkspace,
    #[strum(serialize = "herdr.cards.auto_open")]
    HerdrCardsAutoOpen,
    #[strum(serialize = "herdr.timeout_ms")]
    HerdrTimeoutMs,
    #[strum(serialize = "herdr.pane_height_fraction")]
    HerdrPaneHeightFraction,
    #[strum(serialize = "pi.naming.session")]
    PiNamingSession,
    #[strum(serialize = "pi.naming.import_edits")]
    PiNamingImportEdits,
    #[strum(serialize = "pi.reporting.mode")]
    PiReportingMode,
    #[strum(serialize = "pi.reporting.enrollment")]
    PiReportingEnrollment,
    #[strum(serialize = "pi.reporting.frequency")]
    PiReportingFrequency,
    #[strum(serialize = "pi.reporting.context")]
    PiReportingContext,
    #[strum(serialize = "pi.reporting.model")]
    PiReportingModel,
    #[strum(serialize = "pi.reporting.model.provider")]
    PiReportingModelProvider,
    #[strum(serialize = "pi.reporting.model.id")]
    PiReportingModelId,
    #[strum(serialize = "pi.reporting.session_logging")]
    PiReportingSessionLogging,
    #[strum(serialize = "pi.reporting.debug_commands")]
    PiReportingDebugCommands,
}

/// Readable config paths (tables or leaves) accepted by `streams config get`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString, Display, AsRefStr, EnumIter)]
pub enum ConfigSection {
    #[strum(serialize = "catalog")]
    Catalog,
    #[strum(serialize = "frontend")]
    Frontend,
    #[strum(serialize = "naming")]
    Naming,
    #[strum(serialize = "lifecycle")]
    Lifecycle,
    #[strum(serialize = "herdr")]
    Herdr,
    #[strum(serialize = "herdr.viewer")]
    HerdrViewer,
    #[strum(serialize = "herdr.naming")]
    HerdrNaming,
    #[strum(serialize = "herdr.cards")]
    HerdrCards,
    #[strum(serialize = "pi")]
    Pi,
    #[strum(serialize = "pi.naming")]
    PiNaming,
    #[strum(serialize = "pi.reporting")]
    PiReporting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPath {
    Section(ConfigSection),
    Key(ConfigKey),
}

impl std::str::FromStr for ConfigPath {
    type Err = ConfigError;

    fn from_str(s: &str) -> Result<Self> {
        if let Ok(key) = s.parse::<ConfigKey>() {
            return Ok(Self::Key(key));
        }
        if let Ok(section) = s.parse::<ConfigSection>() {
            return Ok(Self::Section(section));
        }
        Err(invalid(format!("unknown streams config key: {s}")))
    }
}

impl std::fmt::Display for ConfigPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Section(section) => write!(f, "{section}"),
            Self::Key(key) => write!(f, "{key}"),
        }
    }
}

/// Validation and filesystem failures retain uncertain-write evidence from storage.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

type Result<T> = std::result::Result<T, ConfigError>;
fn invalid(message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid(message.into())
}

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    EnumString,
    VariantNames,
    Display,
    AsRefStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ViewerMode {
    #[default]
    Editor,
    Less,
    Hunk,
    Native,
}
crate::codec::deserialize_string_enum!(ViewerMode);

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    EnumString,
    VariantNames,
    Display,
    AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ViewerPlacement {
    #[default]
    PaneRight,
    PaneDown,
    Tab,
    Floating,
}
crate::codec::deserialize_string_enum!(ViewerPlacement);

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    EnumString,
    VariantNames,
    Display,
    AsRefStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ReportingContext {
    #[default]
    Trimmed,
    Full,
}
crate::codec::deserialize_string_enum!(ReportingContext);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReportingModel {
    pub provider: String,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HerdrSettings {
    pub viewer_mode: ViewerMode,
    pub viewer_placement: ViewerPlacement,
    pub tab_naming: bool,
    pub workspace_naming: bool,
    pub cards_auto_open: CardMode,
    pub timeout_ms: u32,
    pub pane_height_fraction: f64,
}

impl Default for HerdrSettings {
    fn default() -> Self {
        Self {
            viewer_mode: ViewerMode::Editor,
            viewer_placement: ViewerPlacement::PaneRight,
            tab_naming: true,
            workspace_naming: true,
            cards_auto_open: CardMode::Automatic,
            timeout_ms: 3000,
            pane_height_fraction: 0.2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PiSettings {
    pub session_naming: bool,
    pub import_edits: bool,
    pub reporting_mode: NamingMode,
    pub enrollment: NamingMode,
    pub frequency: u32,
    pub context: ReportingContext,
    pub model: Option<ReportingModel>,
    pub session_logging: bool,
    pub debug_commands: bool,
}

impl Default for PiSettings {
    fn default() -> Self {
        Self {
            session_naming: true,
            import_edits: true,
            reporting_mode: NamingMode::Automatic,
            enrollment: NamingMode::Automatic,
            frequency: 1,
            context: ReportingContext::Trimmed,
            model: None,
            session_logging: false,
            debug_commands: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleSettings {
    pub decay_days: u32,
}

impl Default for LifecycleSettings {
    fn default() -> Self {
        Self { decay_days: 14 }
    }
}

/// Effective host-local settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub catalog_root: PathBuf,
    pub frontend: FrontendMode,
    pub naming: TitleDefaults,
    pub lifecycle: LifecycleSettings,
    pub herdr: HerdrSettings,
    pub pi: PiSettings,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum CardMode {
    #[default]
    Inherit,
    Automatic,
    Manual,
}
crate::codec::deserialize_string_enum!(CardMode);

impl CardMode {
    pub fn resolve(self, global: Self) -> Self {
        if self == Self::Inherit { global } else { self }
    }
}

/// An immutable environment snapshot and the paths derived from it.
#[derive(Debug, Clone)]
pub struct Config {
    home: PathBuf,
    config_path: PathBuf,
    data_home: PathBuf,
    state_home: PathBuf,
}

impl Config {
    /// Resolve HOME/XDG values without consulting or mutating the process environment.
    /// Relative config/state paths use the supplied absolute working directory.
    /// A relative XDG_DATA_HOME default fails settings validation unless root is overridden.
    pub fn new(environment: &BTreeMap<String, String>, cwd: &Path) -> Result<Self> {
        if !cwd.is_absolute() {
            return Err(invalid("working directory must be absolute"));
        }
        let home = environment
            .get("HOME")
            .filter(|v| !v.is_empty())
            .ok_or_else(|| invalid("HOME must be supplied explicitly"))?;
        let home = PathBuf::from(home);
        if !home.is_absolute() || home.as_os_str().as_encoded_bytes().contains(&0) {
            return Err(invalid(
                "HOME must be an absolute path without NUL characters",
            ));
        }
        let xdg = |name: &str, fallback: PathBuf| -> Result<PathBuf> {
            let path = environment
                .get(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or(fallback);
            if path.as_os_str().as_encoded_bytes().contains(&0) {
                return Err(invalid("XDG paths must not contain NUL characters"));
            }
            Ok(if path.is_absolute() {
                path
            } else {
                cwd.join(path)
            })
        };
        let config_home = xdg("XDG_CONFIG_HOME", home.join(".config"))?;
        let data_home = environment
            .get("XDG_DATA_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/share"));
        Ok(Self {
            state_home: xdg("XDG_STATE_HOME", home.join(".local/state"))?,
            config_path: config_home.join("streams/config.toml"),
            data_home,
            home,
        })
    }

    pub fn config_path(&self) -> &Path {
        &self.config_path
    }
    pub fn host_path(&self) -> PathBuf {
        self.config_path.with_file_name("host.json")
    }
    pub fn checkout_state_dir(&self) -> PathBuf {
        self.state_home.join("streams/checkouts")
    }

    /// Bind runtime state to an already-open catalog instead of reloading mutable settings.
    pub fn runtime_state_dir_for(&self, root: &Path) -> Result<PathBuf> {
        let root = resolve_path(root)?;
        Ok(self.state_home.join("streams/catalogs").join(format!(
            "{:x}",
            Sha256::digest(path_text(&root)?.as_bytes())
        )))
    }

    /// Return only configured values from `config.toml` after validating the entire nested schema.
    pub fn read_overrides_toml(&self) -> Result<toml::Table> {
        let path = resolve_path(&self.config_path)?;
        let bytes = file_io::read_bytes(&path)?;
        let Some(bytes) = bytes else {
            if self.config_path.is_symlink() {
                return Err(invalid("broken config symlink"));
            }
            return Ok(toml::Table::new());
        };
        let text =
            std::str::from_utf8(&bytes).map_err(|_| invalid("config.toml must be valid UTF-8"))?;
        let table: toml::Table = toml::from_str(text).map_err(|e| {
            invalid(format!(
                "invalid TOML in {}: {e}",
                self.config_path.display()
            ))
        })?;
        self.validate_overrides_toml(table)
    }

    /// Return only configured values as JSON for callers and existing tests.
    pub fn read_overrides(&self) -> Result<Value> {
        toml_table_to_json(&self.read_overrides_toml()?)
    }

    fn validate_overrides_toml(&self, mut table: toml::Table) -> Result<toml::Table> {
        let sections: Vec<String> = table.keys().cloned().collect();
        for section in sections {
            let value = table.get_mut(&section).expect("key exists");
            let subtable = value
                .as_table_mut()
                .ok_or_else(|| invalid(format!("config section {section} must be a table")))?;
            match section.as_str() {
                "catalog" => {
                    for (name, v) in subtable.iter_mut() {
                        let key = format!("catalog.{name}")
                            .parse::<ConfigKey>()
                            .map_err(|_| {
                                invalid(format!("unknown streams config key: catalog.{name}"))
                            })?;
                        *v = self.validate_toml_value(key, v.clone())?;
                    }
                }
                "frontend" => {
                    for (name, v) in subtable.iter_mut() {
                        let key =
                            format!("frontend.{name}")
                                .parse::<ConfigKey>()
                                .map_err(|_| {
                                    invalid(format!("unknown streams config key: frontend.{name}"))
                                })?;
                        *v = self.validate_toml_value(key, v.clone())?;
                    }
                }
                "naming" => {
                    for (name, v) in subtable.iter_mut() {
                        let key = format!("naming.{name}").parse::<ConfigKey>().map_err(|_| {
                            invalid(format!("unknown streams config key: naming.{name}"))
                        })?;
                        *v = self.validate_toml_value(key, v.clone())?;
                    }
                }
                "lifecycle" => {
                    for (name, v) in subtable.iter_mut() {
                        let key =
                            format!("lifecycle.{name}")
                                .parse::<ConfigKey>()
                                .map_err(|_| {
                                    invalid(format!("unknown streams config key: lifecycle.{name}"))
                                })?;
                        *v = self.validate_toml_value(key, v.clone())?;
                    }
                }
                "herdr" => self.validate_herdr_table(subtable)?,
                "pi" => self.validate_pi_table(subtable)?,
                _ => return Err(invalid(format!("invalid config section: {section}"))),
            }
        }
        Ok(table)
    }

    fn validate_herdr_table(&self, table: &mut toml::Table) -> Result<()> {
        let keys: Vec<String> = table.keys().cloned().collect();
        for key_name in keys {
            let entry = table.get_mut(&key_name).expect("key exists");
            match key_name.as_str() {
                "viewer" | "naming" | "cards" => {
                    let sub = entry.as_table_mut().ok_or_else(|| {
                        invalid(format!("config section herdr.{key_name} must be a table"))
                    })?;
                    for (leaf, v) in sub.iter_mut() {
                        let wire = format!("herdr.{key_name}.{leaf}");
                        let key = wire
                            .parse::<ConfigKey>()
                            .map_err(|_| invalid(format!("unknown streams config key: {wire}")))?;
                        *v = self.validate_toml_value(key, v.clone())?;
                    }
                }
                "timeout_ms" => {
                    *entry = self.validate_toml_value(ConfigKey::HerdrTimeoutMs, entry.clone())?;
                }
                "pane_height_fraction" => {
                    *entry = self
                        .validate_toml_value(ConfigKey::HerdrPaneHeightFraction, entry.clone())?;
                }
                _ => {
                    return Err(invalid(format!(
                        "unknown streams config key: herdr.{key_name}"
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_pi_table(&self, table: &mut toml::Table) -> Result<()> {
        let keys: Vec<String> = table.keys().cloned().collect();
        for key_name in keys {
            let entry = table.get_mut(&key_name).expect("key exists");
            match key_name.as_str() {
                "naming" => {
                    let sub = entry
                        .as_table_mut()
                        .ok_or_else(|| invalid("config section pi.naming must be a table"))?;
                    for (leaf, v) in sub.iter_mut() {
                        let wire = format!("pi.naming.{leaf}");
                        let key = wire
                            .parse::<ConfigKey>()
                            .map_err(|_| invalid(format!("unknown streams config key: {wire}")))?;
                        *v = self.validate_toml_value(key, v.clone())?;
                    }
                }
                "reporting" => {
                    let sub = entry
                        .as_table_mut()
                        .ok_or_else(|| invalid("config section pi.reporting must be a table"))?;
                    for (leaf, v) in sub.iter_mut() {
                        let wire = format!("pi.reporting.{leaf}");
                        let key = wire
                            .parse::<ConfigKey>()
                            .map_err(|_| invalid(format!("unknown streams config key: {wire}")))?;
                        *v = self.validate_toml_value(key, v.clone())?;
                    }
                }
                _ => {
                    return Err(invalid(format!(
                        "unknown streams config key: pi.{key_name}"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn load(&self) -> Result<Settings> {
        let data = self.read_overrides_toml()?;
        let root_val = match lookup_toml(&data, "catalog.root") {
            Some(v) => v.clone(),
            None => toml::Value::String(path_text(&self.data_home.join("streams/catalog"))?.into()),
        };
        let root_val = self.validate_toml_value(ConfigKey::CatalogRoot, root_val)?;
        let frontend = match lookup_toml(&data, "frontend.mode")
            .and_then(toml::Value::as_str)
            .unwrap_or("auto")
        {
            "auto" => FrontendMode::Auto,
            "none" => FrontendMode::None,
            name => FrontendMode::Explicit(
                name.parse()
                    .map_err(|_| invalid("frontend.mode must be auto, none, herdr, cmux or hc"))?,
            ),
        };

        let stream_naming = lookup_toml(&data, "naming.stream")
            .and_then(toml::Value::as_str)
            .map(str::parse)
            .transpose()
            .map_err(|_| invalid("invalid naming.stream"))?
            .unwrap_or(NamingMode::Automatic);
        let session_naming = lookup_toml(&data, "naming.session")
            .and_then(toml::Value::as_str)
            .map(str::parse)
            .transpose()
            .map_err(|_| invalid("invalid naming.session"))?
            .unwrap_or(NamingMode::Automatic);

        let lifecycle_defaults = LifecycleSettings::default();
        let lifecycle = LifecycleSettings {
            decay_days: lookup_toml(&data, "lifecycle.decay_days")
                .and_then(toml::Value::as_integer)
                .map(|v| v as u32)
                .unwrap_or(lifecycle_defaults.decay_days),
        };

        let herdr_defaults = HerdrSettings::default();
        let herdr = HerdrSettings {
            viewer_mode: lookup_toml(&data, "herdr.viewer.mode")
                .and_then(toml::Value::as_str)
                .map(str::parse)
                .transpose()
                .map_err(|_| invalid("invalid herdr.viewer.mode"))?
                .unwrap_or(herdr_defaults.viewer_mode),
            viewer_placement: lookup_toml(&data, "herdr.viewer.placement")
                .and_then(toml::Value::as_str)
                .map(str::parse)
                .transpose()
                .map_err(|_| invalid("invalid herdr.viewer.placement"))?
                .unwrap_or(herdr_defaults.viewer_placement),
            tab_naming: lookup_toml(&data, "herdr.naming.tab")
                .and_then(toml::Value::as_bool)
                .unwrap_or(herdr_defaults.tab_naming),
            workspace_naming: lookup_toml(&data, "herdr.naming.workspace")
                .and_then(toml::Value::as_bool)
                .unwrap_or(herdr_defaults.workspace_naming),
            cards_auto_open: lookup_toml(&data, "herdr.cards.auto_open")
                .and_then(toml::Value::as_str)
                .map(str::parse)
                .transpose()
                .map_err(|_| invalid("invalid herdr.cards.auto_open"))?
                .unwrap_or(herdr_defaults.cards_auto_open),
            timeout_ms: lookup_toml(&data, "herdr.timeout_ms")
                .and_then(toml::Value::as_integer)
                .map(|v| v as u32)
                .unwrap_or(herdr_defaults.timeout_ms),
            pane_height_fraction: lookup_toml(&data, "herdr.pane_height_fraction")
                .and_then(toml::Value::as_float)
                .unwrap_or(herdr_defaults.pane_height_fraction),
        };

        let pi_defaults = PiSettings::default();
        let model = match lookup_toml(&data, "pi.reporting.model") {
            Some(value) => {
                let table = value.as_table().expect("validated model table");
                Some(ReportingModel {
                    provider: table["provider"].as_str().expect("validated").into(),
                    id: table["id"].as_str().expect("validated").into(),
                })
            }
            None => None,
        };
        let pi = PiSettings {
            session_naming: lookup_toml(&data, "pi.naming.session")
                .and_then(toml::Value::as_bool)
                .unwrap_or(pi_defaults.session_naming),
            import_edits: lookup_toml(&data, "pi.naming.import_edits")
                .and_then(toml::Value::as_bool)
                .unwrap_or(pi_defaults.import_edits),
            reporting_mode: lookup_toml(&data, "pi.reporting.mode")
                .and_then(toml::Value::as_str)
                .map(str::parse)
                .transpose()
                .map_err(|_| invalid("invalid pi.reporting.mode"))?
                .unwrap_or(pi_defaults.reporting_mode),
            enrollment: lookup_toml(&data, "pi.reporting.enrollment")
                .and_then(toml::Value::as_str)
                .map(str::parse)
                .transpose()
                .map_err(|_| invalid("invalid pi.reporting.enrollment"))?
                .unwrap_or(pi_defaults.enrollment),
            frequency: lookup_toml(&data, "pi.reporting.frequency")
                .and_then(toml::Value::as_integer)
                .map(|v| v as u32)
                .unwrap_or(pi_defaults.frequency),
            context: lookup_toml(&data, "pi.reporting.context")
                .and_then(toml::Value::as_str)
                .map(str::parse)
                .transpose()
                .map_err(|_| invalid("invalid pi.reporting.context"))?
                .unwrap_or(pi_defaults.context),
            model,
            session_logging: lookup_toml(&data, "pi.reporting.session_logging")
                .and_then(toml::Value::as_bool)
                .unwrap_or(pi_defaults.session_logging),
            debug_commands: lookup_toml(&data, "pi.reporting.debug_commands")
                .and_then(toml::Value::as_bool)
                .unwrap_or(pi_defaults.debug_commands),
        };

        Ok(Settings {
            catalog_root: PathBuf::from(root_val.as_str().expect("validated string")),
            frontend,
            naming: TitleDefaults {
                stream: stream_naming,
                session: session_naming,
            },
            lifecycle,
            herdr,
            pi,
        })
    }

    /// Effective settings as a TOML table, including defaults for unspecified keys.
    pub fn values_toml(&self) -> Result<toml::Table> {
        let settings = self.load()?;
        let mode = match settings.frontend {
            FrontendMode::Auto => "auto".into(),
            FrontendMode::None => "none".into(),
            FrontendMode::Explicit(id) => id.to_string(),
        };
        let mut root = toml::Table::new();

        let mut catalog = toml::Table::new();
        catalog.insert(
            "root".into(),
            toml::Value::String(path_text(&settings.catalog_root)?.into()),
        );
        root.insert("catalog".into(), toml::Value::Table(catalog));

        let mut frontend = toml::Table::new();
        frontend.insert("mode".into(), toml::Value::String(mode));
        root.insert("frontend".into(), toml::Value::Table(frontend));

        let mut naming = toml::Table::new();
        naming.insert(
            "stream".into(),
            toml::Value::String(settings.naming.stream.as_ref().into()),
        );
        naming.insert(
            "session".into(),
            toml::Value::String(settings.naming.session.as_ref().into()),
        );
        root.insert("naming".into(), toml::Value::Table(naming));

        let mut lifecycle = toml::Table::new();
        lifecycle.insert(
            "decay_days".into(),
            toml::Value::Integer(i64::from(settings.lifecycle.decay_days)),
        );
        root.insert("lifecycle".into(), toml::Value::Table(lifecycle));

        let mut herdr = toml::Table::new();
        let mut viewer = toml::Table::new();
        viewer.insert(
            "mode".into(),
            toml::Value::String(settings.herdr.viewer_mode.as_ref().into()),
        );
        viewer.insert(
            "placement".into(),
            toml::Value::String(settings.herdr.viewer_placement.as_ref().into()),
        );
        herdr.insert("viewer".into(), toml::Value::Table(viewer));

        let mut herdr_naming = toml::Table::new();
        herdr_naming.insert(
            "tab".into(),
            toml::Value::Boolean(settings.herdr.tab_naming),
        );
        herdr_naming.insert(
            "workspace".into(),
            toml::Value::Boolean(settings.herdr.workspace_naming),
        );
        herdr.insert("naming".into(), toml::Value::Table(herdr_naming));

        let mut cards = toml::Table::new();
        cards.insert(
            "auto_open".into(),
            toml::Value::String(match settings.herdr.cards_auto_open {
                CardMode::Automatic | CardMode::Inherit => "automatic".into(),
                CardMode::Manual => "manual".into(),
            }),
        );
        herdr.insert("cards".into(), toml::Value::Table(cards));
        herdr.insert(
            "timeout_ms".into(),
            toml::Value::Integer(i64::from(settings.herdr.timeout_ms)),
        );
        herdr.insert(
            "pane_height_fraction".into(),
            toml::Value::Float(settings.herdr.pane_height_fraction),
        );
        root.insert("herdr".into(), toml::Value::Table(herdr));

        let mut pi = toml::Table::new();
        let mut pi_naming = toml::Table::new();
        pi_naming.insert(
            "session".into(),
            toml::Value::Boolean(settings.pi.session_naming),
        );
        pi_naming.insert(
            "import_edits".into(),
            toml::Value::Boolean(settings.pi.import_edits),
        );
        pi.insert("naming".into(), toml::Value::Table(pi_naming));

        let mut reporting = toml::Table::new();
        reporting.insert(
            "mode".into(),
            toml::Value::String(settings.pi.reporting_mode.as_ref().into()),
        );
        reporting.insert(
            "enrollment".into(),
            toml::Value::String(settings.pi.enrollment.as_ref().into()),
        );
        reporting.insert(
            "frequency".into(),
            toml::Value::Integer(i64::from(settings.pi.frequency)),
        );
        reporting.insert(
            "context".into(),
            toml::Value::String(settings.pi.context.as_ref().into()),
        );
        if let Some(model) = &settings.pi.model {
            let mut m = toml::Table::new();
            m.insert(
                "provider".into(),
                toml::Value::String(model.provider.clone()),
            );
            m.insert("id".into(), toml::Value::String(model.id.clone()));
            reporting.insert("model".into(), toml::Value::Table(m));
        }
        reporting.insert(
            "session_logging".into(),
            toml::Value::Boolean(settings.pi.session_logging),
        );
        reporting.insert(
            "debug_commands".into(),
            toml::Value::Boolean(settings.pi.debug_commands),
        );
        pi.insert("reporting".into(), toml::Value::Table(reporting));
        root.insert("pi".into(), toml::Value::Table(pi));

        Ok(root)
    }

    /// Effective settings as JSON, including defaults for unspecified keys.
    pub fn values(&self) -> Result<Value> {
        toml_table_to_json(&self.values_toml()?)
    }

    /// Snapshot exposed to `extensions/stream-checkpoint` via `GetConfig`/`SetConfig`/`UnsetConfig`.
    pub fn extension_snapshot(&self) -> Result<Value> {
        let settings = self.load()?;
        Ok(json!({
            "naming": {
                "stream": settings.naming.stream,
                "session": settings.naming.session,
            },
            "herdr": {
                "viewer": {
                    "mode": settings.herdr.viewer_mode,
                    "placement": settings.herdr.viewer_placement,
                },
                "naming": {
                    "tab": settings.herdr.tab_naming,
                    "workspace": settings.herdr.workspace_naming,
                },
                "cards": {
                    "auto_open": match settings.herdr.cards_auto_open {
                        CardMode::Automatic | CardMode::Inherit => "automatic",
                        CardMode::Manual => "manual",
                    },
                },
                "timeout_ms": settings.herdr.timeout_ms,
                "pane_height_fraction": settings.herdr.pane_height_fraction,
            },
            "pi": {
                "naming": {
                    "session": settings.pi.session_naming,
                    "import_edits": settings.pi.import_edits,
                },
                "reporting": {
                    "mode": settings.pi.reporting_mode,
                    "enrollment": settings.pi.enrollment,
                    "frequency": settings.pi.frequency,
                    "context": settings.pi.context,
                    "model": settings.pi.model,
                    "session_logging": settings.pi.session_logging,
                    "debug_commands": settings.pi.debug_commands,
                },
            },
        }))
    }

    pub fn get(&self, key: ConfigKey) -> Result<Value> {
        self.get_path(ConfigPath::Key(key))
    }

    pub fn get_path_toml(&self, path: ConfigPath) -> Result<toml::Value> {
        let table = self.values_toml()?;
        let dotted = path.to_string();
        lookup_toml(&table, &dotted)
            .cloned()
            .ok_or_else(|| invalid(format!("{dotted} is not set")))
    }

    pub fn get_path(&self, path: ConfigPath) -> Result<Value> {
        toml_value_to_json(&self.get_path_toml(path)?)
    }

    /// Validate before locking, then atomically preserve every other configured key.
    pub fn set(&self, key: ConfigKey, text: &str) -> Result<()> {
        let value = self.parse_toml_value(key, text)?;
        self.with_lock(|lock| lock.set_toml_value(key, value))
    }

    /// Remove a key or optional table from `config.toml` under lock so it reverts to default.
    pub fn unset(&self, key: ConfigKey) -> Result<()> {
        if matches!(
            key,
            ConfigKey::PiReportingModelProvider | ConfigKey::PiReportingModelId
        ) {
            return Err(invalid(
                "cannot unset only one field of pi.reporting.model; unset pi.reporting.model instead",
            ));
        }
        self.with_lock(|lock| lock.unset_key(key))
    }

    /// Acquire the non-reentrant writer lock. Do not call locking Config methods
    /// while holding this guard; use the guard's methods instead.
    pub fn lock(&self) -> Result<ConfigLock<'_>> {
        let directory = self.config_path.parent().expect("config parent");
        let file = file_io::lock(&directory.join(".catalog-config.lock"))?;
        Ok(ConfigLock {
            config: self,
            _file: file,
        })
    }

    /// Keep host and later catalog-marker decisions inside this same lock scope.
    pub fn with_lock<T>(&self, f: impl FnOnce(&mut ConfigLock<'_>) -> Result<T>) -> Result<T> {
        f(&mut self.lock()?)
    }

    /// Read the existing identity without creating state or acquiring the writer lock.
    pub fn host_identity(&self) -> Result<HostIdentity> {
        self.read_host()?.ok_or_else(missing_host)
    }

    fn read_host(&self) -> Result<Option<HostIdentity>> {
        read_json(&self.host_path())?
            .map(|data| {
                let wire: HostWire = serde_json::from_value(data).map_err(|e| {
                    invalid(format!(
                        "invalid host identity: {e}; restore host.json, do not regenerate it"
                    ))
                })?;
                if wire.version != 1 {
                    return Err(invalid("unsupported host identity version"));
                }
                HostIdentity::parse(&wire.id)
            })
            .transpose()
    }

    fn parse_toml_value(&self, key: ConfigKey, text: &str) -> Result<toml::Value> {
        let value = match key {
            ConfigKey::HerdrNamingTab
            | ConfigKey::HerdrNamingWorkspace
            | ConfigKey::PiNamingSession
            | ConfigKey::PiNamingImportEdits
            | ConfigKey::PiReportingSessionLogging
            | ConfigKey::PiReportingDebugCommands => match text {
                "true" => toml::Value::Boolean(true),
                "false" => toml::Value::Boolean(false),
                _ => return Err(invalid(format!("{key} must be true or false"))),
            },
            ConfigKey::LifecycleDecayDays => {
                let parsed: i64 = text
                    .parse()
                    .map_err(|_| invalid("lifecycle.decay_days must be an integer >= 0"))?;
                toml::Value::Integer(parsed)
            }
            ConfigKey::HerdrTimeoutMs => {
                let parsed: i64 = text
                    .parse()
                    .map_err(|_| invalid("herdr.timeout_ms must be a positive integer"))?;
                toml::Value::Integer(parsed)
            }
            ConfigKey::PiReportingFrequency => {
                let parsed: i64 = text
                    .parse()
                    .map_err(|_| invalid("pi.reporting.frequency must be an integer >= 1"))?;
                toml::Value::Integer(parsed)
            }
            ConfigKey::HerdrPaneHeightFraction => {
                let parsed: f64 = text.parse().map_err(|_| {
                    invalid("herdr.pane_height_fraction must be a number between 0 and 1")
                })?;
                toml::Value::Float(parsed)
            }
            ConfigKey::PiReportingModel => {
                let Some((provider, id)) = text.split_once('/') else {
                    return Err(invalid(
                        "pi.reporting.model must be formatted as <provider>/<id>",
                    ));
                };
                let mut table = toml::Table::new();
                table.insert("provider".into(), toml::Value::String(provider.into()));
                table.insert("id".into(), toml::Value::String(id.into()));
                toml::Value::Table(table)
            }
            _ => toml::Value::String(text.into()),
        };
        self.validate_toml_value(key, value)
    }

    fn validate_toml_value(&self, key: ConfigKey, value: toml::Value) -> Result<toml::Value> {
        fn text(key: ConfigKey, v: &toml::Value) -> Result<&str> {
            v.as_str()
                .filter(|s| !s.trim().is_empty() && !s.contains('\0'))
                .ok_or_else(|| {
                    invalid(format!(
                        "{key} must be a nonempty string without NUL characters"
                    ))
                })
        }
        match key {
            ConfigKey::FrontendMode => {
                let mode = text(key, &value)?;
                if !matches!(mode, "auto" | "none") && mode.parse::<FrontendId>().is_err() {
                    return Err(invalid(format!(
                        "{key} must be auto, none, herdr, cmux or hc"
                    )));
                }
                Ok(value)
            }
            ConfigKey::StreamNaming
            | ConfigKey::SessionNaming
            | ConfigKey::PiReportingMode
            | ConfigKey::PiReportingEnrollment => {
                text(key, &value)?
                    .parse::<NamingMode>()
                    .map_err(|_| invalid(format!("{key} must be automatic or manual")))?;
                Ok(value)
            }
            ConfigKey::HerdrCardsAutoOpen => match text(key, &value)?.parse::<CardMode>() {
                Ok(CardMode::Automatic | CardMode::Manual) => Ok(value),
                _ => Err(invalid("herdr.cards.auto_open must be automatic or manual")),
            },
            ConfigKey::CatalogRoot => {
                let path = expanduser(text(key, &value)?, &self.home)?;
                if !path.is_absolute() {
                    return Err(invalid(
                        "catalog.root must be an absolute path (or start with ~/)",
                    ));
                }
                Ok(toml::Value::String(path_text(&path)?.into()))
            }
            ConfigKey::HerdrViewerMode => {
                text(key, &value)?.parse::<ViewerMode>().map_err(|_| {
                    invalid("herdr.viewer.mode must be editor, less, hunk, or native")
                })?;
                Ok(value)
            }
            ConfigKey::HerdrViewerPlacement => {
                text(key, &value)?.parse::<ViewerPlacement>().map_err(|_| {
                    invalid(
                        "herdr.viewer.placement must be pane-right, pane-down, tab, or floating",
                    )
                })?;
                Ok(value)
            }
            ConfigKey::HerdrNamingTab
            | ConfigKey::HerdrNamingWorkspace
            | ConfigKey::PiNamingSession
            | ConfigKey::PiNamingImportEdits
            | ConfigKey::PiReportingSessionLogging
            | ConfigKey::PiReportingDebugCommands => {
                if !value.is_bool() {
                    return Err(invalid(format!("{key} must be a boolean")));
                }
                Ok(value)
            }
            ConfigKey::LifecycleDecayDays => {
                let Some(n) = value.as_integer() else {
                    return Err(invalid("lifecycle.decay_days must be an integer"));
                };
                if n < 0 || u32::try_from(n).is_err() {
                    return Err(invalid("lifecycle.decay_days must be an integer >= 0"));
                }
                Ok(value)
            }
            ConfigKey::HerdrTimeoutMs => {
                let Some(n) = value.as_integer() else {
                    return Err(invalid("herdr.timeout_ms must be an integer"));
                };
                if !(1..=2_147_483_647).contains(&n) {
                    return Err(invalid(
                        "herdr.timeout_ms must be an integer between 1 and 2147483647",
                    ));
                }
                Ok(value)
            }
            ConfigKey::HerdrPaneHeightFraction => {
                let Some(f) = value.as_float() else {
                    return Err(invalid("herdr.pane_height_fraction must be a float"));
                };
                if !(f.is_finite() && 0.0 < f && f < 1.0) {
                    return Err(invalid(
                        "herdr.pane_height_fraction must be strictly between 0 and 1",
                    ));
                }
                Ok(value)
            }
            ConfigKey::PiReportingFrequency => {
                let Some(n) = value.as_integer() else {
                    return Err(invalid("pi.reporting.frequency must be an integer"));
                };
                if n < 1 || u32::try_from(n).is_err() {
                    return Err(invalid("pi.reporting.frequency must be an integer >= 1"));
                }
                Ok(value)
            }
            ConfigKey::PiReportingContext => {
                text(key, &value)?
                    .parse::<ReportingContext>()
                    .map_err(|_| invalid("pi.reporting.context must be trimmed or full"))?;
                Ok(value)
            }
            ConfigKey::PiReportingModel => {
                let table = value.as_table().ok_or_else(|| {
                    invalid("pi.reporting.model must be a table with provider and id")
                })?;
                for k in table.keys() {
                    if k != "provider" && k != "id" {
                        return Err(invalid(format!(
                            "unknown streams config key: pi.reporting.model.{k}"
                        )));
                    }
                }
                let provider = table
                    .get("provider")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| invalid("pi.reporting.model requires both provider and id"))?;
                let id = table
                    .get("id")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| invalid("pi.reporting.model requires both provider and id"))?;
                validate_model_part("pi.reporting.model.provider", provider)?;
                validate_model_part("pi.reporting.model.id", id)?;
                Ok(value)
            }
            ConfigKey::PiReportingModelProvider | ConfigKey::PiReportingModelId => {
                let s = value
                    .as_str()
                    .ok_or_else(|| invalid(format!("{key} must be a non-empty string")))?;
                validate_model_part(key.as_ref(), s)?;
                Ok(value)
            }
        }
    }
}

fn validate_model_part(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.chars().any(|c| {
            matches!(
                get_general_category(c),
                GeneralCategory::Control | GeneralCategory::Format
            )
        })
    {
        return Err(invalid(format!(
            "{field} must be a non-empty string without leading/trailing whitespace or control characters"
        )));
    }
    Ok(())
}

/// A held exclusive config lock. Do not reacquire it through Config inside this scope.
pub struct ConfigLock<'a> {
    config: &'a Config,
    _file: file_io::FileLock,
}
impl ConfigLock<'_> {
    pub fn settings(&self) -> Result<Settings> {
        self.config.load()
    }
    pub fn set(&mut self, key: ConfigKey, text: &str) -> Result<()> {
        let value = self.config.parse_toml_value(key, text)?;
        self.set_toml_value(key, value)
    }
    fn set_toml_value(&mut self, key: ConfigKey, value: toml::Value) -> Result<()> {
        let mut data = self.config.read_overrides_toml()?;
        if matches!(
            key,
            ConfigKey::PiReportingModelProvider | ConfigKey::PiReportingModelId
        ) && lookup_toml(&data, "pi.reporting.model").is_none()
        {
            return Err(invalid(
                "pi.reporting.model is unset; set pi.reporting.model <provider>/<id> first",
            ));
        }
        insert_toml(&mut data, key.as_ref(), value);
        let validated = self.config.validate_overrides_toml(data)?;
        let encoded = encode_toml(&validated)?;
        file_io::write_text(&resolve_path(&self.config.config_path)?, &encoded)?;
        Ok(())
    }
    fn unset_key(&mut self, key: ConfigKey) -> Result<()> {
        let mut data = self.config.read_overrides_toml()?;
        let removed = remove_toml(&mut data, key.as_ref());
        if !removed {
            return Ok(());
        }
        let validated = self.config.validate_overrides_toml(data)?;
        let encoded = encode_toml(&validated)?;
        file_io::write_text(&resolve_path(&self.config.config_path)?, &encoded)?;
        Ok(())
    }

    /// Create only for an unmarked catalog, keeping this guard held through later
    /// catalog-marker decisions. Never repair missing or malformed host state.
    pub fn host_identity_for_new_catalog(&mut self) -> Result<HostIdentity> {
        if let Some(identity) = self.config.read_host()? {
            return Ok(identity);
        }
        let root = self.settings()?.catalog_root;
        if root.exists() {
            for entry in fs::read_dir(&root)? {
                let entry = entry?;
                if entry.file_name() != ".catalog.lock" || !entry.file_type()?.is_file() {
                    return Err(missing_host());
                }
            }
        }
        let identity = HostIdentity(Uuid::new_v4().to_string());
        write_json(
            &self.config.host_path(),
            &json!({"version":1,"id":identity.as_str()}),
        )?;
        Ok(identity)
    }
}

pub fn encode_toml(table: &toml::Table) -> Result<String> {
    if table.is_empty() {
        return Ok(String::new());
    }
    let mut rendered =
        toml::to_string_pretty(table).map_err(|e| invalid(format!("cannot encode TOML: {e}")))?;
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn lookup_toml<'a>(table: &'a toml::Table, dotted: &str) -> Option<&'a toml::Value> {
    let mut parts = dotted.split('.');
    let first = parts.next()?;
    let mut current = table.get(first)?;
    for part in parts {
        current = current.as_table()?.get(part)?;
    }
    Some(current)
}

fn insert_toml(table: &mut toml::Table, dotted: &str, value: toml::Value) {
    let parts: Vec<&str> = dotted.split('.').collect();
    let mut current = table;
    for part in &parts[..parts.len() - 1] {
        let entry = current
            .entry((*part).to_owned())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        current = entry.as_table_mut().expect("validated parent table");
    }
    current.insert((*parts.last().expect("non-empty key")).to_owned(), value);
}

fn remove_toml(table: &mut toml::Table, dotted: &str) -> bool {
    let parts: Vec<&str> = dotted.split('.').collect();
    remove_toml_parts(table, &parts)
}

fn remove_toml_parts(table: &mut toml::Table, parts: &[&str]) -> bool {
    if parts.is_empty() {
        return false;
    }
    if parts.len() == 1 {
        return table.remove(parts[0]).is_some();
    }
    let Some(toml::Value::Table(child)) = table.get_mut(parts[0]) else {
        return false;
    };
    let removed = remove_toml_parts(child, &parts[1..]);
    if removed && child.is_empty() {
        table.remove(parts[0]);
    }
    removed
}

pub fn toml_table_to_json(table: &toml::Table) -> Result<Value> {
    let mut map = serde_json::Map::new();
    for (k, v) in table {
        map.insert(k.clone(), toml_value_to_json(v)?);
    }
    Ok(Value::Object(map))
}

pub fn toml_value_to_json(value: &toml::Value) -> Result<Value> {
    match value {
        toml::Value::String(s) => Ok(Value::String(s.clone())),
        toml::Value::Integer(n) => Ok(json!(n)),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .ok_or_else(|| invalid("non-finite float in TOML")),
        toml::Value::Boolean(b) => Ok(Value::Bool(*b)),
        toml::Value::Table(t) => toml_table_to_json(t),
        toml::Value::Array(_) | toml::Value::Datetime(_) => {
            Err(invalid("unsupported TOML value type"))
        }
    }
}

/// Canonical lowercase, hyphenated UUID. Malformed existing IDs are never normalized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostIdentity(String);
impl HostIdentity {
    pub fn parse(value: &str) -> Result<Self> {
        let id = Uuid::parse_str(value)
            .map_err(|_| invalid("host identity must be a canonical UUID"))?;
        if id.to_string() != value {
            return Err(invalid("host identity must be a canonical UUID"));
        }
        Ok(Self(value.into()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HostWire {
    version: u64,
    id: String,
}

/// Existing version-one marker strategies; no backend is constructed by this module.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum Strategy {
    None,
    Git,
}

crate::codec::deserialize_string_enum!(Strategy);

fn missing_host() -> ConfigError {
    invalid(
        "host identity is missing; run streams init for a new catalog; restore host.json for an existing catalog",
    )
}
fn read_json(path: &Path) -> Result<Option<Value>> {
    Ok(file_io::read_json(path)?)
}
fn write_json(path: &Path, value: &Value) -> Result<()> {
    Ok(file_io::write_json(path, value)?)
}
fn path_text(path: &Path) -> Result<&str> {
    path.to_str().ok_or_else(|| invalid("path must be UTF-8"))
}

fn expanduser(value: &str, home: &Path) -> Result<PathBuf> {
    if value == "~" {
        return Ok(home.to_path_buf());
    }
    if let Some(rest) = value.strip_prefix("~/") {
        let tail = rest.trim_start_matches('/');
        return Ok(if tail.is_empty() {
            home.to_path_buf()
        } else {
            home.join(tail)
        });
    }
    if let Some(rest) = value.strip_prefix('~') {
        let (name, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let name = CString::new(name).map_err(|_| invalid("invalid user name"))?;
        let mut size = 4096;
        loop {
            let mut buffer = vec![0u8; size];
            let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
            let mut found = std::ptr::null_mut();
            // The reentrant lookup does not use shared passwd storage or HOME.
            let rc = unsafe {
                libc::getpwnam_r(
                    name.as_ptr(),
                    entry.as_mut_ptr(),
                    buffer.as_mut_ptr().cast(),
                    size,
                    &mut found,
                )
            };
            if rc == libc::ERANGE {
                size *= 2;
                continue;
            }
            if rc != 0 || found.is_null() {
                return Err(invalid("cannot resolve home directory"));
            }
            let directory = unsafe { CStr::from_ptr((*found).pw_dir) }
                .to_str()
                .map_err(|_| invalid("home directory must be UTF-8"))?;
            let tail = tail.trim_start_matches('/');
            return Ok(if tail.is_empty() {
                PathBuf::from(directory)
            } else {
                Path::new(directory).join(tail)
            });
        }
    }
    Ok(PathBuf::from(value))
}

/// Python Path.resolve(strict=False): resolve symlinks before `..`, preserving missing tails.
/// std::fs::canonicalize alone rejects non-existing trailing components.
pub fn resolve_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("path to resolve must be absolute"));
    }
    enum Pending {
        Part(std::ffi::OsString),
        EndLink(PathBuf),
    }
    let mut pending: VecDeque<_> = path
        .components()
        .map(|c| Pending::Part(c.as_os_str().to_owned()))
        .collect();
    let mut resolved = PathBuf::from("/");
    let mut active_links = BTreeSet::new();
    while let Some(item) = pending.pop_front() {
        let part = match item {
            Pending::Part(part) => part,
            Pending::EndLink(link) => {
                active_links.remove(&link);
                continue;
            }
        };
        match Path::new(&part).components().next() {
            Some(Component::RootDir | Component::CurDir) => continue,
            Some(Component::ParentDir) => {
                resolved.pop();
                continue;
            }
            _ => {}
        }
        let candidate = resolved.join(&part);
        match fs::symlink_metadata(&candidate) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if !active_links.insert(candidate.clone()) {
                    return Err(invalid("symlink loop while resolving path"));
                }
                let target = fs::read_link(&candidate)?;
                pending.push_front(Pending::EndLink(candidate));
                if target.is_absolute() {
                    resolved = PathBuf::from("/");
                }
                for component in target.components().rev() {
                    pending.push_front(Pending::Part(component.as_os_str().to_owned()));
                }
            }
            Ok(_) => resolved.push(part),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                resolved.push(part)
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(resolved)
}

/// Bind requests to the configured producer and canonical catalog path.
/// Replacing a directory's contents does not change this logical identity.
pub fn source_id(root: &Path, host: &HostIdentity) -> Result<String> {
    let root = resolve_path(root)?;
    let stat = fs::metadata(&root)?;
    if !stat.is_dir() {
        return Err(invalid("catalog source must be a directory"));
    }
    let preimage = format!(
        "[{}, {}]",
        crate::codec::ascii_json(path_text(&root)?),
        crate::codec::ascii_json(host.as_str())
    );
    Ok(format!("{:x}", Sha256::digest(preimage.as_bytes())))
}
