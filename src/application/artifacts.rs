//! Record-local attachment operations. Metadata lives only in the producing Session.

use super::Result;

#[cfg(test)]
mod tests;
use crate::{
    artifacts::{self, ArtifactKey, ArtifactStore, ArtifactSummary, ContentChange},
    model::{Record, RecordKind, Session, ValidationError},
    storage::records::{RecordAddress, RecordChange, RecordRead, RecordReader, RecordRequest},
};
use serde::Serialize;
use serde_json::json;

#[derive(Debug, Clone, Serialize)]
pub struct WriteArtifact {
    pub session_id: String,
    pub stream_id: Option<String>,
    pub name: String,
    pub description: String,
    pub content: Option<String>,
}

fn session(reader: &dyn RecordReader, host: &str, id: &str) -> Result<Session> {
    let key = RecordAddress::new(host, RecordKind::Session, id)?;
    match reader.read(&key)? {
        Some(RecordRead {
            record: Record::Session(session),
            ..
        }) => Ok(session),
        _ => Err(ValidationError::new("producing Session is missing").into()),
    }
}

pub fn list_session_artifacts(
    store: &impl ArtifactStore,
    host: &str,
    session_id: &str,
    stream: Option<&str>,
) -> Result<Vec<ArtifactSummary>> {
    store.with_artifacts(|reader| {
        let session = session(reader, host, session_id)?;
        Ok(artifacts::summaries(
            &session,
            stream.unwrap_or(&session.stream_id),
        ))
    })
}

pub fn list_stream_artifacts(
    store: &impl ArtifactStore,
    host: &str,
    stream: &str,
) -> Result<Vec<ArtifactSummary>> {
    store.with_artifacts(|reader| {
        let key = RecordAddress::new(host, RecordKind::Stream, stream)?;
        if reader.read(&key)?.is_none() {
            return Err(ValidationError::new("producing Stream is missing").into());
        }
        let mut outputs = Vec::new();
        for id in reader.stream_producers(host, stream)? {
            // An unattached leftover directory is not an artifact reference.
            let key = RecordAddress::new(host, RecordKind::Session, id)?;
            if let Some(RecordRead {
                record: Record::Session(session),
                ..
            }) = reader.read(&key)?
            {
                outputs.extend(artifacts::summaries(&session, stream));
            }
        }
        outputs.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(outputs)
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct ArtifactRead {
    pub artifact: ArtifactSummary,
    pub content: String,
}

pub fn read_artifact(store: &impl ArtifactStore, key: &ArtifactKey) -> Result<ArtifactRead> {
    key.validate()?;
    store.with_artifacts(|reader| {
        let producer = session(reader, &key.host, &key.session)?;
        let artifact = artifacts::summaries(&producer, &key.stream)
            .into_iter()
            .find(|item| item.key == *key)
            .ok_or_else(|| ValidationError::new("artifact is not attached to this Session"))?;
        let content = reader
            .read_content(key)?
            .ok_or_else(|| ValidationError::new("attached artifact content is missing"))?;
        Ok(ArtifactRead { artifact, content })
    })
}

fn before_session(before: Option<&RecordRead>) -> Result<&Session> {
    match before {
        Some(RecordRead {
            record: Record::Session(session),
            ..
        }) => Ok(session),
        _ => Err(ValidationError::new("producing Session is missing").into()),
    }
}

pub fn write_artifact(
    store: &impl ArtifactStore,
    host: &str,
    operation_id: &str,
    intent: &WriteArtifact,
) -> Result<artifacts::ArtifactSave> {
    artifacts::ArtifactEntry {
        name: intent.name.clone(),
        description: intent.description.clone(),
    }
    .validate()?;
    if let Some(content) = &intent.content {
        artifacts::validate_content(content)?;
    }
    let target = RecordAddress::new(host, RecordKind::Session, &intent.session_id)?;
    let request = RecordRequest::new(
        target,
        operation_id,
        &json!({"action":"write_artifact", "intent":intent}),
    )?;
    let content = intent
        .content
        .clone()
        .map(ContentChange::Write)
        .unwrap_or(ContentChange::Keep);
    store.save_artifact(&request, content, |reader, before| {
        let session = before_session(before)?;
        let key = ArtifactKey {
            host: host.into(),
            session: session.id.clone(),
            stream: intent
                .stream_id
                .clone()
                .unwrap_or_else(|| session.stream_id.clone()),
            name: intent.name.clone(),
        };
        let after = artifacts::edit(
            session,
            &key,
            Some(&intent.description),
            intent.content.is_some(),
        )?;
        let stream_key = RecordAddress::new(host, RecordKind::Stream, &key.stream)?;
        if reader.read(&stream_key)?.is_none() {
            return Err(ValidationError::new("producing Stream is missing").into());
        }
        let summary = artifacts::summaries(&after, &key.stream)
            .into_iter()
            .find(|item| item.key == key)
            .expect("write adds metadata");
        let outcome = json!({"artifact":summary});
        Ok((
            RecordChange {
                record: Record::Session(after),
                outcome: outcome.as_object().expect("object").clone(),
            },
            key,
        ))
    })
}

pub fn delete_artifact(
    store: &impl ArtifactStore,
    operation_id: &str,
    key: &ArtifactKey,
) -> Result<artifacts::ArtifactSave> {
    key.validate()?;
    let target = RecordAddress::new(&key.host, RecordKind::Session, &key.session)?;
    let request = RecordRequest::new(
        target,
        operation_id,
        &json!({"action":"delete_artifact", "key":key}),
    )?;
    store.save_artifact(&request, ContentChange::Delete, |_, before| {
        let after = artifacts::edit(before_session(before)?, key, None, false)?;
        let outcome = json!({"deleted":key.reference()});
        Ok((
            RecordChange {
                record: Record::Session(after),
                outcome: outcome.as_object().expect("object").clone(),
            },
            key.clone(),
        ))
    })
}

/// Bounded regular-file input, shared by CLI and the child. Stdin is supplied by the CLI.
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
