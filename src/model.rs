//! Typed records with strict wire field mapping. Deserialization checks wire shapes;
//! record validation checks local constraints, and catalog construction checks relationships.
//! Validation preserves field values; catalog construction canonically orders records.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, FixedOffset, SecondsFormat};
use percent_encoding::percent_decode_str;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use strum::{AsRefStr, Display, EnumString, VariantNames};
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::codec::{object, object_map, objects, optional_object, required_nullable};

/// Personal lifecycle of a stream or contribution; completion does not close its workspace.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
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
pub enum Status {
    #[default]
    Active,
    Waiting,
    Blocked,
    Parked,
    Done,
    Closed,
}

impl Status {
    /// Whether work is neither finished (`done`) nor `closed`.
    pub const fn is_open(self) -> bool {
        matches!(
            self,
            Self::Active | Self::Waiting | Self::Blocked | Self::Parked
        )
    }

    /// Whether a local Session is eligible for automatic open/resume selection.
    pub const fn is_open_eligible(self) -> bool {
        matches!(self, Self::Active | Self::Parked)
    }
}

/// Whether generated reports may replace text. Manual text remains user-owned.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
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
pub enum NamingMode {
    #[default]
    Automatic,
    Manual,
}

/// Per-record title policy; inheritance resolves against the record type's global default.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
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
pub enum TitleMode {
    Automatic,
    Manual,
    #[default]
    Inherit,
}

impl TitleMode {
    pub fn resolve(self, default: NamingMode) -> NamingMode {
        match self {
            Self::Automatic => NamingMode::Automatic,
            Self::Manual => NamingMode::Manual,
            Self::Inherit => default,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TitleDefaults {
    pub stream: NamingMode,
    pub session: NamingMode,
}

/// The record schemas accepted by catalog files and operation envelopes.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    EnumString,
    VariantNames,
    Display,
    AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum RecordKind {
    Host,
    Session,
    Stream,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ReferenceRole {
    Deliverable,
    Dependency,
    Brief,
    Background,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ReferenceState {
    Active,
    Fulfilled,
    Dropped,
}

crate::codec::deserialize_string_enum!(
    Status,
    NamingMode,
    TitleMode,
    RecordKind,
    ReferenceRole,
    ReferenceState
);

fn checkout_directory() -> String {
    ".".into()
}

fn deserialize_stream_title<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    String::deserialize(d)
        .map_err(|_| serde::de::Error::custom("stream.title must be a nonempty string"))
}

mod owner {
    use super::*;

    #[derive(Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Owner<T> {
        host_id: T,
    }

    pub fn serialize<S: Serializer>(host_id: &str, s: S) -> Result<S::Ok, S::Error> {
        Owner { host_id }.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        Ok(object::<Owner<String>, _>(d)?.host_id)
    }
}

fn serialize_checked_at<S: Serializer>(
    checked_at: &DateTime<FixedOffset>,
    s: S,
) -> Result<S::Ok, S::Error> {
    let precision = if checked_at.timestamp_subsec_nanos() == 0 {
        SecondsFormat::Secs
    } else {
        SecondsFormat::Micros
    };
    s.serialize_str(&checked_at.to_rfc3339_opts(precision, false))
}

/// A logical constraint violation in a record or catalog.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

impl ValidationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// Rejects NULs and, unless allowed, empty or whitespace-only text; preserves input.
pub fn check_text(value: &str, name: &str, empty_ok: bool) -> Result<(), ValidationError> {
    if value.contains('\0') || (!empty_ok && value.trim().is_empty()) {
        let kind = if empty_ok {
            "a string"
        } else {
            "a nonempty string"
        };
        return Err(ValidationError::new(format!("{name} must be {kind}")));
    }
    Ok(())
}

/// Rejects absolute paths, drive prefixes, backslashes, and parent traversal.
pub fn check_relative_path(value: &str, name: &str) -> Result<(), ValidationError> {
    check_text(value, name, false)?;
    let has_drive = value.len() >= 2
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.as_bytes()[1] == b':';
    if value.starts_with('/')
        || has_drive
        || value.contains('\\')
        || value.split('/').any(|part| part == "..")
    {
        return Err(ValidationError::new(format!(
            "{name} must be a portable relative path"
        )));
    }
    Ok(())
}

/// A repository URL referenced by its alias within a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub url: String,
}

impl Repository {
    /// Requires nonempty text, not a particular URL scheme or reachable repository.
    pub fn new(url: impl Into<String>) -> Result<Self, ValidationError> {
        let repository = Self { url: url.into() };
        repository.validate()?;
        Ok(repository)
    }

    /// Checks the URL text without resolving or modifying it.
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.url, "repository.url", false)
    }
}

/// Checks browser safety without changing the exact saved URL identity.
pub fn reference_url(value: &str) -> Option<url::Url> {
    let unsafe_character = |c| {
        matches!(
            get_general_category(c),
            GeneralCategory::Control | GeneralCategory::Format
        )
    };
    if value
        .chars()
        .any(|c| c.is_whitespace() || c == '\\' || unsafe_character(c))
    {
        return None;
    }
    let bytes = value.as_bytes();
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'%'
            && !(bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit))
        {
            return None;
        }
    }
    if percent_decode_str(value)
        .decode_utf8()
        .ok()?
        .chars()
        .any(unsafe_character)
    {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(url)
}

/// A document locator and recovery hints, independent of its relationship to work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryDocument {
    pub repository_url: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
}

impl RepositoryDocument {
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.repository_url, "document.repository_url", false)?;
        check_relative_path(&self.path, "document.path")?;
        for (name, value) in [
            ("document.ref", &self.r#ref),
            ("document.commit", &self.commit),
            ("document.source_session_id", &self.source_session_id),
        ] {
            if let Some(value) = value {
                check_text(value, name, false)?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ReferenceTarget {
    Document(RepositoryDocument),
    Pr { url: String },
    Issue { url: String },
    Link { url: String },
}

impl ReferenceTarget {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Document(document) => document.validate(),
            Self::Pr { url } | Self::Issue { url } | Self::Link { url } => {
                check_text(url, "reference.target.url", false)?;
                reference_url(url).ok_or_else(|| {
                    ValidationError::new("reference.target.url must be a safe HTTP(S) URL")
                })?;
                Ok(())
            }
        }
    }

    pub fn key(&self) -> ResourceKey {
        match self {
            Self::Document(document) => ResourceKey::Document {
                repository_url: document.repository_url.clone(),
                path: document.path.clone(),
            },
            Self::Pr { url } | Self::Issue { url } | Self::Link { url } => {
                ResourceKey::Url { url: url.clone() }
            }
        }
    }
}

/// Exact identity excludes classification, relationship text and recovery hints.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ResourceKey {
    Document {
        repository_url: String,
        path: String,
    },
    Url {
        url: String,
    },
}

impl ResourceKey {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Document {
                repository_url,
                path,
            } => {
                check_text(repository_url, "reference.key.repository_url", false)?;
                check_relative_path(path, "reference.key.path")
            }
            Self::Url { url } => {
                check_text(url, "reference.key.url", false)?;
                reference_url(url).ok_or_else(|| {
                    ValidationError::new("reference.key.url must be a safe HTTP(S) URL")
                })?;
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceValue {
    pub name: String,
    pub purpose: String,
    pub role: ReferenceRole,
    pub state: ReferenceState,
    pub target: ReferenceTarget,
}

impl ReferenceValue {
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.name, "reference.name", false)?;
        check_text(&self.purpose, "reference.purpose", false)?;
        self.target.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub name: String,
    pub purpose: String,
    pub role: ReferenceRole,
    pub state: ReferenceState,
    pub target: ReferenceTarget,
    pub revision: String,
}

impl Reference {
    pub fn new(
        value: ReferenceValue,
        revision: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let reference = Self {
            name: value.name,
            purpose: value.purpose,
            role: value.role,
            state: value.state,
            target: value.target,
            revision: revision.into(),
        };
        reference.validate()?;
        Ok(reference)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.name, "reference.name", false)?;
        check_text(&self.purpose, "reference.purpose", false)?;
        check_text(&self.revision, "reference.revision", false)?;
        self.target.validate()
    }

    pub fn value(&self) -> ReferenceValue {
        ReferenceValue {
            name: self.name.clone(),
            purpose: self.purpose.clone(),
            role: self.role,
            state: self.state,
            target: self.target.clone(),
        }
    }
}

/// A session's repository ref and working directory within its checkout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    /// Legacy Stream-local label; an explicit URL takes precedence after reassociation.
    pub repository: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository_url: Option<String>,
    pub r#ref: String,
    #[serde(default = "checkout_directory")]
    pub directory_within_checkout: String,
}

impl Checkout {
    /// Requires a portable relative directory; `.` denotes the checkout root.
    pub fn new(
        repository: impl Into<String>,
        r#ref: impl Into<String>,
        directory_within_checkout: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let checkout = Self {
            repository: repository.into(),
            repository_url: None,
            r#ref: r#ref.into(),
            directory_within_checkout: directory_within_checkout.into(),
        };
        checkout.validate()?;
        Ok(checkout)
    }

    /// Check portable checkout fields.
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.repository, "checkout.repository", false)?;
        if let Some(url) = &self.repository_url {
            check_text(url, "checkout.repository_url", false)?;
        }
        check_text(&self.r#ref, "checkout.ref", false)?;
        check_relative_path(
            &self.directory_within_checkout,
            "checkout.directory_within_checkout",
        )
    }
}

/// A durable effort with stream-local repository aliases and supporting references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub id: String,
    #[serde(deserialize_with = "deserialize_stream_title")]
    pub title: String,
    #[serde(deserialize_with = "required_nullable")]
    pub goal: Option<String>,
    /// Durable ownership, separate from any current runtime host binding.
    #[serde(rename = "owner", with = "owner")]
    pub owner_host_id: String,
    #[serde(default)]
    pub title_mode: TitleMode,
    pub goal_short: Option<String>,
    #[serde(default)]
    pub goal_mode: NamingMode,
    #[serde(default)]
    pub status: Status,
    #[serde(default, deserialize_with = "object_map")]
    pub repositories: BTreeMap<String, Repository>,
    #[serde(deserialize_with = "objects")]
    pub references: Vec<Reference>,
}

impl Stream {
    /// Creates an active stream with inherited title policy and no references.
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        goal: Option<String>,
        owner_host_id: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        Self {
            id: id.into(),
            title: title.into(),
            goal,
            owner_host_id: owner_host_id.into(),
            title_mode: TitleMode::Inherit,
            goal_short: None,
            goal_mode: NamingMode::Automatic,
            status: Status::Active,
            repositories: BTreeMap::new(),
            references: Vec::new(),
        }
        .validated()
    }

    /// Returns this stream unchanged after checking its local invariants.
    pub fn validated(self) -> Result<Self, ValidationError> {
        self.validate()?;
        Ok(self)
    }

    /// Checks fields and exact resource-key uniqueness.
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.id, "stream.id", false)?;
        check_text(&self.title, "stream.title", false)?;
        check_text(&self.owner_host_id, "stream.owner_host_id", false)?;
        match (&self.goal, &self.goal_short) {
            (Some(goal), _) => check_text(goal, "stream.goal", false)?,
            (None, Some(_)) => {
                return Err(ValidationError::new("stream.goal_short requires a goal"));
            }
            (None, None) => {}
        }
        if let Some(ref goal_short) = self.goal_short {
            check_text(goal_short, "stream.goal_short", false)?;
        }
        for (alias, repo) in &self.repositories {
            check_text(alias, "repository alias", false)?;
            repo.validate()?;
        }
        let mut keys = BTreeSet::new();
        for reference in &self.references {
            reference.validate()?;
            if !keys.insert(reference.target.key()) {
                return Err(ValidationError::new("duplicate reference resource key"));
            }
        }
        Ok(())
    }

    /// Returns the owned wire identity without copying the stream's contents.
    pub fn key(&self) -> RecordKey {
        RecordKey::Stream {
            id: self.id.clone(),
        }
    }
}

/// A logical harness conversation across stream memberships; its ID is scoped to its host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub id: String,
    pub host_id: String,
    pub stream_id: String,
    /// Ended memberships in transition order; revisiting a stream is allowed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub previous_stream_ids: Vec<String>,
    /// Attachment metadata grouped by the Stream where each output was produced.
    #[serde(
        default,
        skip_serializing_if = "BTreeMap::is_empty",
        deserialize_with = "crate::artifacts::groups"
    )]
    pub artifacts: BTreeMap<String, Vec<crate::artifacts::ArtifactEntry>>,
    pub harness: String,
    pub harness_session_id: String,
    pub title: String,
    #[serde(default)]
    pub title_mode: TitleMode,
    #[serde(default)]
    pub status: Status,
    /// This conversation's scope and cumulative contribution.
    #[serde(default)]
    pub recap: String,
    #[serde(default)]
    pub recap_short: Option<String>,
    /// Current work, distinct from lifecycle status.
    #[serde(default)]
    pub state: String,
    pub state_short: Option<String>,
    #[serde(default)]
    pub remaining: String,
    #[serde(default)]
    pub remaining_short: Option<String>,
    #[serde(default, deserialize_with = "optional_object")]
    pub checkout: Option<Checkout>,
}

impl Session {
    /// Stable native identity, independent of current membership or physical location.
    pub fn id_for(host_id: &str, harness: &str, native_id: &str) -> String {
        let identity = format!(
            "streams:session:[{}, {}, {}]",
            crate::codec::ascii_json(host_id),
            crate::codec::ascii_json(harness),
            crate::codec::ascii_json(native_id),
        );
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, identity.as_bytes()).to_string()
    }

    /// The portable record format requires a direct identity and self-contained checkout.
    pub fn validate_portable(&self) -> Result<(), ValidationError> {
        self.validate()?;
        if self.id != Self::id_for(&self.host_id, &self.harness, &self.harness_session_id) {
            return Err(ValidationError::new(
                "session ID does not match its native identity; run offline catalog cutover",
            ));
        }
        if self
            .checkout
            .as_ref()
            .is_some_and(|checkout| checkout.repository_url.is_none())
        {
            return Err(ValidationError::new(
                "session checkout must contain its portable repository URL; run offline catalog cutover",
            ));
        }
        Ok(())
    }

    /// Creates an active conversation; stream and checkout relationships are catalog checks.
    pub fn new(
        id: impl Into<String>,
        host_id: impl Into<String>,
        stream_id: impl Into<String>,
        harness: impl Into<String>,
        harness_session_id: impl Into<String>,
        title: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        Self {
            id: id.into(),
            host_id: host_id.into(),
            stream_id: stream_id.into(),
            previous_stream_ids: Vec::new(),
            artifacts: BTreeMap::new(),
            harness: harness.into(),
            harness_session_id: harness_session_id.into(),
            title: title.into(),
            title_mode: TitleMode::Inherit,
            status: Status::Active,
            recap: String::new(),
            recap_short: None,
            state: String::new(),
            state_short: None,
            remaining: String::new(),
            remaining_short: None,
            checkout: None,
        }
        .validated()
    }

    /// Returns this conversation unchanged after checking its local invariants.
    pub fn validated(self) -> Result<Self, ValidationError> {
        self.validate()?;
        Ok(self)
    }

    /// Checks fields and checkout shape, not catalog relationships or native ID uniqueness.
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.id, "session.id", false)?;
        check_text(&self.host_id, "session.host_id", false)?;
        check_text(&self.stream_id, "session.stream_id", false)?;
        for id in &self.previous_stream_ids {
            check_text(id, "session.previous_stream_ids entry", false)?;
        }
        if self
            .previous_stream_ids
            .windows(2)
            .any(|pair| pair[0] == pair[1])
            || self.previous_stream_ids.last() == Some(&self.stream_id)
        {
            return Err(ValidationError::new(
                "session membership transitions must change streams",
            ));
        }
        for (stream, artifacts) in &self.artifacts {
            if stream != &self.stream_id && !self.previous_stream_ids.contains(stream) {
                return Err(ValidationError::new(
                    "artifact group must name a current or former Stream",
                ));
            }
            let mut names = BTreeSet::new();
            for artifact in artifacts {
                artifact.validate()?;
                if !names.insert(&artifact.name) {
                    return Err(ValidationError::new(
                        "duplicate artifact name in Session Stream group",
                    ));
                }
            }
        }
        check_text(&self.harness, "session.harness", false)?;
        check_text(
            &self.harness_session_id,
            "session.harness_session_id",
            false,
        )?;
        check_text(&self.title, "session.title", false)?;
        check_text(&self.recap, "session.recap", true)?;
        check_text(&self.state, "session.state", true)?;
        check_text(&self.remaining, "session.remaining", true)?;
        for (name, value) in [
            ("session.recap_short", &self.recap_short),
            ("session.state_short", &self.state_short),
            ("session.remaining_short", &self.remaining_short),
        ] {
            if let Some(value) = value {
                check_text(value, name, false)?;
            }
        }
        if let Some(ref co) = self.checkout {
            co.validate()?;
        }
        Ok(())
    }

    /// Returns the host-scoped owned identity without copying conversation contents.
    pub fn key(&self) -> RecordKey {
        RecordKey::Session {
            id: self.id.clone(),
            host_id: self.host_id.clone(),
        }
    }

    /// Native harness identity must be unique across the catalog, independent of record ID.
    pub fn native_identity(&self) -> (&str, &str, &str) {
        (&self.host_id, &self.harness, &self.harness_session_id)
    }
}

/// A durable host identity and the time it was last checked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub id: String,
    pub label: String,
    #[serde(serialize_with = "serialize_checked_at")]
    pub checked_at: DateTime<FixedOffset>,
}

impl Host {
    /// Requires nonempty identity/label text and a timestamp with at most microsecond precision.
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        checked_at: DateTime<FixedOffset>,
    ) -> Result<Self, ValidationError> {
        let host = Self {
            id: id.into(),
            label: label.into(),
            checked_at,
        };
        host.validate()?;
        Ok(host)
    }

    /// Checks text and timestamp precision without rounding or changing the offset.
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.id, "host.id", false)?;
        check_text(&self.label, "host.label", false)?;
        if !self
            .checked_at
            .timestamp_subsec_nanos()
            .is_multiple_of(1000)
        {
            return Err(ValidationError::new(
                "host.checked_at exceeds microsecond precision",
            ));
        }
        Ok(())
    }

    /// Returns the owned identity without copying host metadata.
    pub fn key(&self) -> RecordKey {
        RecordKey::Host {
            id: self.id.clone(),
        }
    }
}

/// Any catalog record, used for operations that share an identity boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[allow(
    clippy::large_enum_variant,
    reason = "keep owned records without adding a heap allocation to each Session read"
)]
pub enum Record {
    Stream(Stream),
    Session(Session),
    Host(Host),
}

impl Record {
    /// Returns this record unchanged after checking its local invariants.
    pub fn validated(self) -> Result<Self, ValidationError> {
        self.validate()?;
        Ok(self)
    }

    /// Checks the contained record; cross-record relationships require a catalog.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Stream(s) => s.validate(),
            Self::Session(s) => s.validate(),
            Self::Host(h) => h.validate(),
        }
    }

    /// Validate the current portable schema, including directly addressable Sessions.
    pub fn validate_portable(&self) -> Result<(), ValidationError> {
        match self {
            Self::Session(session) => session.validate_portable(),
            _ => self.validate(),
        }
    }

    /// Copies only the identity fields into an owned key.
    pub fn key(&self) -> RecordKey {
        match self {
            Self::Stream(s) => s.key(),
            Self::Session(s) => s.key(),
            Self::Host(h) => h.key(),
        }
    }

    pub fn kind(&self) -> RecordKind {
        match self {
            Record::Stream(_) => RecordKind::Stream,
            Record::Session(_) => RecordKind::Session,
            Record::Host(_) => RecordKind::Host,
        }
    }
}

/// An owned catalog identity; only session identities include a host scope.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum RecordKey {
    // Preserve the lexical kind/id/host ordering used by the catalog wire boundary.
    Host { id: String },
    Session { id: String, host_id: String },
    Stream { id: String },
}

impl RecordKey {
    pub fn kind(&self) -> RecordKind {
        match self {
            Self::Stream { .. } => RecordKind::Stream,
            Self::Session { .. } => RecordKind::Session,
            Self::Host { .. } => RecordKind::Host,
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Stream { id } | Self::Session { id, .. } | Self::Host { id } => id,
        }
    }
    /// Returns the session identity scope, not a stream owner or runtime binding.
    pub fn host_id(&self) -> Option<&str> {
        match self {
            Self::Session { host_id, .. } => Some(host_id),
            _ => None,
        }
    }
}

impl fmt::Display for RecordKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.host_id() {
            Some(host) => write!(f, "{}:{host}/{}", self.kind(), self.id()),
            None => write!(f, "{}:{}", self.kind(), self.id()),
        }
    }
}
