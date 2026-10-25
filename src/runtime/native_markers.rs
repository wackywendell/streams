//! Ordinary workspace selectors, not saved placement or caller credentials.

use std::collections::BTreeMap;

use crate::frontend::herdr::HerdrNativeSnapshot;

use super::{Result, digest, invalid, text};

pub fn stream_tokens(catalog_source: &str, host: &str, stream: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (
            "strm_catalog".into(),
            digest(&serde_json::to_vec(catalog_source).expect("string serialization")),
        ),
        ("strm_host".into(), host.into()),
        ("strm_stream".into(), stream.into()),
    ])
}

/// Publish selectors to an already-observed unmarked workspace; never confirm or retry.
pub fn associate_workspace(
    client: &crate::frontend::herdr::HerdrClient,
    workspace: &str,
    catalog_source: &str,
    host: &str,
    stream: &str,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<()> {
    if cancelled.load(std::sync::atomic::Ordering::Acquire) {
        return Err(invalid("native request cancelled"));
    }
    client.card_request(
        "workspace.report_metadata",
        serde_json::json!({"workspace_id":workspace,
        "source":"streams","tokens":stream_tokens(catalog_source, host, stream)}),
        Some("ok"),
        true,
    )?;
    Ok(())
}

pub fn selected_stream(
    snapshot: &HerdrNativeSnapshot,
    workspace: &str,
    catalog_source: &str,
    host: &str,
) -> Result<Option<String>> {
    let tokens = snapshot
        .workspace_tokens(workspace)
        .ok_or_else(|| invalid("calling workspace is absent from the snapshot"))?;
    if !["strm_catalog", "strm_host", "strm_stream"]
        .iter()
        .any(|key| tokens.contains_key(*key))
    {
        return Ok(None);
    }
    let stream = tokens
        .get("strm_stream")
        .ok_or_else(|| invalid("calling workspace Stream marker is incomplete"))?;
    text(stream)?;
    if stream_tokens(catalog_source, host, stream)
        .iter()
        .any(|(key, value)| tokens.get(key) != Some(value))
    {
        return Err(invalid(
            "calling workspace markers name another catalog or host, or are incomplete",
        ));
    }
    Ok(Some(stream.clone()))
}
