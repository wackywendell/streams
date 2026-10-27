//! Local current-file hints are not native effect authority or historical revisions.

use serde_json::{Value, json};
use std::collections::BTreeMap;
use streams_temper::{
    model::{Session, Stream},
    runtime::{CheckoutRegistry, DirectoryBindings},
};

pub(super) fn context(
    registry: &CheckoutRegistry,
    locations: &DirectoryBindings,
    stream: &Stream,
    session: &Session,
) -> Value {
    let selected = (|| -> Result<_, String> {
        let selected = locations
            .read(&stream.id, Some(&session.id))
            .map_err(|error| error.to_string())?
            .ok_or("no directory is selected for this Session; use streams session directory")?;
        if let Some(checkout) = &selected.checkout {
            let primary = registry
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
            return Err(
                "the Session checkout is not registered; use streams checkout register".into(),
            );
        }
        Ok(selected)
    })();
    let selected_id = selected
        .as_ref()
        .ok()
        .and_then(|binding| binding.checkout.as_ref())
        .map(|checkout| checkout.id.as_str());
    let repository_url = session
        .checkout
        .as_ref()
        .and_then(|portable| portable.repository_url.as_deref())
        .or_else(|| {
            selected
                .as_ref()
                .ok()
                .and_then(|binding| binding.checkout.as_ref())
                .and_then(|checkout| checkout.repository_url.as_deref())
        });
    let mut checkouts = Vec::new();
    // Keep one checkout snapshot per repository for these read-only hints, not action authority.
    let mut repositories = BTreeMap::new();
    for (alias, repository) in &stream.repositories {
        let resolved = match &selected {
            Err(error) => Err(error.clone()),
            Ok(_) => registry
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
