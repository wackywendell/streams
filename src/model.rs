//! Logical records and relationships with strict wire field mapping.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use chrono::{DateTime, FixedOffset, SecondsFormat};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

fn automatic() -> String {
    "automatic".into()
}

fn active() -> String {
    "active".into()
}

fn checkout_directory() -> String {
    ".".into()
}

// A custom deserializer prevents serde from treating an absent Option as None.
fn required_nullable<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    Option::deserialize(d)
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
        Ok(Owner::<String>::deserialize(d)?.host_id)
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

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ValidationError(pub String);

impl ValidationError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

pub const STATUSES: &[&str] = &["active", "waiting", "blocked", "parked", "done"];
pub const NAMING_MODES: &[&str] = &["automatic", "manual"];

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

pub fn check_choice(value: &str, name: &str, options: &[&str]) -> Result<(), ValidationError> {
    if !options.contains(&value) {
        return Err(ValidationError::new(format!(
            "{name} must be one of {}",
            options.join(", ")
        )));
    }
    Ok(())
}

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Repository {
    pub url: String,
}

impl Repository {
    pub fn new(url: impl Into<String>) -> Result<Self, ValidationError> {
        let url = url.into();
        check_text(&url, "repository.url", false)?;
        Ok(Self { url })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub url: String,
    #[serde(default)]
    pub description: String,
}

impl Link {
    pub fn new(
        url: impl Into<String>,
        description: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let url = url.into();
        let description = description.into();
        check_text(&url, "link.url", false)?;
        check_text(&description, "link.description", true)?;
        Ok(Self { url, description })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub repository: String,
    pub r#ref: String,
    pub path: String,
    #[serde(default)]
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_at: Option<String>,
}

impl Document {
    pub fn new(
        repository: impl Into<String>,
        r#ref: impl Into<String>,
        path: impl Into<String>,
        description: impl Into<String>,
        added_at: Option<String>,
    ) -> Result<Self, ValidationError> {
        let repository = repository.into();
        let r#ref = r#ref.into();
        let path = path.into();
        let description = description.into();
        check_text(&repository, "document.repository", false)?;
        check_text(&r#ref, "document.ref", false)?;
        check_relative_path(&path, "document.path")?;
        check_text(&description, "document.description", true)?;
        if let Some(ref commit) = added_at {
            check_text(commit, "document.added_at", false)?;
        }
        Ok(Self {
            repository,
            r#ref,
            path,
            description,
            added_at,
        })
    }

    pub fn identity(&self) -> (&str, &str) {
        (&self.repository, &self.path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Checkout {
    pub repository: String,
    pub r#ref: String,
    #[serde(default = "checkout_directory")]
    pub directory_within_checkout: String,
}

impl Checkout {
    pub fn new(
        repository: impl Into<String>,
        r#ref: impl Into<String>,
        directory_within_checkout: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let repository = repository.into();
        let r#ref = r#ref.into();
        let directory_within_checkout = directory_within_checkout.into();
        check_text(&repository, "checkout.repository", false)?;
        check_text(&r#ref, "checkout.ref", false)?;
        check_relative_path(
            &directory_within_checkout,
            "checkout.directory_within_checkout",
        )?;
        Ok(Self {
            repository,
            r#ref,
            directory_within_checkout,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stream {
    pub id: String,
    #[serde(deserialize_with = "deserialize_stream_title")]
    pub title: String,
    #[serde(deserialize_with = "required_nullable")]
    pub goal: Option<String>,
    #[serde(rename = "owner", with = "owner")]
    pub owner_host_id: String,
    #[serde(default = "automatic")]
    pub title_mode: String,
    pub goal_short: Option<String>,
    #[serde(default = "automatic")]
    pub goal_mode: String,
    #[serde(default = "active")]
    pub status: String,
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub repositories: BTreeMap<String, Repository>,
    #[serde(default)]
    pub documents: Vec<Document>,
    #[serde(default)]
    pub issues: Vec<Link>,
    #[serde(default)]
    pub prs: Vec<Link>,
}

impl Stream {
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
            title_mode: "automatic".to_string(),
            goal_short: None,
            goal_mode: "automatic".to_string(),
            status: "active".to_string(),
            archived: false,
            repositories: BTreeMap::new(),
            documents: Vec::new(),
            issues: Vec::new(),
            prs: Vec::new(),
        }
        .validated()
    }

    pub fn validated(self) -> Result<Self, ValidationError> {
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
        check_choice(&self.title_mode, "stream.title_mode", NAMING_MODES)?;
        check_choice(&self.goal_mode, "stream.goal_mode", NAMING_MODES)?;
        check_choice(&self.status, "stream.status", STATUSES)?;
        for (alias, repo) in &self.repositories {
            check_text(alias, "repository alias", false)?;
            check_text(&repo.url, "repository.url", false)?;
        }
        let mut doc_ids = BTreeSet::new();
        for doc in &self.documents {
            Document::new(
                &doc.repository,
                &doc.r#ref,
                &doc.path,
                &doc.description,
                doc.added_at.clone(),
            )?;
            if !doc_ids.insert((doc.repository.clone(), doc.path.clone())) {
                return Err(ValidationError::new("duplicate documents"));
            }
        }
        let mut issue_urls = BTreeSet::new();
        for issue in &self.issues {
            Link::new(&issue.url, &issue.description)?;
            if !issue_urls.insert(issue.url.clone()) {
                return Err(ValidationError::new("duplicate issues"));
            }
        }
        let mut pr_urls = BTreeSet::new();
        for pr in &self.prs {
            Link::new(&pr.url, &pr.description)?;
            if !pr_urls.insert(pr.url.clone()) {
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
        Ok(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Session {
    pub id: String,
    pub host_id: String,
    pub stream_id: String,
    pub harness: String,
    pub harness_session_id: String,
    pub title: String,
    #[serde(default = "automatic")]
    pub title_mode: String,
    #[serde(default = "active")]
    pub status: String,
    #[serde(default)]
    pub state: String,
    pub state_short: Option<String>,
    #[serde(default)]
    pub remaining: String,
    pub checkout: Option<Checkout>,
}

impl Session {
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
            harness: harness.into(),
            harness_session_id: harness_session_id.into(),
            title: title.into(),
            title_mode: "automatic".to_string(),
            status: "active".to_string(),
            state: String::new(),
            state_short: None,
            remaining: String::new(),
            checkout: None,
        }
        .validated()
    }

    pub fn validated(self) -> Result<Self, ValidationError> {
        check_text(&self.id, "session.id", false)?;
        check_text(&self.host_id, "session.host_id", false)?;
        check_text(&self.stream_id, "session.stream_id", false)?;
        check_text(&self.harness, "session.harness", false)?;
        check_text(
            &self.harness_session_id,
            "session.harness_session_id",
            false,
        )?;
        check_text(&self.title, "session.title", false)?;
        check_choice(&self.title_mode, "session.title_mode", NAMING_MODES)?;
        check_choice(&self.status, "session.status", STATUSES)?;
        check_text(&self.state, "session.state", true)?;
        check_text(&self.remaining, "session.remaining", true)?;
        if let Some(ref state_short) = self.state_short {
            check_text(state_short, "session.state_short", false)?;
        }
        if let Some(ref co) = self.checkout {
            Checkout::new(&co.repository, &co.r#ref, &co.directory_within_checkout)?;
        }
        Ok(self)
    }

    pub fn native_identity(&self) -> (&str, &str, &str) {
        (&self.host_id, &self.harness, &self.harness_session_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    pub id: String,
    pub label: String,
    #[serde(serialize_with = "serialize_checked_at")]
    pub checked_at: DateTime<FixedOffset>,
}

impl Host {
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        checked_at: DateTime<FixedOffset>,
    ) -> Result<Self, ValidationError> {
        let id = id.into();
        let label = label.into();
        check_text(&id, "host.id", false)?;
        check_text(&label, "host.label", false)?;
        if !checked_at.timestamp_subsec_nanos().is_multiple_of(1000) {
            return Err(ValidationError::new(
                "host.checked_at exceeds microsecond precision",
            ));
        }
        Ok(Self {
            id,
            label,
            checked_at,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Record {
    Stream(Stream),
    Session(Session),
    Host(Host),
}

impl Record {
    pub fn validated(self) -> Result<Self, ValidationError> {
        match self {
            Self::Stream(s) => s.validated().map(Self::Stream),
            Self::Session(s) => s.validated().map(Self::Session),
            Self::Host(h) => Host::new(h.id, h.label, h.checked_at).map(Self::Host),
        }
    }

    pub fn key(&self) -> RecordKey {
        match self {
            Self::Stream(s) => RecordKey::Stream { id: s.id.clone() },
            Self::Session(s) => RecordKey::Session {
                id: s.id.clone(),
                host_id: s.host_id.clone(),
            },
            Self::Host(h) => RecordKey::Host { id: h.id.clone() },
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Record::Stream(_) => "stream",
            Record::Session(_) => "session",
            Record::Host(_) => "host",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum RecordKey {
    // Preserve the lexical kind/id/host ordering used by the catalog wire boundary.
    Host { id: String },
    Session { id: String, host_id: String },
    Stream { id: String },
}

impl RecordKey {
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::Stream { .. } => "stream",
            Self::Session { .. } => "session",
            Self::Host { .. } => "host",
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Stream { id } | Self::Session { id, .. } | Self::Host { id } => id,
        }
    }
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
            Some(host) => write!(f, "{}:{host}/{}", self.kind_str(), self.id()),
            None => write!(f, "{}:{}", self.kind_str(), self.id()),
        }
    }
}

pub fn record_key(record: &Record) -> RecordKey {
    record.key()
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Catalog {
    pub streams: Vec<Stream>,
    pub sessions: Vec<Session>,
    pub hosts: Vec<Host>,
}

impl Catalog {
    pub fn new(
        streams: Vec<Stream>,
        sessions: Vec<Session>,
        hosts: Vec<Host>,
    ) -> Result<Self, ValidationError> {
        let mut stream_keys = BTreeSet::new();
        for s in &streams {
            s.clone().validated()?;
            if !stream_keys.insert(Record::Stream(s.clone()).key()) {
                return Err(ValidationError::new("duplicate streams record identity"));
            }
        }
        let mut session_keys = BTreeSet::new();
        let mut native_ids = BTreeSet::new();
        for s in &sessions {
            s.clone().validated()?;
            if !session_keys.insert(Record::Session(s.clone()).key()) {
                return Err(ValidationError::new("duplicate sessions record identity"));
            }
            if !native_ids.insert((
                s.host_id.clone(),
                s.harness.clone(),
                s.harness_session_id.clone(),
            )) {
                return Err(ValidationError::new("duplicate native session identity"));
            }
        }
        let mut host_keys = BTreeSet::new();
        for h in &hosts {
            Host::new(&h.id, &h.label, h.checked_at)?;
            if !host_keys.insert(Record::Host(h.clone()).key()) {
                return Err(ValidationError::new("duplicate hosts record identity"));
            }
        }
        let stream_map: BTreeMap<&str, &Stream> =
            streams.iter().map(|s| (s.id.as_str(), s)).collect();
        for s in &sessions {
            let Some(stream) = stream_map.get(s.stream_id.as_str()) else {
                return Err(ValidationError::new(format!(
                    "unknown session stream: {}",
                    s.stream_id
                )));
            };
            if let Some(ref co) = s.checkout
                && !stream.repositories.contains_key(&co.repository)
            {
                return Err(ValidationError::new(format!(
                    "unknown checkout repository: {}",
                    co.repository
                )));
            }
        }
        Ok(Self {
            streams,
            sessions,
            hosts,
        })
    }

    pub fn stream(&self, id: &str) -> Option<&Stream> {
        self.streams.iter().find(|s| s.id == id)
    }

    pub fn session(&self, host_id: &str, id: &str) -> Option<&Session> {
        self.sessions
            .iter()
            .find(|s| s.host_id == host_id && s.id == id)
    }

    pub fn host(&self, id: &str) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    pub fn records(&self) -> Vec<Record> {
        let mut out =
            Vec::with_capacity(self.streams.len() + self.sessions.len() + self.hosts.len());
        out.extend(self.streams.iter().cloned().map(Record::Stream));
        out.extend(self.sessions.iter().cloned().map(Record::Session));
        out.extend(self.hosts.iter().cloned().map(Record::Host));
        out
    }

    pub fn get(&self, key: &RecordKey) -> Option<Record> {
        match key {
            RecordKey::Stream { id } => self.stream(id).cloned().map(Record::Stream),
            RecordKey::Session { host_id, id } => {
                self.session(host_id, id).cloned().map(Record::Session)
            }
            RecordKey::Host { id } => self.host(id).cloned().map(Record::Host),
        }
    }

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
