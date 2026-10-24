//! Explicit offline cutover. Normal operations never call legacy inspection or discovery.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    model::{Catalog, Record, Session, ValidationError, check_text},
    sync::Inspection,
};

/// Only the configured producer's records survive; foreign owners repopulate their replicas.
/// The ID map is transient cutover input for local references, not a runtime alias service.
#[derive(Debug)]
pub struct PreparedCatalog {
    host: String,
    records: Vec<Record>,
    session_ids: BTreeMap<String, String>,
    dropped_foreign_records: usize,
}

impl PreparedCatalog {
    pub fn records(&self) -> &[Record] {
        &self.records
    }
    pub fn session_id(&self, host: &str, old_id: &str) -> Option<&str> {
        if host != self.host {
            return None;
        }
        self.session_ids.get(old_id).map(String::as_str)
    }
    pub fn dropped_foreign_records(&self) -> usize {
        self.dropped_foreign_records
    }
}

/// Interpret old authority only here, before discarding its envelopes and history.
/// A pending or inconsistent ownership claim cannot silently become fixed-host permission.
pub fn prepare(inspection: &Inspection, host: &str) -> Result<PreparedCatalog, ValidationError> {
    check_text(host, "cutover producer", false)?;
    let old = &inspection.snapshot.catalog;
    let mut streams = Vec::new();
    for stream in old.streams() {
        let stored_local = stream.owner_host_id == host;
        let effective_local = inspection
            .authorities
            .get(&stream.id)
            .is_some_and(|authority| authority.host_id == host);
        if stored_local != effective_local {
            return Err(ValidationError::new(format!(
                "ambiguous ownership for Stream {:?}; explicitly recreate it before cutover",
                stream.id
            )));
        }
        if stored_local {
            streams.push(stream.clone());
        }
    }
    let owned: BTreeSet<_> = streams.iter().map(|stream| stream.id.as_str()).collect();
    let mut sessions = Vec::new();
    let mut session_ids = BTreeMap::new();
    for old_session in old
        .sessions()
        .iter()
        .filter(|session| session.host_id == host)
    {
        if !owned.contains(old_session.stream_id.as_str()) {
            return Err(ValidationError::new(format!(
                "Session {:?} participates in another host's Stream; explicitly recreate its local effort before cutover",
                old_session.id
            )));
        }
        let mut session = old_session.clone();
        session.id = Session::id_for(host, &session.harness, &session.harness_session_id);
        if let Some(checkout) = &mut session.checkout {
            let source = old.stream(&session.stream_id).expect("owned Stream exists");
            let url = checkout
                .repository_url(source)
                .expect("legacy Catalog validates checkout repository")
                .to_owned();
            checkout.repository_url = Some(url);
        }
        session.validate_portable()?;
        session_ids.insert(old_session.id.clone(), session.id.clone());
        sessions.push(session);
    }
    for stream in &mut streams {
        for document in &mut stream.documents {
            if let Some(old_id) = &document.source_session_id {
                document.source_session_id = session_ids.get(old_id).cloned();
            }
        }
    }
    let hosts = old
        .hosts()
        .iter()
        .filter(|record| record.id == host)
        .cloned()
        .collect();
    let catalog = Catalog::new(streams, sessions, hosts)?;
    let records = catalog.records();
    Ok(PreparedCatalog {
        host: host.into(),
        dropped_foreign_records: old.streams().len() + old.sessions().len() + old.hosts().len()
            - records.len(),
        records,
        session_ids,
    })
}

#[cfg(test)]
mod tests;
