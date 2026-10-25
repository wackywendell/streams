use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::model::{Document, Session, Stream, ValidationError, check_text};

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
    /// Native identity survives membership changes and record-ID migration.
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

/// An application-selected checkout. Paths are observations, never filesystem proofs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSelection {
    pub checkout_id: String,
    pub root: PathBuf,
}

/// Repository alias and the viewing session's selected checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutBinding {
    pub repository: String,
    pub selection: CheckoutSelection,
}

/// Application-reported document failure. References remain visible and selectable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentProblem {
    MissingDocument,
    MissingCheckout,
    AmbiguousCheckout,
    Other(String),
}

impl std::fmt::Display for DocumentProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingDocument => f.write_str("Document not present in this checkout"),
            Self::MissingCheckout => f.write_str("No registered checkout"),
            Self::AmbiguousCheckout => f.write_str("Ambiguous viewing-session checkout binding"),
            Self::Other(message) => f.write_str(message),
        }
    }
}

/// Mutually exclusive app-resolved file and unavailable-location observations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedDocument {
    File {
        selection: CheckoutSelection,
        file: PathBuf,
    },
    Unavailable {
        selection: Option<CheckoutSelection>,
        problem: DocumentProblem,
    },
}

impl ResolvedDocument {
    pub(super) fn selection(&self) -> Option<&CheckoutSelection> {
        match self {
            Self::File { selection, .. } => Some(selection),
            Self::Unavailable { selection, .. } => selection.as_ref(),
        }
    }
}

/// Exactly one supplied location per stream document identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentLocation {
    pub repository: String,
    pub path: String,
    pub location: ResolvedDocument,
}

/// Read-only context supplied by the application, without filesystem or native access.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayContext {
    pub catalog_source: String,
    pub owner: NativeOwnerObservation,
    pub checkouts: Vec<CheckoutBinding>,
    pub documents: Vec<DocumentLocation>,
}

/// Validated owned display data, never a catalog save precondition or effect capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSnapshot {
    pub(super) stream: Stream,
    pub(super) session: Session,
    pub(super) scope: Scope,
    pub(super) context: DisplayContext,
}

fn absolute(path: &Path) -> bool {
    path.is_absolute()
        && path.to_str().is_some_and(|text| !text.contains('\0'))
        && path.components().collect::<PathBuf>().as_os_str() == path.as_os_str()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
}

impl CardSnapshot {
    /// Checks domain fields, exact contribution membership, and complete location bindings.
    /// This validates supplied data only; no path or native tuple is attested here.
    pub fn new(
        stream: Stream,
        session: Session,
        context: DisplayContext,
    ) -> Result<Self, ValidationError> {
        stream.validate()?;
        session.validate()?;
        if session.stream_id != stream.id {
            return Err(ValidationError::new(
                "Viewing session belongs to another stream",
            ));
        }
        if let Some(checkout) = &session.checkout
            && checkout.repository_url(&stream).is_none()
        {
            return Err(ValidationError::new(
                "Viewing session checkout has no repository URL or known alias",
            ));
        }
        for value in [
            &context.catalog_source,
            &context.owner.pane_id,
            &context.owner.tab_id,
            &context.owner.workspace_id,
        ] {
            check_text(value, "display identity", false)?;
        }
        let mut bindings = BTreeSet::new();
        for binding in &context.checkouts {
            check_text(&binding.selection.checkout_id, "checkout identity", false)?;
            if !stream.repositories.contains_key(&binding.repository)
                || !absolute(&binding.selection.root)
                || !bindings.insert((&binding.repository, &binding.selection.checkout_id))
            {
                return Err(ValidationError::new(
                    "Invalid or duplicate display checkout binding",
                ));
            }
        }
        if context.documents.len() != stream.documents.len() {
            return Err(ValidationError::new(
                "Exactly one app location is required per document",
            ));
        }
        let mut identities = BTreeSet::new();
        for location in &context.documents {
            if !identities.insert((&location.repository, &location.path))
                || !stream
                    .documents
                    .iter()
                    .any(|doc| doc.repository == location.repository && doc.path == location.path)
            {
                return Err(ValidationError::new(
                    "Unknown or duplicate app document location",
                ));
            }
            if let Some(selection) = location.location.selection()
                && !context
                    .checkouts
                    .iter()
                    .any(|b| b.repository == location.repository && b.selection == *selection)
            {
                return Err(ValidationError::new(
                    "App location has no matching checkout binding",
                ));
            }
            match &location.location {
                ResolvedDocument::File { selection, file }
                    if !absolute(file)
                        || !file.starts_with(&selection.root)
                        || file == &selection.root =>
                {
                    return Err(ValidationError::new("App file must be inside its checkout"));
                }
                ResolvedDocument::Unavailable {
                    problem: DocumentProblem::Other(message),
                    ..
                } => {
                    check_text(message, "document error", false)?;
                }
                _ => {}
            }
        }
        let scope = Scope {
            catalog_source: context.catalog_source.clone(),
            host_id: session.host_id.clone(),
            stream_id: stream.id.clone(),
            session_id: session.id.clone(),
            harness: session.harness.clone(),
            native_session_id: session.harness_session_id.clone(),
        };
        Ok(Self {
            stream,
            session,
            scope,
            context,
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
    /// Reuse the painter's URL policy when validating an external action.
    pub fn contains_link(&self, url: &url::Url) -> bool {
        self.stream
            .issues
            .iter()
            .chain(&self.stream.prs)
            .any(|link| super::render::web_url(&link.url).as_ref() == Some(url))
    }
    pub fn owner(&self) -> &NativeOwnerObservation {
        &self.context.owner
    }
    pub fn document_target(&self, repository: &str, path: &str) -> Option<DocumentTarget> {
        self.stream
            .documents
            .iter()
            .find(|document| document.repository == repository && document.path == path)
            .map(|document| self.target(document))
    }

    pub(super) fn target(&self, document: &Document) -> DocumentTarget {
        let location = self
            .context
            .documents
            .iter()
            .find(|l| l.repository == document.repository && l.path == document.path)
            .expect("snapshot construction checks complete locations");
        DocumentTarget {
            scope: self.scope.clone(),
            owner: self.context.owner.clone(),
            document: document.clone(),
            repository_url: self.stream.repositories[&document.repository].url.clone(),
            location: location.location.clone(),
        }
    }
}

/// Exact painted reference and app location for the future native action guard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentTarget {
    pub scope: Scope,
    pub owner: NativeOwnerObservation,
    pub document: Document,
    pub repository_url: String,
    pub location: ResolvedDocument,
}

/// Editor reuse identity excludes progress and recovery ref/provenance. Native reuse must
/// additionally attest the viewing session and owned editor tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorIdentity {
    pub catalog_source: String,
    pub host_id: String,
    pub checkout_id: String,
    pub file: PathBuf,
}

impl DocumentTarget {
    /// Returns only the app-observed identity, not permission to open or reuse an editor.
    pub fn editor_identity(&self) -> Result<EditorIdentity, &DocumentProblem> {
        match &self.location {
            ResolvedDocument::File { selection, file } => Ok(EditorIdentity {
                catalog_source: self.scope.catalog_source.clone(),
                host_id: self.scope.host_id.clone(),
                checkout_id: selection.checkout_id.clone(),
                file: file.clone(),
            }),
            ResolvedDocument::Unavailable { problem, .. } => Err(problem),
        }
    }
}
