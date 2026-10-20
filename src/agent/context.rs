//! Actual local capture; no projected reads, publication or inferred attachment.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    model::{Session, Stream, ValidationError, check_text},
    operations::Authority,
    runtime::conversations::Harness,
    sync::{CatalogBackend, state::RepairRequired},
};

#[derive(Debug, thiserror::Error)]
pub enum ContextError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error(transparent)]
    Repair(#[from] RepairRequired),
}

/// Explicit caller identities. A workspace association is not enrollment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextRequest<'a> {
    pub harness: Harness,
    pub native_id: &'a str,
    pub stream_id: Option<&'a str>,
    pub environment_stream_id: Option<&'a str>,
    pub workspace_stream_id: Option<&'a str>,
}

/// Captured local records and authority for the existing subprocess contract.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AgentContext {
    pub catalog_source: String,
    pub host_id: String,
    pub stream: Option<Stream>,
    pub session: Option<Session>,
    pub sessions: Vec<Session>,
    pub authority: Option<Authority>,
}

/// Stable catalog identity for one host/harness/native conversation, matching Python.
pub fn session_id_for(host_id: &str, harness: Harness, native_id: &str) -> String {
    let identity = format!(
        "streams:session:[{}, {}, {}]",
        crate::codec::ascii_json(host_id),
        crate::codec::ascii_json(harness.as_ref()),
        crate::codec::ascii_json(native_id),
    );
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, identity.as_bytes()).to_string()
}

/// Resolve only exact native/catalog identities. Conflicting sources fail closed.
/// The caller supplies the physical source fingerprint; live Pi lineage remains
/// an in-process extension guard, not something this snapshot can establish.
pub fn context(
    backend: &dyn CatalogBackend,
    host_id: &str,
    catalog_source: &str,
    request: ContextRequest<'_>,
) -> Result<AgentContext, ContextError> {
    check_text(host_id, "host_id", false)?;
    check_text(catalog_source, "catalog_source", false)?;
    check_text(request.native_id, "harness_session_id", false)?;
    let catalog = backend.snapshot()?.catalog;
    let session = catalog.sessions().iter().find(|session| {
        session.host_id == host_id
            && session.harness == request.harness.as_ref()
            && session.harness_session_id == request.native_id
    });
    let mut identities = BTreeSet::new();
    for id in [
        request.stream_id,
        request.environment_stream_id,
        request.workspace_stream_id,
        session.map(|session| session.stream_id.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        check_text(id, "stream_id", false)?;
        identities.insert(id);
    }
    if identities.len() > 1 {
        return Err(ValidationError::new(
            "conversation already has a different effort; moving it requires an explicit move or fork operation",
        ).into());
    }
    let stream = identities.first().map(|id| {
        catalog.stream(id).cloned().ok_or_else(|| ValidationError::new(format!(
            "stream {id:?} is not in the local catalog; run streams sync to adopt remote-only records",
        )))
    }).transpose()?;
    let authority = stream
        .as_ref()
        .map(|stream| backend.authority(&stream.id))
        .transpose()?
        .flatten();
    let sessions = catalog
        .sessions()
        .iter()
        .filter(|session| {
            stream
                .as_ref()
                .is_some_and(|stream| stream.id == session.stream_id)
        })
        .cloned()
        .collect();
    Ok(AgentContext {
        catalog_source: catalog_source.into(),
        host_id: host_id.into(),
        stream,
        session: session.cloned(),
        sessions,
        authority,
    })
}
