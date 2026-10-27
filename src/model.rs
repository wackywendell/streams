//! Typed records with strict wire field mapping. Deserialization checks wire shapes;
//! record validation checks local constraints, and catalog construction checks relationships.
//! Validation preserves field values; catalog construction canonically orders records.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, FixedOffset, SecondsFormat};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use strum::{AsRefStr, Display, EnumString, VariantNames};

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

crate::codec::deserialize_string_enum!(Status, NamingMode, TitleMode, RecordKind);

fn checkout_directory() -> String {
    ".".into()
}

// Keep the field diagnostic used to recover ambiguous legacy YAML titles.
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

/// An issue or pull-request URL with optional descriptive text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub url: String,
    #[serde(default)]
    pub description: String,
}

impl Link {
    /// Requires nonempty URL text; the description may be empty.
    pub fn new(
        url: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let link = Self {
            url: url.into(),
            description: description.into(),
        };
        link.validate()?;
        Ok(link)
    }

    /// Checks text constraints without resolving or modifying the link.
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.url, "link.url", false)?;
        check_text(&self.description, "link.description", true)
    }
}

/// A repository-relative document tracked by a stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub repository: String,
    pub r#ref: String,
    pub path: String,
    #[serde(default)]
    pub description: String,
    /// Latest known commit from the source session, not a timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_session_id: Option<String>,
}

impl Document {
    /// Checks text and portable-path constraints; repository membership is stream-local.
    pub fn new(
        repository: impl Into<String>,
        r#ref: impl Into<String>,
        path: impl Into<String>,
        description: impl Into<String>,
        commit: Option<String>,
    ) -> Result<Self, ValidationError> {
        let document = Self {
            repository: repository.into(),
            r#ref: r#ref.into(),
            path: path.into(),
            description: description.into(),
            commit,
            source_session_id: None,
        };
        document.validate()?;
        Ok(document)
    }

    /// Checks local fields, not repository membership or ref existence.
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_text(&self.repository, "document.repository", false)?;
        check_text(&self.r#ref, "document.ref", false)?;
        check_relative_path(&self.path, "document.path")?;
        check_text(&self.description, "document.description", true)?;
        for (name, value) in [
            ("document.commit", &self.commit),
            ("document.source_session_id", &self.source_session_id),
        ] {
            if let Some(value) = value {
                check_text(value, name, false)?;
            }
        }
        Ok(())
    }

    /// Identity within a stream excludes the ref and descriptive metadata.
    pub fn identity(&self) -> (&str, &str) {
        (&self.repository, &self.path)
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

    /// Resolve old aliases only while no independent repository URL has been recorded.
    pub fn repository_url<'a>(&'a self, stream: &'a Stream) -> Option<&'a str> {
        self.repository_url.as_deref().or_else(|| {
            stream
                .repositories
                .get(&self.repository)
                .map(|repo| repo.url.as_str())
        })
    }

    /// Check portable checkout fields; the catalog resolves legacy repository aliases.
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
    #[serde(default)]
    pub archived: bool,
    #[serde(default, deserialize_with = "object_map")]
    pub repositories: BTreeMap<String, Repository>,
    #[serde(default, deserialize_with = "objects")]
    pub documents: Vec<Document>,
    #[serde(default, deserialize_with = "objects")]
    pub issues: Vec<Link>,
    #[serde(default, deserialize_with = "objects")]
    pub prs: Vec<Link>,
}

impl Stream {
    /// Creates an active, unarchived stream with inherited title policy and no references.
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
            archived: false,
            repositories: BTreeMap::new(),
            documents: Vec::new(),
            issues: Vec::new(),
            prs: Vec::new(),
        }
        .validated()
    }

    /// Returns this stream unchanged after checking its local invariants.
    pub fn validated(self) -> Result<Self, ValidationError> {
        self.validate()?;
        Ok(self)
    }

    /// Checks fields, reference uniqueness, and document repository membership.
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
        let mut doc_ids = BTreeSet::new();
        for doc in &self.documents {
            doc.validate()?;
            if !doc_ids.insert(doc.identity()) {
                return Err(ValidationError::new("duplicate documents"));
            }
        }
        let mut issue_urls = BTreeSet::new();
        for issue in &self.issues {
            issue.validate()?;
            if !issue_urls.insert(issue.url.as_str()) {
                return Err(ValidationError::new("duplicate issues"));
            }
        }
        let mut pr_urls = BTreeSet::new();
        for pr in &self.prs {
            pr.validate()?;
            if !pr_urls.insert(pr.url.as_str()) {
                return Err(ValidationError::new("duplicate prs"));
            }
        }
        for doc in &self.documents {
            if !self.repositories.contains_key(&doc.repository) {
                return Err(ValidationError::new(format!(
                    "unknown document repository: {}",
                    doc.repository
                )));
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

/// Records checked together for identity uniqueness and attachment relationships.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Catalog {
    streams: Vec<Stream>,
    sessions: Vec<Session>,
    hosts: Vec<Host>,
}

impl Catalog {
    /// Checks records, duplicate record/native identities, session streams, and checkout aliases.
    /// Host references need not have corresponding host records. Collections are ordered by
    /// stream/host ID and session (host ID, ID); nested record values retain their order.
    pub fn new(
        mut streams: Vec<Stream>,
        mut sessions: Vec<Session>,
        mut hosts: Vec<Host>,
    ) -> Result<Self, ValidationError> {
        let mut stream_keys = BTreeSet::new();
        for s in &streams {
            s.validate()?;
            if !stream_keys.insert(s.id.as_str()) {
                return Err(ValidationError::new("duplicate streams record identity"));
            }
        }
        let mut session_keys = BTreeSet::new();
        let mut native_ids = BTreeSet::new();
        for s in &sessions {
            s.validate()?;
            if !session_keys.insert((s.id.as_str(), s.host_id.as_str())) {
                return Err(ValidationError::new("duplicate sessions record identity"));
            }
            if !native_ids.insert(s.native_identity()) {
                return Err(ValidationError::new("duplicate native session identity"));
            }
        }
        let mut host_keys = BTreeSet::new();
        for h in &hosts {
            h.validate()?;
            if !host_keys.insert(h.id.as_str()) {
                return Err(ValidationError::new("duplicate hosts record identity"));
            }
        }
        let stream_map: BTreeMap<&str, &Stream> =
            streams.iter().map(|s| (s.id.as_str(), s)).collect();
        for s in &sessions {
            // Ended memberships are historical identities, not live foreign keys.
            let Some(stream) = stream_map.get(s.stream_id.as_str()) else {
                return Err(ValidationError::new(format!(
                    "unknown session stream: {}",
                    s.stream_id
                )));
            };
            if let Some(ref co) = s.checkout
                && co.repository_url(stream).is_none()
            {
                return Err(ValidationError::new(format!(
                    "unknown checkout repository: {}",
                    co.repository
                )));
            }
        }
        streams.sort_by(|a, b| a.id.cmp(&b.id));
        sessions.sort_by(|a, b| (&a.host_id, &a.id).cmp(&(&b.host_id, &b.id)));
        hosts.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self {
            streams,
            sessions,
            hosts,
        })
    }

    pub fn streams(&self) -> &[Stream] {
        &self.streams
    }

    pub fn sessions(&self) -> &[Session] {
        &self.sessions
    }

    pub fn hosts(&self) -> &[Host] {
        &self.hosts
    }

    pub fn stream(&self, id: &str) -> Option<&Stream> {
        self.streams.iter().find(|s| s.id == id)
    }

    /// Looks up a session by its host-scoped catalog ID, not its native harness ID.
    pub fn session(&self, host_id: &str, id: &str) -> Option<&Session> {
        self.sessions
            .iter()
            .find(|s| s.host_id == host_id && s.id == id)
    }

    pub fn host(&self, id: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// Returns owned snapshots in stream, session, then host order.
    pub fn records(&self) -> Vec<Record> {
        let mut out =
            Vec::with_capacity(self.streams.len() + self.sessions.len() + self.hosts.len());
        out.extend(self.streams.iter().cloned().map(Record::Stream));
        out.extend(self.sessions.iter().cloned().map(Record::Session));
        out.extend(self.hosts.iter().cloned().map(Record::Host));
        out
    }

    /// Returns an owned snapshot; typed getters borrow instead.
    pub fn get(&self, key: &RecordKey) -> Option<Record> {
        match key {
            RecordKey::Stream { id } => self.stream(id).cloned().map(Record::Stream),
            RecordKey::Session { host_id, id } => {
                self.session(host_id, id).cloned().map(Record::Session)
            }
            RecordKey::Host { id } => self.host(id).cloned().map(Record::Host),
        }
    }

    /// Inserts or replaces by identity in a new catalog, rechecking all catalog invariants.
    pub fn replacing(&self, record: Record) -> Result<Self, ValidationError> {
        let mut next = self.clone();
        match record {
            Record::Stream(s) => {
                next.streams.retain(|old| old.id != s.id);
                next.streams.push(s);
            }
            Record::Session(s) => {
                next.sessions
                    .retain(|old| old.id != s.id || old.host_id != s.host_id);
                next.sessions.push(s);
            }
            Record::Host(h) => {
                next.hosts.retain(|old| old.id != h.id);
                next.hosts.push(h);
            }
        }
        Self::new(next.streams, next.sessions, next.hosts)
    }
}
