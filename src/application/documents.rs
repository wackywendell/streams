//! Local current-file hints are not native effect authority or historical revisions.

use crate::{
    model::{Session, Stream},
    runtime::{CheckoutRegistry, DirectoryBindings},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub fn context(
    registry: &CheckoutRegistry,
    locations: &DirectoryBindings,
    stream: &Stream,
    session: &Session,
) -> Value {
    if stream.repositories.is_empty() {
        return json!({"checkouts":[],"document_locations":[]});
    }
    let mut snapshot = registry.snapshot().map_err(|error| error.to_string());
    let selected = snapshot
        .as_mut()
        .map_err(|error| error.clone())
        .and_then(|checkouts| selected_directory(locations, stream, session, checkouts));
    let selected_id = selected
        .as_ref()
        .ok()
        .and_then(|binding| binding.checkout.as_ref())
        .map(|checkout| checkout.id.as_str());
    let repository_url = preferred_repository_url(session, selected.as_ref().ok());
    let mut checkouts = Vec::new();
    // Keep one checkout snapshot per repository for these read-only hints, not action authority.
    let mut repositories = BTreeMap::new();
    for (alias, repository) in &stream.repositories {
        let resolved = match &selected {
            Err(error) => Err(error.clone()),
            Ok(_) => snapshot
                .as_mut()
                .expect("selected directory admitted the registry")
                .resolve(&repository.url, selected_id, repository_url)
                .map_err(|error| error.to_string()),
        };
        if let Ok(checkout) = &resolved {
            checkouts.push(
                json!({"repository":alias,"checkout_id":checkout.id,"root":checkout.directory}),
            );
        }
        repositories.insert(alias.as_str(), resolved);
    }
    let document_locations:Vec<_>=stream.documents.iter().map(|document|{
        let resolved=repositories.get(document.repository.as_str()).expect("validated document repository");
        let (checkout,error)=match resolved {Ok(checkout)=>(Some(checkout),None),Err(error)=>(None,Some(error))};
        let mut location=json!({"repository":document.repository,"path":document.path,
            "checkout_id":checkout.map(|checkout|&checkout.id),"root":checkout.map(|checkout|&checkout.directory),
            "kind":null,"file":null,"error":error});
        if let Some(checkout)=checkout {
            let current=checkout.document(&document.path).map_err(|error|error.to_string());
            match current {
                Ok(file)=>{
                    location["file"]=json!(file);
                    location["kind"]=json!("local");
                },
                Err(error)=>location["error"]=json!(format!("Document not present in this checkout: {}. {error}",document.path)),
            }
        }
        location
    }).collect();
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
    if !stream
        .documents
        .iter()
        .any(|document| document.repository == key.repository && document.path == key.path)
    {
        return Err(RuntimeError::Invalid(
            "saved document reference is unavailable".into(),
        ));
    }
    let mut checkouts = registry.snapshot()?;
    let selected = selected_directory(locations, stream, session, &mut checkouts)
        .map_err(RuntimeError::Invalid)?;
    let selected_id = selected
        .checkout
        .as_ref()
        .map(|checkout| checkout.id.as_str());
    let repository_url = preferred_repository_url(session, Some(&selected));
    let repository = stream
        .repositories
        .get(&key.repository)
        .expect("validated document repository");
    let checkout = checkouts.resolve(&repository.url, selected_id, repository_url)?;
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
    use crate::model::{Document, Repository};
    use crate::runtime::{Result, checkouts::RepositoryInspection};
    use std::path::Path;

    fn inspect(path: &Path) -> Result<Option<RepositoryInspection>> {
        Ok(Some(RepositoryInspection {
            root: path.to_owned(),
            relative_cwd: ".".into(),
        }))
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
            .documents
            .push(Document::new("repo", "main", "guide.md", "Guide", None).unwrap());
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
            json!([{"repository":"repo","checkout_id":id,"root":checkout}])
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
                repository: "repo".into(),
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
            .documents
            .push(Document::new("repo", "main", "guide.md", "Guide", None).unwrap());
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
                repository: "repo".into(),
                path: "guide.md".into(),
            },
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("not registered"));
    }
}
