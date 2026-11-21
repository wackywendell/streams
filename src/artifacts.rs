//! Session-owned artifact metadata and separately stored output text.
//! An artifact's key is `HOST/HARNESS/ID/NAME`, stored at `hosts/HOST/sessions/HARNESS/ID/artifacts/NAME`.

use crate::{
    model::{Record, Session, ValidationError, check_text, check_token},
    storage::{
        StorageError,
        records::{RecordAddress, RecordChange, RecordReader, RecordSave, RecordStore},
    },
};
use serde::{Deserialize, Serialize};

pub const MAX_CONTENT_BYTES: usize = 1024 * 1024;

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

/// Stored value in `Session.artifacts[NAME]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactEntry {
    pub description: String,
}

impl ArtifactEntry {
    pub fn new(description: impl Into<String>) -> Result<Self, ValidationError> {
        let entry = Self {
            description: description.into(),
        };
        entry.validate()?;
        Ok(entry)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
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

/// Full artifact identity: `HOST/HARNESS/ID/NAME`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactKey {
    pub host: String,
    pub harness: String,
    pub session: String,
    pub name: String,
}

impl ArtifactKey {
    pub fn validate(&self) -> Result<(), ValidationError> {
        check_token(&self.host, "artifact host")?;
        check_token(&self.harness, "artifact harness")?;
        check_token(&self.session, "artifact Session")?;
        validate_name(&self.name)
    }

    pub fn reference(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.host, self.harness, self.session, self.name
        )
    }

    pub fn session_address(&self) -> Result<RecordAddress, StorageError> {
        RecordAddress::session(&self.host, &self.harness, &self.session)
    }

    pub fn parse(reference: &str) -> Result<Self, ValidationError> {
        let parts: Vec<&str> = reference.split('/').collect();
        let [host, harness, session, name]: [&str; 4] = parts
            .try_into()
            .map_err(|_| ValidationError::new("artifact key must be HOST/HARNESS/ID/NAME"))?;
        let key = Self {
            host: host.into(),
            harness: harness.into(),
            session: session.into(),
            name: name.into(),
        };
        key.validate()?;
        Ok(key)
    }
}

impl std::fmt::Display for ArtifactKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reference())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactSummary {
    pub key: ArtifactKey,
    pub name: String,
    pub description: String,
}

impl ArtifactSummary {
    pub fn full_key(&self) -> String {
        self.key.reference()
    }
}

pub fn summaries(session: &Session) -> Vec<ArtifactSummary> {
    session
        .artifacts
        .iter()
        .map(|(name, entry)| ArtifactSummary {
            key: ArtifactKey {
                host: session.host_id.clone(),
                harness: session.harness.clone(),
                session: session.id.clone(),
                name: name.clone(),
            },
            name: name.clone(),
            description: entry.description.clone(),
        })
        .collect()
}

pub fn entry<'a>(session: &'a Session, key: &ArtifactKey) -> Option<&'a ArtifactEntry> {
    if session.host_id != key.host || session.harness != key.harness || session.id != key.session {
        return None;
    }
    session.artifacts.get(&key.name)
}

pub enum ContentChange {
    Keep,
    Write(String),
    Delete,
}

pub struct ArtifactSave<T> {
    pub save: RecordSave<T>,
    pub content_written: bool,
    pub warnings: Vec<String>,
}

pub trait ArtifactReader: RecordReader {
    fn read_content(&self, key: &ArtifactKey) -> Result<Option<String>, StorageError>;
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
            Option<&Record>,
        ) -> Result<(RecordChange<T>, ArtifactKey), E>,
    ) -> Result<ArtifactSave<T>, E>;
}

/// Pure metadata change on a Session's `artifacts` map.
pub fn edit(
    session: &Session,
    key: &ArtifactKey,
    description: Option<&str>,
    has_content: bool,
) -> Result<Session, ValidationError> {
    key.validate()?;
    if session.host_id != key.host || session.harness != key.harness || session.id != key.session {
        return Err(ValidationError::new("artifact key does not match Session"));
    }
    let exists = session.artifacts.contains_key(&key.name);
    if !exists && description.is_some() && !has_content {
        return Err(ValidationError::new("new artifacts require content"));
    }
    let mut after = session.clone();
    if let Some(description) = description {
        let replacement = ArtifactEntry::new(description)?;
        after.artifacts.insert(key.name.clone(), replacement);
    } else {
        after.artifacts.remove(&key.name);
    }
    after.validate()?;
    Ok(after)
}
