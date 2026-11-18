//! Session-owned attachment metadata and separately stored output text.

use crate::{
    model::{Session, ValidationError, check_text},
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordRead, RecordReader, RecordSave, RecordStore},
    },
};
use serde::{Deserialize, Serialize};

pub const MAX_CONTENT_BYTES: usize = 1024 * 1024;

/// Typed child and CLI intents. Missing Session IDs select the child's bound Session only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactRequest {
    Write {
        name: String,
        description: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<std::path::PathBuf>,
    },
    Read {
        reference: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        offset: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        limit: Option<usize>,
    },
    ListSession {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stream_id: Option<String>,
    },
    ListStream {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
        stream_id: String,
    },
    Delete {
        reference: String,
    },
}

pub(crate) fn groups<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<std::collections::BTreeMap<String, Vec<ArtifactEntry>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Entries(#[serde(deserialize_with = "crate::codec::objects")] Vec<ArtifactEntry>);
    std::collections::BTreeMap::<String, Entries>::deserialize(deserializer).map(|groups| {
        groups
            .into_iter()
            .map(|(stream, entries)| (stream, entries.0))
            .collect()
    })
}

pub fn validate_name(name: &str) -> Result<(), ValidationError> {
    let mut bytes = name.bytes();
    if !(1..=64).contains(&name.len())
        || !bytes
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        || !bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
    {
        return Err(ValidationError::new(
            "artifact name must be 1–64 lowercase ASCII letters, digits, dots, underscores or hyphens, starting with a letter or digit",
        ));
    }
    Ok(())
}

pub fn validate_content(content: &str) -> Result<(), ValidationError> {
    if content.len() > MAX_CONTENT_BYTES {
        return Err(ValidationError::new("artifact content exceeds 1 MiB"));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEntry {
    pub name: String,
    pub description: String,
}

impl ArtifactEntry {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_name(&self.name)?;
        check_text(&self.description, "artifact description", false)?;
        if self.description.chars().count() > 240 || self.description.chars().any(char::is_control)
        {
            return Err(ValidationError::new(
                "artifact description must be one line, at most 240 characters",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactKey {
    pub host: String,
    pub stream: String,
    pub session: String,
    pub name: String,
}

impl ArtifactKey {
    pub fn validate(&self) -> Result<(), ValidationError> {
        for (label, value) in [
            ("artifact host", &self.host),
            ("artifact Stream", &self.stream),
            ("artifact Session", &self.session),
        ] {
            check_text(value, label, false)?;
        }
        validate_name(&self.name)
    }

    pub fn reference(&self) -> String {
        [&self.host, &self.stream, &self.session, &self.name]
            .map(|part| crate::file_store::encode_component(part))
            .join("/")
    }

    pub fn parse(reference: &str) -> Result<Self, ValidationError> {
        let parts = reference
            .split('/')
            .map(|part| {
                percent_encoding::percent_decode_str(part)
                    .decode_utf8()
                    .map(|s| s.into_owned())
                    .map_err(|_| ValidationError::new("artifact reference must be UTF-8"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let [host, stream, session, name]: [String; 4] = parts.try_into().map_err(|_| {
            ValidationError::new("artifact reference must be host/stream/session/name")
        })?;
        let key = Self {
            host,
            stream,
            session,
            name,
        };
        key.validate()?;
        if key.reference() != reference {
            return Err(ValidationError::new(
                "artifact reference must use canonical component encoding",
            ));
        }
        Ok(key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSummary {
    pub key: ArtifactKey,
    pub reference: String,
    pub description: String,
}

pub fn summaries(session: &Session, stream: &str) -> Vec<ArtifactSummary> {
    session
        .artifacts
        .get(stream)
        .into_iter()
        .flatten()
        .map(|entry| {
            let key = ArtifactKey {
                host: session.host_id.clone(),
                stream: stream.into(),
                session: session.id.clone(),
                name: entry.name.clone(),
            };
            ArtifactSummary {
                reference: key.reference(),
                key,
                description: entry.description.clone(),
            }
        })
        .collect()
}

pub fn entry<'a>(session: &'a Session, key: &ArtifactKey) -> Option<&'a ArtifactEntry> {
    session
        .artifacts
        .get(&key.stream)?
        .iter()
        .find(|entry| entry.name == key.name)
}

pub enum ContentChange {
    Keep,
    Write(String),
    Delete,
}

pub struct ArtifactSave<T> {
    pub save: RecordSave<T>,
    pub warnings: Vec<String>,
}

pub trait ArtifactReader: RecordReader {
    fn read_content(&self, key: &ArtifactKey) -> Result<Option<String>, StorageError>;
    fn stream_producers(&self, host: &str, stream: &str) -> Result<Vec<String>, StorageError>;
}

pub trait ArtifactStore: RecordStore {
    fn with_artifacts<T, E: From<StorageError>>(
        &self,
        read: impl FnOnce(&dyn ArtifactReader) -> Result<T, E>,
    ) -> Result<T, E>;
    fn save_artifact<T, E: From<StorageError> + std::fmt::Display>(
        &self,
        target: &RecordAddress,
        content: ContentChange,
        apply: impl FnOnce(
            &dyn RecordReader,
            Option<&RecordRead>,
        ) -> Result<(RecordChange<T>, ArtifactKey), E>,
    ) -> Result<ArtifactSave<T>, E>;
}

/// Pure metadata change. Moving a Session leaves all producing-Stream groups intact.
pub fn edit(
    session: &Session,
    key: &ArtifactKey,
    description: Option<&str>,
    has_content: bool,
) -> Result<Session, ValidationError> {
    key.validate()?;
    if session.host_id != key.host || session.id != key.session {
        return Err(ValidationError::new(
            "artifact producer does not match Session",
        ));
    }
    let exists = entry(session, key).is_some();
    if !exists && description.is_some() && (session.stream_id != key.stream || !has_content) {
        return Err(ValidationError::new(
            "new artifacts require content and current Stream membership",
        ));
    }
    let mut after = session.clone();
    if let Some(description) = description {
        let replacement = ArtifactEntry {
            name: key.name.clone(),
            description: description.into(),
        };
        replacement.validate()?;
        let entries = after.artifacts.entry(key.stream.clone()).or_default();
        if let Some(entry) = entries.iter_mut().find(|entry| entry.name == key.name) {
            *entry = replacement;
        } else {
            entries.push(replacement);
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
    } else if let Some(entries) = after.artifacts.get_mut(&key.stream) {
        entries.retain(|entry| entry.name != key.name);
        if entries.is_empty() {
            after.artifacts.remove(&key.stream);
        }
    }
    after.validate()?;
    Ok(after)
}
