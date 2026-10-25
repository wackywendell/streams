//! Saved registry schema and selection policy, independent of filesystem liveness.

use super::{CheckoutLocation, canonical_saved_path};
use crate::codec::objects;
use crate::runtime::{Result, RuntimeError, invalid, text};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct State {
    pub(super) version: u64,
    pub(super) host_id: String,
    #[serde(deserialize_with = "objects")]
    pub(super) checkouts: Vec<CheckoutLocation>,
    pub(super) defaults: BTreeMap<String, String>,
}

impl State {
    pub(super) fn new(host_id: &str) -> Self {
        Self {
            version: 1,
            host_id: host_id.into(),
            checkouts: Vec::new(),
            defaults: BTreeMap::new(),
        }
    }

    pub(super) fn check(&self, host_id: &str) -> Result<()> {
        if self.version != 1 || self.host_id != host_id {
            return Err(invalid("unsupported registry schema or host"));
        }
        let (mut ids, mut roots) = (BTreeSet::new(), BTreeSet::new());
        for checkout in &self.checkouts {
            text(&checkout.id)?;
            canonical_saved_path(&checkout.directory)?;
            let mut urls = BTreeSet::new();
            for url in &checkout.repository_urls {
                text(url)?;
                if !urls.insert(url) {
                    return Err(invalid("duplicate repository binding"));
                }
            }
            if !ids.insert(&checkout.id) || !roots.insert(&checkout.directory) {
                return Err(invalid("duplicate checkout ID or directory path"));
            }
        }
        for (url, id) in &self.defaults {
            text(url)?;
            if !self
                .checkouts
                .iter()
                .any(|item| &item.id == id && item.repository_urls.contains(url))
            {
                return Err(invalid("default must name an explicitly bound checkout"));
            }
        }
        Ok(())
    }

    pub(super) fn lookup(&self, id: &str) -> Result<&CheckoutLocation> {
        text(id)?;
        self.checkouts
            .iter()
            .find(|item| item.id == id)
            .ok_or_else(|| RuntimeError::NotFound(format!("checkout is not registered: {id}")))
    }

    pub(super) fn resolve(
        &self,
        repository_url: &str,
        selected_id: Option<&str>,
        selected_repository_url: Option<&str>,
    ) -> Result<&CheckoutLocation> {
        text(repository_url)?;
        if let Some(url) = selected_repository_url {
            text(url)?;
        }
        if let Some(id) = selected_id {
            let selected = self.lookup(id)?;
            if selected_repository_url.is_none() && selected.repository_urls.is_empty() {
                return Err(invalid(
                    "selected checkout lacks an explicit repository URL binding",
                ));
            }
            let bound = selected
                .repository_urls
                .iter()
                .any(|url| url == repository_url);
            if selected_repository_url == Some(repository_url) || bound {
                if !bound {
                    return Err(invalid(
                        "selected checkout lacks an explicit repository URL binding",
                    ));
                }
                return Ok(selected);
            }
        } else if selected_repository_url == Some(repository_url) {
            return Err(invalid(
                "selected repository requires a selected checkout ID",
            ));
        }
        if let Some(id) = self.defaults.get(repository_url) {
            return self.lookup(id);
        }
        let candidates: Vec<_> = self
            .checkouts
            .iter()
            .filter(|item| item.repository_urls.iter().any(|url| url == repository_url))
            .map(|item| item.id.clone())
            .collect();
        let message = format!("no explicit checkout default for repository: {repository_url}");
        if candidates.len() > 1 {
            Err(RuntimeError::Ambiguous {
                message,
                candidates,
            })
        } else {
            Err(RuntimeError::NotFound(message))
        }
    }

    pub(super) fn set_default(&mut self, repository_url: &str, id: &str) -> Result<()> {
        text(repository_url)?;
        let checkout = self.lookup(id)?;
        if !checkout
            .repository_urls
            .iter()
            .any(|url| url == repository_url)
        {
            return Err(invalid(
                "default requires an explicit repository URL binding",
            ));
        }
        self.defaults.insert(repository_url.into(), id.into());
        Ok(())
    }
}

#[cfg(test)]
mod tests;
