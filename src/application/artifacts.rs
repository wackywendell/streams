//! Record-local artifact operations. Metadata lives in `Session.artifacts`; content lives at
//! `hosts/HOST/sessions/HARNESS/ID/artifacts/NAME`.

use serde::Serialize;

use super::{ApplicationError, OpenedRecords, Result, fields::ArtifactRow};
use crate::{
    artifacts::{
        self, ArtifactEntry, ArtifactKey, ArtifactStore, ArtifactSummary, ContentChange,
        validate_name,
    },
    model::{Record, Session, ValidationError},
    storage::records::{RecordAddress, RecordChange, RecordReader, RecordStore},
};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Serialize)]
pub struct WriteArtifact {
    pub harness: String,
    pub session_id: String,
    pub name: String,
    pub description: String,
    pub content: Option<String>,
}

fn read_session(reader: &dyn RecordReader, host: &str, harness: &str, id: &str) -> Result<Session> {
    let address = RecordAddress::session(host, harness, id)?;
    match reader.read(&address)? {
        Some(Record::Session(session)) => Ok(session),
        _ => Err(ApplicationError::not_found(format!(
            "no Session {host}/{harness}/{id}"
        ))),
    }
}

pub fn list_session_artifacts(
    store: &impl ArtifactStore,
    host: &str,
    harness: &str,
    session_id: &str,
) -> Result<Vec<ArtifactRow>> {
    store.with_artifacts(|reader| {
        let session = read_session(reader, host, harness, session_id)?;
        let mut rows: Vec<ArtifactRow> = artifacts::summaries(&session)
            .into_iter()
            .map(|summary| ArtifactRow::from_session(&session, &summary.key, &summary.description))
            .collect();
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(rows)
    })
}

pub fn list_stream_artifacts(
    opened: &OpenedRecords,
    host: &str,
    stream_id: &str,
) -> Result<(
    Vec<ArtifactRow>,
    Vec<crate::storage::records::UnreadableRecord>,
)> {
    let stream_addr = RecordAddress::stream(host, stream_id)?;
    let stream_exists = opened
        .records()
        .with_reader(|reader| reader.read(&stream_addr))?
        .is_some();
    if !stream_exists {
        return Err(ApplicationError::not_found(format!(
            "no Stream {host}/{stream_id}"
        )));
    }
    let listing = opened.sessions_for_host(host)?;
    let mut rows = Vec::new();
    for record in listing.records {
        let Record::Session(session) = record else {
            continue;
        };
        if session.stream_id != stream_id {
            continue;
        }
        for summary in artifacts::summaries(&session) {
            rows.push(ArtifactRow::from_session(
                &session,
                &summary.key,
                &summary.description,
            ));
        }
    }
    rows.sort_by(|a, b| a.key.cmp(&b.key));
    Ok((rows, listing.unreadable))
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRead {
    pub artifact: ArtifactSummary,
    pub content: String,
}

pub fn read_artifact(store: &impl ArtifactStore, key: &ArtifactKey) -> Result<ArtifactRead> {
    key.validate()?;
    store.with_artifacts(|reader| {
        let producer = read_session(reader, &key.host, &key.harness, &key.session)?;
        let artifact = artifacts::summaries(&producer)
            .into_iter()
            .find(|item| item.key == *key)
            .ok_or_else(|| ApplicationError::not_found(format!("no Artifact {key}")))?;
        let content = reader
            .read_content(key)?
            .ok_or_else(|| ValidationError::new("attached artifact content is missing"))?;
        Ok(ArtifactRead { artifact, content })
    })
}

fn before_session<'a>(before: Option<&'a Record>, target: &RecordAddress) -> Result<&'a Session> {
    match before {
        Some(Record::Session(session)) => Ok(session),
        _ => Err(ApplicationError::not_found(format!(
            "no Session {}",
            target.full_key()
        ))),
    }
}

/// What an artifact save reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactOutcome {
    Artifact(ArtifactSummary),
    Deleted(String),
}

pub fn write_artifact(
    store: &impl ArtifactStore,
    host: &str,
    intent: &WriteArtifact,
) -> Result<artifacts::ArtifactSave<ArtifactOutcome>> {
    validate_name(&intent.name)?;
    ArtifactEntry {
        description: intent.description.clone(),
    }
    .validate()?;
    if let Some(content) = &intent.content {
        artifacts::validate_content(content)?;
    }
    let target = RecordAddress::session(host, &intent.harness, &intent.session_id)?;
    let content = intent
        .content
        .clone()
        .map(ContentChange::Write)
        .unwrap_or(ContentChange::Keep);
    store.save_artifact(&target, content, |_reader, before| {
        let session = before_session(before, &target)?;
        let key = ArtifactKey {
            host: host.into(),
            harness: session.harness.clone(),
            session: session.id.clone(),
            name: intent.name.clone(),
        };
        let after = artifacts::edit(
            session,
            &key,
            Some(&intent.description),
            intent.content.is_some(),
        )?;
        let summary = ArtifactSummary {
            key: key.clone(),
            name: key.name.clone(),
            description: intent.description.clone(),
        };
        Ok((
            RecordChange {
                record: Record::Session(after),
                outcome: ArtifactOutcome::Artifact(summary),
            },
            key,
        ))
    })
}

pub fn delete_artifact(
    store: &impl ArtifactStore,
    key: &ArtifactKey,
) -> Result<artifacts::ArtifactSave<ArtifactOutcome>> {
    key.validate()?;
    let target = key.session_address()?;
    store.save_artifact(&target, ContentChange::Delete, |_, before| {
        let after = artifacts::edit(before_session(before, &target)?, key, None, false)?;
        Ok((
            RecordChange {
                record: Record::Session(after),
                outcome: ArtifactOutcome::Deleted(key.reference()),
            },
            key.clone(),
        ))
    })
}

/// Bounded regular-file input for `artifact write --file`.
pub fn artifact_file(path: &std::path::Path) -> Result<String> {
    use std::{io::Read, os::unix::fs::OpenOptionsExt};
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| crate::storage::StorageError::new(error.to_string()))?;
    if !file
        .metadata()
        .map_err(|error| crate::storage::StorageError::new(error.to_string()))?
        .is_file()
    {
        return Err(ValidationError::new("artifact input must be a regular file").into());
    }
    let mut bytes = Vec::new();
    file.take((artifacts::MAX_CONTENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| crate::storage::StorageError::new(error.to_string()))?;
    let text = String::from_utf8(bytes)
        .map_err(|_| ValidationError::new("artifact input must be UTF-8"))?;
    artifacts::validate_content(&text)?;
    Ok(text)
}
