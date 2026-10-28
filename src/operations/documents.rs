//! Session-evidenced additions, never replacement or refresh of the shared reading list.

use serde::Deserialize;

use super::Reconciliation;
use crate::model::{Document, Record, Session, Stream, ValidationError, check_text};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentProposal {
    pub repository: String,
    #[serde(default)]
    pub r#ref: Option<String>,
    pub path: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct DocumentAdditions {
    operation_id: String,
    before: Stream,
    session: Session,
    documents: Vec<Document>,
}

impl DocumentAdditions {
    pub fn new(
        operation_id: &str,
        before: &Stream,
        session: &Session,
        proposals: Vec<DocumentProposal>,
    ) -> Result<Self, ValidationError> {
        check_text(operation_id, "operation_id", false)?;
        before.validate()?;
        session.validate_portable()?;
        if session.stream_id != before.id || session.host_id != before.owner_host_id {
            return Err(ValidationError::new(
                "document additions require their captured Stream and Session",
            ));
        }
        let mut documents = Vec::new();
        for proposal in proposals {
            let document = Document {
                repository: proposal.repository,
                r#ref: proposal.r#ref,
                path: proposal.path,
                description: proposal.description,
                commit: None,
                source_session_id: Some(session.id.clone()),
            };
            document.validate()?;
            if !before.repositories.contains_key(&document.repository) {
                return Err(ValidationError::new("unknown document repository alias"));
            }
            // An echoed existing link is not permission to restore it after manual removal.
            if !before
                .documents
                .iter()
                .any(|old| old.identity() == document.identity())
                && !documents
                    .iter()
                    .any(|old: &Document| old.identity() == document.identity())
            {
                documents.push(document);
            }
        }
        Ok(Self {
            operation_id: operation_id.into(),
            before: before.clone(),
            session: session.clone(),
            documents,
        })
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn before(&self) -> &Stream {
        &self.before
    }
    pub fn captured_session(&self) -> &Session {
        &self.session
    }

    pub fn apply(&self, current: &Stream, session: Option<&Session>) -> Reconciliation {
        let same_period = session.is_some_and(|session| {
            session.id == self.session.id
                && session.native_identity() == self.session.native_identity()
                && session.stream_id == self.session.stream_id
                && session.previous_stream_ids == self.session.previous_stream_ids
        });
        if current.id != self.before.id
            || current.owner_host_id != self.before.owner_host_id
            || !same_period
            || self.documents.iter().any(|document| {
                current.repositories.get(&document.repository)
                    != self.before.repositories.get(&document.repository)
            })
        {
            return Reconciliation::Superseded {
                operation_id: self.operation_id.clone(),
                conflicts: vec!["inputs".into()],
                reason: "captured document membership or repository binding changed".into(),
            };
        }
        let mut next = current.clone();
        for document in &self.documents {
            if !next
                .documents
                .iter()
                .any(|old| old.identity() == document.identity())
            {
                next.documents.push(document.clone());
            }
        }
        if next == *current {
            Reconciliation::no_change(&self.operation_id, "no new document identities")
        } else {
            Reconciliation::Changes {
                operation_id: self.operation_id.clone(),
                record: Box::new(Record::Stream(next)),
            }
        }
    }
}
