//! Format-5 typed records with strict wire field mapping.
//! Every absent value is `null`, never empty text and never omitted.

use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;

use percent_encoding::percent_decode_str;
use serde::{Deserialize, Serialize};
use strum::{AsRefStr, Display, EnumString, VariantNames};
use unicode_general_category::{GeneralCategory, get_general_category};

pub use crate::artifacts::ArtifactEntry;
use crate::artifacts::validate_name as validate_artifact_name;
use crate::codec::{object, object_map, optional_object, required_nullable};

/// Personal lifecycle of a Session; Stream status is derived from its member Sessions.
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
    /// Status values automatic Session reports may set (`closed` requires user action or tidy).
    pub const REPORT_VARIANTS: &'static [&'static str] =
        &["active", "waiting", "blocked", "parked", "done"];

    /// Whether an automatic Session report may set this status.
    pub const fn reportable(self) -> bool {
        !matches!(self, Self::Closed)
    }

    /// Precedence order for deriving a Stream's status as `max(sessions)`:
    /// `active > waiting > blocked > parked > done > closed`.
    pub const fn precedence(self) -> u8 {
        match self {
            Self::Active => 5,
            Self::Waiting => 4,
            Self::Blocked => 3,
            Self::Parked => 2,
            Self::Done => 1,
            Self::Closed => 0,
        }
    }

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

/// Whether generated reports may propose titles or goals (`naming.stream`, `naming.session`).
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TitleDefaults {
    pub stream: NamingMode,
    pub session: NamingMode,
}

/// The record schemas accepted by catalog files.
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
pub enum ReferenceKind {
    Pr,
    Issue,
    Link,
    Document,
}

impl ReferenceKind {
    pub fn is_url(self) -> bool {
        !matches!(self, Self::Document)
    }
}

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
pub enum ReferenceRole {
    Deliverable,
    Dependency,
    Brief,
    Background,
}

impl ReferenceRole {
    /// Display sort order for cards and sidebars: deliverable, dependency, brief, background.
    pub const fn sort_rank(self) -> u8 {
        match self {
            Self::Deliverable => 0,
            Self::Dependency => 1,
            Self::Brief => 2,
            Self::Background => 3,
        }
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum ReferenceStatus {
    Active,
    Fulfilled,
    Dropped,
}

crate::codec::deserialize_string_enum!(
    Status,
    NamingMode,
    RecordKind,
    ReferenceKind,
    ReferenceRole,
    ReferenceStatus
);

/// A logical constraint violation in a record or catalog.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

impl ValidationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// The most characters a short form keeps. Short forms name tabs and cards.
pub const SHORT_TEXT_CHARS: usize = 24;

/// A short form on one line, cut at a character boundary to `SHORT_TEXT_CHARS`.
pub fn short_text(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let short: String = line.chars().take(SHORT_TEXT_CHARS).collect();
    short.trim_end().to_owned()
}

/// Safe path token for `host_id`, `harness`, Stream `id` and Session `id`.
/// ASCII letters, digits, `.`, `_` and `-`, starting and ending with a letter or digit.
pub fn check_token(value: &str, name: &str) -> Result<(), ValidationError> {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || !bytes[0].is_ascii_alphanumeric()
        || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
        || !bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(ValidationError::new(format!(
            "{name} {value:?} must use only ASCII letters, digits, '.', '_' and '-', starting and ending with a letter or digit"
        )));
    }
    Ok(())
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

/// Portable relative path key used for document reference keys and `directory.repository.within`.
/// Segments are separated by `/`; no segment is empty, `.` or `..`; no `\`, no leading `/`,
/// no `:` in the first segment, at least one non-whitespace char, and no ASCII control or DEL.
pub fn check_path_key(value: &str, name: &str) -> Result<(), ValidationError> {
    if value.is_empty()
        || value.trim().is_empty()
        || value.bytes().any(|b| b < 0x20 || b == 0x7f)
        || value.contains('\\')
        || value.starts_with('/')
        || value.split('/').any(|part| matches!(part, "" | "." | ".."))
        || value
            .split('/')
            .next()
            .is_some_and(|part| part.contains(':'))
    {
        return Err(ValidationError::new(format!(
            "{name} {value:?} must be a portable relative path"
        )));
    }
    Ok(())
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

/// A title and its manual mark, kept as one editable unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Title {
    pub text: String,
    pub manual: bool,
}

impl Title {
    pub fn new(text: impl Into<String>, manual: bool) -> Self {
        Self {
            text: text.into(),
            manual,
        }
    }

    pub fn validate(&self, name: &str) -> Result<(), ValidationError> {
        check_text(&self.text, &format!("{name}.text"), false)
    }
}

/// A Stream's goal, optional short form, and manual mark, kept as one editable unit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    pub text: String,
    #[serde(deserialize_with = "required_nullable")]
    pub short: Option<String>,
    pub manual: bool,
}

impl Goal {
    pub fn new(text: impl Into<String>, short: Option<String>, manual: bool) -> Self {
        Self {
            text: text.into(),
            short,
            manual,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.text, "goal.text", false)?;
        if let Some(short) = &self.short {
            check_text(short, "goal.short", false)?;
        }
        Ok(())
    }
}

/// A progress text and its optional short form (`state`, `recap`, `remaining`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextPair {
    pub text: String,
    #[serde(deserialize_with = "required_nullable")]
    pub short: Option<String>,
}

impl TextPair {
    pub fn new(text: impl Into<String>, short: Option<String>) -> Self {
        Self {
            text: text.into(),
            short,
        }
    }

    pub fn validate(&self, name: &str) -> Result<(), ValidationError> {
        check_text(&self.text, &format!("{name}.text"), false)?;
        if let Some(short) = &self.short {
            check_text(short, &format!("{name}.short"), false)?;
        }
        Ok(())
    }
}

/// Clone metadata inspected when a Session's directory is set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepositoryInfo {
    pub url: String,
    #[serde(deserialize_with = "required_nullable")]
    pub within: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub branch: Option<String>,
}

impl RepositoryInfo {
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.url, "directory.repository.url", false)?;
        if let Some(within) = &self.within {
            check_path_key(within, "directory.repository.within")?;
        }
        if let Some(branch) = &self.branch {
            check_text(branch, "directory.repository.branch", false)?;
        }
        Ok(())
    }
}

/// Where a Session runs on its own host, and optional clone metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionDirectory {
    pub path: String,
    #[serde(deserialize_with = "optional_object")]
    pub repository: Option<RepositoryInfo>,
}

impl SessionDirectory {
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.path, "directory.path", false)?;
        if !Path::new(&self.path).is_absolute() {
            return Err(ValidationError::new(
                "directory.path must be an absolute path",
            ));
        }
        if let Some(repo) = &self.repository {
            repo.validate()?;
        }
        Ok(())
    }
}

/// One entry in a Session's `references` map; its map key is the URL or relative document path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferenceEntry {
    pub kind: ReferenceKind,
    pub name: String,
    pub purpose: String,
    pub role: ReferenceRole,
    pub status: ReferenceStatus,
    pub manual: bool,
}

impl ReferenceEntry {
    pub fn validate_for_key(&self, key: &str) -> Result<(), ValidationError> {
        match self.kind {
            ReferenceKind::Document => check_path_key(key, "document reference key")?,
            ReferenceKind::Pr | ReferenceKind::Issue | ReferenceKind::Link => {
                check_text(key, "URL reference key", false)?;
                reference_url(key).ok_or_else(|| {
                    ValidationError::new(format!(
                        "reference key {key:?} must be a safe HTTP(S) URL for kind {}",
                        self.kind
                    ))
                })?;
            }
        }
        check_text(&self.name, &format!("reference {key:?} name"), false)?;
        check_text(&self.purpose, &format!("reference {key:?} purpose"), false)?;
        Ok(())
    }
}

/// A durable work stream owned by one host; its status is derived from its member Sessions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub host_id: String,
    pub id: String,
    #[serde(deserialize_with = "object")]
    pub title: Title,
    #[serde(deserialize_with = "optional_object")]
    pub goal: Option<Goal>,
    pub revision: u64,
    pub updated_at_ms: u64,
}

impl Stream {
    pub fn new(
        host_id: impl Into<String>,
        id: impl Into<String>,
        title: Title,
        goal: Option<Goal>,
    ) -> Result<Self, ValidationError> {
        let stream = Self {
            host_id: host_id.into(),
            id: id.into(),
            title,
            goal,
            revision: 0,
            updated_at_ms: 0,
        };
        stream.validated()
    }

    pub fn validated(self) -> Result<Self, ValidationError> {
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        check_token(&self.host_id, "stream.host_id")?;
        check_token(&self.id, "stream.id")?;
        self.title.validate("stream.title")?;
        if let Some(goal) = &self.goal {
            goal.validate()?;
        }
        Ok(())
    }

    /// Full Stream key: `HOST/ID`.
    pub fn full_key(&self) -> String {
        format!("{}/{}", self.host_id, self.id)
    }

    pub fn key(&self) -> RecordKey {
        RecordKey::Stream {
            host_id: self.host_id.clone(),
            id: self.id.clone(),
        }
    }
}

/// A single agent conversation's contribution to one same-host Stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub host_id: String,
    pub harness: String,
    pub id: String,
    pub stream_id: String,
    #[serde(deserialize_with = "object")]
    pub title: Title,
    pub status: Status,
    #[serde(deserialize_with = "optional_object")]
    pub state: Option<TextPair>,
    #[serde(deserialize_with = "optional_object")]
    pub recap: Option<TextPair>,
    #[serde(deserialize_with = "optional_object")]
    pub remaining: Option<TextPair>,
    #[serde(deserialize_with = "optional_object")]
    pub directory: Option<SessionDirectory>,
    #[serde(deserialize_with = "object_map")]
    pub references: BTreeMap<String, ReferenceEntry>,
    #[serde(deserialize_with = "object_map")]
    pub artifacts: BTreeMap<String, ArtifactEntry>,
    pub revision: u64,
    pub updated_at_ms: u64,
}

impl Session {
    pub fn new(
        host_id: impl Into<String>,
        harness: impl Into<String>,
        id: impl Into<String>,
        stream_id: impl Into<String>,
        title: Title,
    ) -> Result<Self, ValidationError> {
        let session = Self {
            host_id: host_id.into(),
            harness: harness.into(),
            id: id.into(),
            stream_id: stream_id.into(),
            title,
            status: Status::Active,
            state: None,
            recap: None,
            remaining: None,
            directory: None,
            references: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            revision: 0,
            updated_at_ms: 0,
        };
        session.validated()
    }

    pub fn validated(self) -> Result<Self, ValidationError> {
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        check_token(&self.host_id, "session.host_id")?;
        check_token(&self.harness, "session.harness")?;
        check_token(&self.id, "session.id")?;
        check_token(&self.stream_id, "session.stream_id")?;
        self.title.validate("session.title")?;
        if let Some(state) = &self.state {
            state.validate("session.state")?;
        }
        if let Some(recap) = &self.recap {
            recap.validate("session.recap")?;
        }
        if let Some(remaining) = &self.remaining {
            remaining.validate("session.remaining")?;
        }
        if let Some(directory) = &self.directory {
            directory.validate()?;
        }
        for (key, entry) in &self.references {
            entry.validate_for_key(key)?;
        }
        for (name, entry) in &self.artifacts {
            validate_artifact_name(name)?;
            entry.validate()?;
        }
        Ok(())
    }

    /// Full Session key: `HOST/HARNESS/ID`.
    pub fn full_key(&self) -> String {
        format!("{}/{}/{}", self.host_id, self.harness, self.id)
    }

    /// Local Session pair: `HARNESS/ID`.
    pub fn harness_key(&self) -> String {
        format!("{}/{}", self.harness, self.id)
    }

    /// Owning Stream's full key: `HOST/STREAM_ID`.
    pub fn stream_key(&self) -> String {
        format!("{}/{}", self.host_id, self.stream_id)
    }

    pub fn key(&self) -> RecordKey {
        RecordKey::Session {
            host_id: self.host_id.clone(),
            harness: self.harness.clone(),
            id: self.id.clone(),
        }
    }
}

/// A durable host identity and its human-readable label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub id: String,
    pub label: String,
}

impl Host {
    /// Requires valid token ID and nonempty single-line label.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Result<Self, ValidationError> {
        let host = Self {
            id: id.into(),
            label: label.into(),
        };
        host.validate()?;
        Ok(host)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        check_token(&self.id, "host.id")?;
        check_text(&self.label, "host.label", false)?;
        Ok(())
    }

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
    pub fn validated(self) -> Result<Self, ValidationError> {
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Stream(s) => s.validate(),
            Self::Session(s) => s.validate(),
            Self::Host(h) => h.validate(),
        }
    }

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

    /// Equal apart from the store's save stamps, as a retried write compares itself.
    pub fn same_content(&self, other: &Record) -> bool {
        let mut other = other.clone();
        if let Some(stamps) = self.stamps() {
            other.set_stamps(stamps);
        }
        *self == other
    }

    pub fn stamps(&self) -> Option<Stamps> {
        match self {
            Self::Stream(s) => Some(Stamps::of(s.revision, s.updated_at_ms)),
            Self::Session(s) => Some(Stamps::of(s.revision, s.updated_at_ms)),
            Self::Host(_) => None,
        }
    }

    pub fn revision(&self) -> Option<u64> {
        self.stamps().map(|stamps| stamps.revision)
    }

    pub fn set_stamps(&mut self, stamps: Stamps) {
        let (revision, updated_at_ms) = match self {
            Self::Stream(s) => (&mut s.revision, &mut s.updated_at_ms),
            Self::Session(s) => (&mut s.revision, &mut s.updated_at_ms),
            Self::Host(_) => return,
        };
        *revision = stamps.revision;
        *updated_at_ms = stamps.updated_at_ms;
    }
}

/// A Stream or Session's save count and the time of its last changing save.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stamps {
    pub revision: u64,
    pub updated_at_ms: u64,
}

impl Stamps {
    fn of(revision: u64, updated_at_ms: u64) -> Self {
        Self {
            revision,
            updated_at_ms,
        }
    }
}

/// An owned catalog identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum RecordKey {
    Host {
        id: String,
    },
    Session {
        host_id: String,
        harness: String,
        id: String,
    },
    Stream {
        host_id: String,
        id: String,
    },
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
            Self::Stream { id, .. } | Self::Session { id, .. } | Self::Host { id } => id,
        }
    }

    pub fn host_id(&self) -> &str {
        match self {
            Self::Stream { host_id, .. } | Self::Session { host_id, .. } => host_id,
            Self::Host { id } => id,
        }
    }

    pub fn harness(&self) -> Option<&str> {
        match self {
            Self::Session { harness, .. } => Some(harness),
            _ => None,
        }
    }
}

impl fmt::Display for RecordKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host { id } => f.write_str(id),
            Self::Stream { host_id, id } => write!(f, "{host_id}/{id}"),
            Self::Session {
                host_id,
                harness,
                id,
            } => write!(f, "{host_id}/{harness}/{id}"),
        }
    }
}
