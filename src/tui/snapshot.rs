use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::model::{
    ReferenceEntry, ReferenceKind, ReferenceStatus, Session, Stream, ValidationError, check_text,
};

/// Exact viewing contribution and catalog identity, not a native ownership witness.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Scope {
    pub catalog_source: String,
    pub host_id: String,
    pub stream_id: String,
    pub session_id: String,
    pub harness: String,
    pub native_session_id: String,
}

impl Scope {
    /// Native identity survives membership changes.
    pub fn same_conversation(&self, other: &Self) -> bool {
        self.catalog_source == other.catalog_source
            && self.host_id == other.host_id
            && self.harness == other.harness
            && self.native_session_id == other.native_session_id
    }
}

/// Supplied display context; callers must separately obtain live native attestation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeOwnerObservation {
    pub pane_id: String,
    pub tab_id: String,
    pub workspace_id: String,
}

/// Mutually exclusive app-resolved file and unavailable-location observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDocument {
    File { root: PathBuf, file: PathBuf },
    Unavailable { reason: String },
}

/// Exactly one supplied location per visible Session document reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentLocation {
    pub path: String,
    pub location: ResolvedDocument,
}

/// Read-only context supplied by the application, without filesystem or native access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayContext {
    pub catalog_source: String,
    pub owner: NativeOwnerObservation,
    pub documents: Vec<DocumentLocation>,
}

/// Validated viewing records and identity, without document-location observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRecords {
    pub(super) stream: Stream,
    pub(super) session: Session,
    pub(super) scope: Scope,
    pub(super) owner: NativeOwnerObservation,
}

/// Validated owned display data, never a catalog save precondition or effect capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSnapshot {
    pub(super) records: CardRecords,
    pub(super) context: DisplayContext,
}

/// A painted web-link reference on the viewing Session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkTarget {
    pub url: String,
    pub entry: ReferenceEntry,
}

fn absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|text| !text.contains('\0'))
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
}

impl CardRecords {
    pub fn new(
        stream: Stream,
        session: Session,
        catalog_source: String,
        owner: NativeOwnerObservation,
    ) -> Result<Self, ValidationError> {
        stream.validate()?;
        session.validate()?;
        if session.stream_id != stream.id || session.host_id != stream.host_id {
            return Err(ValidationError::new(
                "Viewing session belongs to another stream",
            ));
        }
        for value in [
            &catalog_source,
            &owner.pane_id,
            &owner.tab_id,
            &owner.workspace_id,
        ] {
            check_text(value, "display identity", false)?;
        }
        let scope = Scope {
            catalog_source,
            host_id: session.host_id.clone(),
            stream_id: stream.id.clone(),
            session_id: session.id.clone(),
            harness: session.harness.clone(),
            native_session_id: session.id.clone(),
        };
        Ok(Self {
            stream,
            session,
            scope,
            owner,
        })
    }
    pub fn scope(&self) -> &Scope {
        &self.scope
    }
    pub fn stream(&self) -> &Stream {
        &self.stream
    }
    pub fn session(&self) -> &Session {
        &self.session
    }
    pub fn owner(&self) -> &NativeOwnerObservation {
        &self.owner
    }
    /// Reuse the painter's URL policy when validating an external action.
    pub fn contains_link(&self, link: &LinkTarget) -> bool {
        link.entry.status != ReferenceStatus::Dropped
            && link.entry.kind.is_url()
            && super::render::web_url(&link.url).is_some()
            && self.session.references.get(&link.url) == Some(&link.entry)
    }
}

impl CardSnapshot {
    /// Checks domain fields, exact contribution membership, and complete location bindings.
    pub fn new(
        stream: Stream,
        session: Session,
        context: DisplayContext,
    ) -> Result<Self, ValidationError> {
        let records = CardRecords::new(stream, session, context.catalog_source, context.owner)?;
        Self::from_records(records, context.documents)
    }

    pub fn from_records(
        records: CardRecords,
        documents: Vec<DocumentLocation>,
    ) -> Result<Self, ValidationError> {
        let session = records.session();
        let visible_documents: Vec<&str> = session
            .references
            .iter()
            .filter(|(_, entry)| {
                entry.status != ReferenceStatus::Dropped && entry.kind == ReferenceKind::Document
            })
            .map(|(path, _)| path.as_str())
            .collect();
        if documents.len() != visible_documents.len() {
            return Err(ValidationError::new(
                "Exactly one app location is required per document",
            ));
        }
        let mut identities = BTreeSet::new();
        for location in &documents {
            if !identities.insert(location.path.as_str())
                || !visible_documents.contains(&location.path.as_str())
            {
                return Err(ValidationError::new(
                    "Unknown or duplicate app document location",
                ));
            }
            match &location.location {
                ResolvedDocument::File { root, file }
                    if !absolute(root)
                        || !absolute(file)
                        || !file.starts_with(root)
                        || file == root =>
                {
                    return Err(ValidationError::new(
                        "App file must be inside the Session's directory",
                    ));
                }
                ResolvedDocument::Unavailable { reason } => {
                    check_text(reason, "document error", false)?;
                }
                _ => {}
            }
        }
        let context = DisplayContext {
            catalog_source: records.scope().catalog_source.clone(),
            owner: records.owner().clone(),
            documents,
        };
        Ok(Self { records, context })
    }

    pub fn into_records(self) -> CardRecords {
        self.records
    }
    pub fn scope(&self) -> &Scope {
        self.records.scope()
    }
    pub fn stream(&self) -> &Stream {
        self.records.stream()
    }
    pub fn session(&self) -> &Session {
        self.records.session()
    }
    pub fn contains_link(&self, link: &LinkTarget) -> bool {
        self.records.contains_link(link)
    }
    pub fn owner(&self) -> &NativeOwnerObservation {
        self.records.owner()
    }
    pub fn document_target(&self, path: &str) -> Option<DocumentTarget> {
        let entry = self.records.session.references.get(path)?;
        if entry.status == ReferenceStatus::Dropped || entry.kind != ReferenceKind::Document {
            return None;
        }
        Some(self.target(path, entry))
    }

    pub(super) fn target(&self, path: &str, entry: &ReferenceEntry) -> DocumentTarget {
        let location = self
            .context
            .documents
            .iter()
            .find(|l| l.path == path)
            .expect("snapshot construction checks complete locations");
        DocumentTarget {
            scope: self.scope().clone(),
            owner: self.owner().clone(),
            path: path.to_owned(),
            entry: entry.clone(),
            location: location.location.clone(),
        }
    }
}

/// Exact painted reference and app location for the native action guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTarget {
    pub scope: Scope,
    pub owner: NativeOwnerObservation,
    pub path: String,
    pub entry: ReferenceEntry,
    pub location: ResolvedDocument,
}

/// Editor reuse identity on the local host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorIdentity {
    pub catalog_source: String,
    pub host_id: String,
    pub file: PathBuf,
}

impl DocumentTarget {
    pub fn path(&self) -> &str {
        &self.path
    }
    /// Returns only the app-observed identity, not permission to open or reuse an editor.
    pub fn editor_identity(&self) -> Result<EditorIdentity, &str> {
        match &self.location {
            ResolvedDocument::File { file, .. } => Ok(EditorIdentity {
                catalog_source: self.scope.catalog_source.clone(),
                host_id: self.scope.host_id.clone(),
                file: file.clone(),
            }),
            ResolvedDocument::Unavailable { reason } => Err(reason.as_str()),
        }
    }
}
