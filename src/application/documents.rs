//! Local current-file hints are not native effect authority or historical revisions.

use crate::{
    model::{ReferenceState, ReferenceTarget, Session, Stream},
    runtime::{CheckoutRegistry, DirectoryBindings},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub fn context(
    registry: &CheckoutRegistry,
    locations: &DirectoryBindings,
    stream: &Stream,
    session: &Session,
) -> Value {
    let documents: Vec<_> = stream
        .references
        .iter()
        .filter(|reference| reference.state != ReferenceState::Dropped)
        .filter_map(|reference| match &reference.target {
            ReferenceTarget::Document(document) => Some(document),
            _ => None,
        })
        .collect();
    let mut urls: BTreeSet<String> = documents.iter().map(|d| d.repository_url.clone()).collect();
    if let Some(url) = session
        .checkout
        .as_ref()
        .and_then(|checkout| checkout.repository_url.as_ref())
    {
        urls.insert(url.clone());
    }
    let mut snapshot = registry.snapshot().map_err(|error| error.to_string());
    let selected = (|| -> Result<_, String> {
        let checkouts = snapshot.as_mut().map_err(|error| error.clone())?;
        let selected = locations
            .read_with_checkouts(&stream.id, Some(&session.id), checkouts)
            .map_err(|error| error.to_string())?;
        if let Some(selected) = &selected {
            if let Some(checkout) = &selected.checkout {
                let primary = checkouts
                    .get(&checkout.id)
                    .map_err(|error| error.to_string())?;
                urls.extend(primary.repository_urls.iter().cloned());
                if primary.repository_urls.is_empty() {
                    return Err("the Session checkout has no repository binding; register its repository URL explicitly".into());
                }
            } else if selected.cwd.ancestors().any(|directory| {
                [".git", ".jj"].iter().any(|name| {
                    let marker = directory.join(name);
                    marker.exists() || marker.is_symlink()
                })
            }) {
                return Err(
                    "the Session checkout is not registered; use streams checkout register".into(),
                );
            }
        }
        Ok(selected)
    })();
    let binding = selected
        .as_ref()
        .ok()
        .and_then(Option::as_ref)
        .and_then(|binding| binding.checkout.as_ref());
    let selected_id = binding.map(|checkout| checkout.id.as_str());
    // A portable URL without a selected checkout must not prevent explicit defaults.
    let repository_url = selected_id.and_then(|_| {
        session
            .checkout
            .as_ref()
            .and_then(|portable| portable.repository_url.as_deref())
            .or_else(|| binding.and_then(|checkout| checkout.repository_url.as_deref()))
    });
    let mut checkouts = Vec::new();
    let mut repositories = BTreeMap::new();
    for url in urls {
        let resolved = match &selected {
            Err(error) => Err(error.clone()),
            Ok(_) => snapshot
                .as_mut()
                .expect("selection admitted the registry")
                .resolve(&url, selected_id, repository_url)
                .map_err(|error| error.to_string()),
        };
        if let Ok(checkout) = &resolved {
            checkouts.push(
                json!({"repository_url":url,"checkout_id":checkout.id,"root":checkout.directory}),
            );
        }
        repositories.insert(url, resolved);
    }
    let document_locations: Vec<_> = documents
        .into_iter()
        .map(|document| {
            let resolved = &repositories[&document.repository_url];
            let (checkout, error) = match resolved {
                Ok(checkout) => (Some(checkout), None),
                Err(error) => (None, Some(error)),
            };
            let mut location = json!({"repository_url":document.repository_url,"path":document.path,
                "checkout_id":checkout.map(|checkout|&checkout.id),"root":checkout.map(|checkout|&checkout.directory),
                "kind":null,"file":null,"error":error});
            if let Some(checkout) = checkout {
                match checkout.document(&document.path) {
                    Ok(file) => {
                        location["file"] = json!(file);
                        location["kind"] = json!("local");
                    }
                    Err(error) => {
                        location["error"] = json!(format!(
                            "Document not present in this checkout: {}. {error}",
                            document.path
                        ))
                    }
                }
            }
            location
        })
        .collect();
    json!({"checkouts":checkouts,"document_locations":document_locations})
}

fn selected_directory(
    locations: &DirectoryBindings,
    stream: &Stream,
    session: &Session,
    checkouts: &mut crate::runtime::checkouts::CheckoutSnapshot<'_>,
) -> Result<crate::runtime::DirectoryBinding, String> {
    let selected = locations
        .read_with_checkouts(&stream.id, Some(&session.id), checkouts)
        .map_err(|error| error.to_string())?
        .ok_or("no directory is selected for this Session; use streams session directory")?;
    if let Some(checkout) = &selected.checkout {
        let primary = checkouts
            .get(&checkout.id)
            .map_err(|error| error.to_string())?;
        if primary.repository_urls.is_empty() {
            return Err("the Session checkout has no repository binding; register its repository URL explicitly".into());
        }
    } else if selected.cwd.ancestors().any(|directory| {
        [".git", ".jj"].iter().any(|name| {
            let marker = directory.join(name);
            marker.exists() || marker.is_symlink()
        })
    }) {
        return Err("the Session checkout is not registered; use streams checkout register".into());
    }
    Ok(selected)
}

fn preferred_repository_url<'a>(
    session: &'a Session,
    selected: Option<&'a crate::runtime::DirectoryBinding>,
) -> Option<&'a str> {
    session
        .checkout
        .as_ref()
        .and_then(|portable| portable.repository_url.as_deref())
        .or_else(|| {
            selected
                .and_then(|binding| binding.checkout.as_ref())
                .and_then(|checkout| checkout.repository_url.as_deref())
        })
}

/// Resolve one freshly selected saved document, using the same checkout policy as card hints.
pub fn resolve(
    registry: &CheckoutRegistry,
    locations: &DirectoryBindings,
    stream: &Stream,
    session: &Session,
    key: &crate::integration::DocumentKey,
) -> crate::runtime::Result<WorkingDocument> {
    use crate::runtime::RuntimeError;
    let repository_url = if stream.references.iter().any(|reference| {
        reference.state != ReferenceState::Dropped
            && matches!(
                &reference.target,
                ReferenceTarget::Document(doc)
                    if doc.repository_url == key.repository && doc.path == key.path
            )
    }) {
        key.repository.as_str()
    } else if let Some(repo) = stream.repositories.get(&key.repository)
        && stream.references.iter().any(|reference| {
            reference.state != ReferenceState::Dropped
                && matches!(
                    &reference.target,
                    ReferenceTarget::Document(doc)
                        if doc.repository_url == repo.url && doc.path == key.path
                )
        })
    {
        repo.url.as_str()
    } else {
        return Err(RuntimeError::Invalid(
            "saved document reference is unavailable".into(),
        ));
    };
    let mut checkouts = registry.snapshot()?;
    let selected = selected_directory(locations, stream, session, &mut checkouts)
        .map_err(RuntimeError::Invalid)?;
    let selected_id = selected
        .checkout
        .as_ref()
        .map(|checkout| checkout.id.as_str());
    let preferred_url = preferred_repository_url(session, Some(&selected));
    let checkout = checkouts.resolve(repository_url, selected_id, preferred_url)?;
    let file = checkout.document(&key.path)?;
    Ok(WorkingDocument { checkout, file })
}

pub struct WorkingDocument {
    pub checkout: crate::runtime::checkouts::CheckoutLocation,
    pub file: std::path::PathBuf,
}

/// Recheck an observed checkout and file before a native effect.
pub fn revalidate(
    registry: &CheckoutRegistry,
    checkout_id: &str,
    root: &std::path::Path,
    repository_url: &str,
    path: &str,
    expected_file: &std::path::Path,
) -> crate::runtime::Result<WorkingDocument> {
    use crate::runtime::RuntimeError;
    let checkout = registry.get(checkout_id)?;
    if checkout.directory != root
        || !checkout
            .repository_urls
            .iter()
            .any(|url| url == repository_url)
        || std::fs::canonicalize(root)? != root
    {
        return Err(RuntimeError::Invalid(
            "selected checkout root or repository binding changed".into(),
        ));
    }
    let file = checkout.document(path)?;
    if file != expected_file
        || !std::fs::metadata(&file)?.is_file()
        || std::fs::canonicalize(&file)? != file
        || std::fs::canonicalize(root)? != root
    {
        return Err(RuntimeError::Invalid(
            "app-resolved document or checkout changed during resolution".into(),
        ));
    }
    Ok(WorkingDocument { checkout, file })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Reference, ReferenceRole, ReferenceState, ReferenceTarget, ReferenceValue, Repository,
        RepositoryDocument,
    };
    use crate::runtime::{Result, checkouts::RepositoryInspection};
    use std::path::Path;

    fn inspect(path: &Path) -> Result<Option<RepositoryInspection>> {
        Ok(Some(RepositoryInspection {
            root: path.to_owned(),
            relative_cwd: ".".into(),
        }))
    }

    fn doc_ref(repo_url: &str, path: &str, name: &str) -> Reference {
        Reference::new(ReferenceValue {
            name: name.into(),
            purpose: "Guide".into(),
            role: ReferenceRole::Brief,
            state: ReferenceState::Active,
            target: ReferenceTarget::Document(RepositoryDocument {
                repository_url: repo_url.into(),
                path: path.into(),
                r#ref: Some("main".into()),
                commit: None,
                source_session_id: None,
            }),
        })
        .unwrap()
    }

    #[test]
    fn hints_and_action_revalidation_preserve_selected_checkout_and_repository() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let checkout = root.join("checkout");
        std::fs::create_dir(&checkout).unwrap();
        let file = checkout.join("guide.md");
        std::fs::write(&file, "Saved document").unwrap();
        let registry = CheckoutRegistry::new(root.join("state"), "host")
            .unwrap()
            .with_inspector(inspect);
        let locations = DirectoryBindings::new(root.join("bindings"), registry.clone());
        let mut stream = Stream::new("stream", "Stream", None, "host").unwrap();
        stream.repositories.insert(
            "repo".into(),
            Repository::new("https://example.test/repo").unwrap(),
        );
        stream
            .references
            .push(doc_ref("https://example.test/repo", "guide.md", "Guide"));
        let session = Session::new("session", "host", "stream", "pi", "native", "Session").unwrap();
        let selected = locations
            .remember(
                "stream",
                &checkout,
                Some("session"),
                false,
                Some("https://example.test/repo"),
                None,
            )
            .unwrap();
        let id = selected.checkout.unwrap().id;
        let hints = context(&registry, &locations, &stream, &session);
        assert_eq!(
            hints["checkouts"],
            json!([{"repository_url":"https://example.test/repo","checkout_id":id,"root":checkout}])
        );
        assert_eq!(hints["document_locations"][0]["file"], json!(file));
        assert_eq!(hints["document_locations"][0]["error"], Value::Null);
        let resolved = revalidate(
            &registry,
            &id,
            &checkout,
            "https://example.test/repo",
            "guide.md",
            &file,
        )
        .unwrap();
        assert_eq!(resolved.checkout.id, id);
        assert_eq!(resolved.file, file);
        let fresh = resolve(
            &registry,
            &locations,
            &stream,
            &session,
            &crate::integration::DocumentKey {
                repository: "https://example.test/repo".into(),
                path: "guide.md".into(),
            },
        )
        .unwrap();
        assert_eq!(fresh.file, file);
        assert_eq!(fresh.checkout.id, id);
        assert!(
            revalidate(
                &registry,
                &id,
                &checkout,
                "https://example.test/other",
                "guide.md",
                &file
            )
            .is_err()
        );
        assert!(
            revalidate(
                &registry,
                &id,
                &checkout,
                "https://example.test/repo",
                "guide.md",
                &root.join("other")
            )
            .is_err()
        );
    }

    #[test]
    fn unregistered_vcs_ancestor_remains_unavailable_without_creating_registry_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let registry = CheckoutRegistry::new(root.join("state"), "host").unwrap();
        let locations = DirectoryBindings::new(root.join("bindings"), registry.clone());
        locations
            .remember("stream", &root, Some("session"), false, None, None)
            .unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        let mut stream = Stream::new("stream", "Stream", None, "host").unwrap();
        stream.repositories.insert(
            "repo".into(),
            Repository::new("https://example.test/repo").unwrap(),
        );
        stream
            .references
            .push(doc_ref("https://example.test/repo", "guide.md", "Guide"));
        let session = Session::new("session", "host", "stream", "pi", "native", "Session").unwrap();
        let hints = context(&registry, &locations, &stream, &session);
        assert_eq!(hints["checkouts"], json!([]));
        assert!(
            hints["document_locations"][0]["error"]
                .as_str()
                .unwrap()
                .contains("not registered")
        );
        assert!(!registry.path().exists());
        let error = resolve(
            &registry,
            &locations,
            &stream,
            &session,
            &crate::integration::DocumentKey {
                repository: "https://example.test/repo".into(),
                path: "guide.md".into(),
            },
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("not registered"));
    }
}
