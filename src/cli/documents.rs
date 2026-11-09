//! Local current-file hints are not native effect authority or historical revisions.

use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use streams::{
    model::{ReferenceState, ReferenceTarget, Session, Stream},
    runtime::{CheckoutRegistry, DirectoryBindings},
};

pub(super) fn context(
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
    let document_locations: Vec<_> = documents.into_iter().map(|document| {
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
                Err(error) => location["error"] = json!(format!("Document not present in this checkout: {}. {error}", document.path)),
            }
        }
        location
    }).collect();
    json!({"checkouts":checkouts,"document_locations":document_locations})
}
