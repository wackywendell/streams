//! Strict host-local settings and identity primitives. No backend is opened here.
//!
//! Environment and working directory are explicit inputs; reads never create state.
//! Settings follow Stow-managed symlinks; host and lock files reject symlinks.
//! Writers share a non-reentrant durable config lock.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::{CStr, CString};
use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use strum::{AsRefStr, Display, EnumIter, EnumString, VariantNames};
use uuid::Uuid;

use crate::codec::load_json;
use crate::file_io::{self, check_catalog_root};
use crate::frontend::{FrontendId, FrontendMode};
use crate::storage::{Journal, StorageError};
use crate::sync::policy::Policy;
use crate::sync::state::{RepairRequired, State};

#[cfg(test)]
mod tests;

/// The supported persistent keys; backend selection lives in the catalog marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString, Display, AsRefStr, EnumIter)]
pub enum ConfigKey {
    #[strum(serialize = "catalog.root")]
    CatalogRoot,
    #[strum(serialize = "frontend.mode")]
    FrontendMode,
    #[strum(serialize = "sync.stream_seconds")]
    StreamSeconds,
    #[strum(serialize = "sync.session_seconds")]
    SessionSeconds,
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

/// Effective settings; policy-only transport limits retain their existing defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub catalog_root: PathBuf,
    pub frontend: FrontendMode,
    pub sync: Policy,
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
            config_path: config_home.join("streams/config.json"),
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

    /// Runtime identity is the resolved catalog path, not frontend selection or inode.
    pub fn runtime_state_dir(&self) -> Result<PathBuf> {
        self.runtime_state_dir_for(&self.load()?.catalog_root)
    }

    /// Bind runtime state to an already-open catalog instead of reloading mutable settings.
    pub fn runtime_state_dir_for(&self, root: &Path) -> Result<PathBuf> {
        let root = resolve_path(root)?;
        Ok(self.state_home.join("streams/catalogs").join(format!(
            "{:x}",
            Sha256::digest(path_text(&root)?.as_bytes())
        )))
    }

    /// Return only configured values after validating the entire nested schema.
    pub fn read_overrides(&self) -> Result<Value> {
        let path = resolve_path(&self.config_path)?;
        let data = read_json(&path)?;
        if data.is_none() && self.config_path.is_symlink() {
            return Err(invalid("broken config symlink"));
        }
        self.validate_overrides(data.unwrap_or_else(|| json!({})))
    }

    fn validate_overrides(&self, mut data: Value) -> Result<Value> {
        let object = data
            .as_object_mut()
            .ok_or_else(|| invalid("config must contain a JSON object"))?;
        for (section, values) in object {
            if !matches!(section.as_str(), "catalog" | "frontend" | "sync") {
                return Err(invalid(format!("invalid config section: {section}")));
            }
            for (name, value) in values
                .as_object_mut()
                .ok_or_else(|| invalid("config sections must be objects"))?
            {
                let wire_key = format!("{section}.{name}");
                let key = wire_key
                    .parse::<ConfigKey>()
                    .map_err(|_| invalid(format!("unknown streams config key: {wire_key}")))?;
                *value = self.validate_value(key, value.clone())?;
            }
        }
        Ok(data)
    }

    pub fn load(&self) -> Result<Settings> {
        let data = self.read_overrides()?;
        let root = data
            .pointer("/catalog/root")
            .cloned()
            .unwrap_or_else(|| json!(self.data_home.join("streams/catalog")));
        let root = self.validate_value(ConfigKey::CatalogRoot, root)?;
        let mode = data
            .pointer("/frontend/mode")
            .and_then(Value::as_str)
            .unwrap_or("auto");
        let default = Policy::default();
        let sync = Policy::new(
            data.pointer("/sync/stream_seconds")
                .and_then(Value::as_f64)
                .unwrap_or(default.stream_seconds()),
            data.pointer("/sync/session_seconds")
                .and_then(Value::as_f64)
                .unwrap_or(default.session_seconds()),
            default.cooldown_seconds(),
            default.timeout_seconds(),
            default.attempts(),
        )
        .map_err(invalid)?;
        Ok(Settings {
            catalog_root: PathBuf::from(root.as_str().expect("validated string")),
            frontend: match mode {
                "auto" => FrontendMode::Auto,
                "none" => FrontendMode::None,
                name => FrontendMode::Explicit(FrontendId(name.into())),
            },
            sync,
        })
    }

    /// Effective settings in the existing Python JSON schema, with no transport-only keys.
    pub fn values(&self) -> Result<Value> {
        let settings = self.load()?;
        let mode = match settings.frontend {
            FrontendMode::Auto => "auto".into(),
            FrontendMode::None => "none".into(),
            FrontendMode::Explicit(id) => id.0,
        };
        Ok(
            json!({"catalog":{"root":settings.catalog_root}, "frontend":{"mode":mode},
            "sync":{"stream_seconds":settings.sync.stream_seconds(),"session_seconds":settings.sync.session_seconds()}}),
        )
    }

    pub fn get(&self, key: ConfigKey) -> Result<Value> {
        Ok(self
            .values()?
            .pointer(&format!("/{}", key.as_ref().replace('.', "/")))
            .expect("known key")
            .clone())
    }

    /// Validate before locking, then atomically preserve every other configured key.
    pub fn set(&self, key: ConfigKey, text: &str) -> Result<()> {
        let value = self.parse_value(key, text)?;
        self.with_lock(|lock| lock.set_value(key, value))
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

    fn parse_value(&self, key: ConfigKey, text: &str) -> Result<Value> {
        let value = match key {
            ConfigKey::StreamSeconds | ConfigKey::SessionSeconds => {
                load_json(text).map_err(|e| invalid(e.to_string()))?
            }
            ConfigKey::CatalogRoot | ConfigKey::FrontendMode => json!(text),
        };
        self.validate_value(key, value)
    }

    fn validate_value(&self, key: ConfigKey, value: Value) -> Result<Value> {
        let text = || {
            value
                .as_str()
                .filter(|v| !v.trim().is_empty() && !v.contains('\0'))
                .ok_or_else(|| {
                    invalid(format!(
                        "{key} must be a nonempty string without NUL characters"
                    ))
                })
        };
        match key {
            ConfigKey::StreamSeconds | ConfigKey::SessionSeconds => {
                if !value.as_f64().is_some_and(|v| v.is_finite() && v >= 0.0) {
                    return Err(invalid(format!(
                        "{key} must be a finite nonnegative number"
                    )));
                }
                Ok(value)
            }
            ConfigKey::FrontendMode => {
                text()?;
                Ok(value)
            }
            ConfigKey::CatalogRoot => {
                let path = expanduser(text()?, &self.home)?;
                if !path.is_absolute() {
                    return Err(invalid(
                        "catalog.root must be an absolute path (or start with ~/)",
                    ));
                }
                Ok(json!(path_text(&path)?))
            }
        }
    }
}

/// A held exclusive config lock. Do not reacquire it through Config inside this scope.
pub struct ConfigLock<'a> {
    config: &'a Config,
    _file: File,
}
impl ConfigLock<'_> {
    pub fn settings(&self) -> Result<Settings> {
        self.config.load()
    }
    pub fn existing_host_identity(&self) -> Result<HostIdentity> {
        self.config.host_identity()
    }
    pub fn set(&mut self, key: ConfigKey, text: &str) -> Result<()> {
        let value = self.config.parse_value(key, text)?;
        self.set_value(key, value)
    }
    fn set_value(&mut self, key: ConfigKey, value: Value) -> Result<()> {
        let mut data = self.config.read_overrides()?;
        let (section, name) = key.as_ref().split_once('.').expect("validated key");
        let object = data.as_object_mut().expect("validated object");
        object
            .entry(section)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .expect("validated section")
            .insert(name.into(), value);
        write_json(&resolve_path(&self.config.config_path)?, &data)
    }

    /// Create only for an unmarked catalog, keeping this guard held through later
    /// catalog-marker decisions. Never repair missing or malformed host state.
    pub fn host_identity_for_new_catalog(&mut self) -> Result<HostIdentity> {
        if let Some(identity) = self.config.read_host()? {
            return Ok(identity);
        }
        let root = self.settings()?.catalog_root;
        check_catalog_root(&root)?;
        if Marker::exists(&root)? {
            return Err(missing_host());
        }
        let identity = HostIdentity(Uuid::new_v4().to_string());
        write_json(
            &self.config.host_path(),
            &json!({"version":1,"id":identity.as_str()}),
        )?;
        Ok(identity)
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

/// Host-bound initialization evidence read under the caller's store transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub strategy: Strategy,
    pub host: HostIdentity,
}
impl Marker {
    /// Inspect before acquiring the catalog lock; malformed markers never permit host creation.
    pub fn exists(root: &Path) -> Result<bool> {
        match fs::symlink_metadata(root.join(".catalog-sync/cli.json")) {
            Ok(metadata) if metadata.is_file() => Ok(true),
            Ok(_) => Err(invalid(
                "catalog initialization marker must be a regular file",
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub fn write(&self, state: &State<'_>) -> std::result::Result<(), RepairRequired> {
        state.write(
            "cli.json",
            json!({"strategy": self.strategy, "host": self.host.as_str()})
                .as_object()
                .expect("object")
                .clone(),
        )
    }

    pub fn read(journal: &dyn Journal) -> Result<Option<Self>> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            version: u64,
            strategy: Strategy,
            host: String,
        }
        journal
            .read("cli.json")?
            .map(|data| {
                let wire: Wire = serde_json::from_value(Value::Object(data))
                    .map_err(|e| invalid(format!("invalid catalog initialization marker: {e}")))?;
                if wire.version != 1 {
                    return Err(invalid("unsupported catalog initialization marker version"));
                }
                Ok(Self {
                    strategy: wire.strategy,
                    host: HostIdentity::parse(&wire.host)?,
                })
            })
            .transpose()
    }
    pub fn check_host(&self, identity: &HostIdentity) -> Result<()> {
        if &self.host != identity {
            return Err(invalid(
                "catalog belongs to a different host; restore the matching private host identity",
            ));
        }
        Ok(())
    }
}

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

/// Bind host-local requests to the resolved directory, device, inode and producer ID.
/// The preimage is Python json.dumps with ASCII escaping and comma-space separators.
pub fn source_id(root: &Path, host: &HostIdentity) -> Result<String> {
    let root = resolve_path(root)?;
    let stat = fs::metadata(&root)?;
    if !stat.is_dir() {
        return Err(invalid("catalog source must be a directory"));
    }
    let preimage = format!(
        "[{}, {}, {}, {}]",
        crate::codec::ascii_json(path_text(&root)?),
        stat.dev(),
        stat.ino(),
        crate::codec::ascii_json(host.as_str())
    );
    Ok(format!("{:x}", Sha256::digest(preimage.as_bytes())))
}
