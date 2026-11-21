//! Exact and partial key selection for Streams, Sessions and Artifacts.
//!
//! No selector is ever inferred from environment variables, hc labels or workspace markers.
//! Full keys perform a single-file point read with zero directory listings.

use streams::{
    application::OpenedRecords,
    artifacts::{ArtifactKey, validate_name as check_artifact_name},
    model::{Record, Session, Stream, check_token},
    storage::records::{RecordAddress, RecordStore},
};

use super::{CliError, Result};

fn check_partial_token(part: &str, label: &str) -> Result<()> {
    if part.is_empty()
        || part.contains('/')
        || part.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(CliError::InvalidRef(format!("invalid {label} {part:?}")));
    }
    Ok(())
}

/// Resolve a Stream selector (`HOST/ID`, `ID`, or unique ID prefix).
pub(super) fn select_stream(opened: &OpenedRecords, selector: &str) -> Result<Stream> {
    if selector.is_empty() {
        return Err(CliError::InvalidRef(
            "Stream selector cannot be empty".into(),
        ));
    }
    let parts: Vec<&str> = selector.split('/').collect();
    match parts.as_slice() {
        [host, id] => {
            check_token(host, "Stream host").map_err(|e| CliError::InvalidRef(e.to_string()))?;
            check_token(id, "Stream ID").map_err(|e| CliError::InvalidRef(e.to_string()))?;
            let found = opened
                .records()
                .with_reader(|reader| reader.read_stream(host, id))?;
            found.ok_or_else(|| CliError::NotFound(format!("no Stream {host}/{id}")))
        }
        [id_or_prefix] => {
            check_partial_token(id_or_prefix, "Stream selector")?;
            let listing = opened.listing()?;
            let streams: Vec<Stream> = listing
                .records
                .into_iter()
                .filter_map(|r| match r {
                    Record::Stream(s) => Some(s),
                    _ => None,
                })
                .collect();
            let exact: Vec<Stream> = streams
                .iter()
                .filter(|s| s.id == *id_or_prefix)
                .cloned()
                .collect();
            if let Some(stream) = finish_unique(
                exact,
                |s| s.key().to_string(),
                &format!("ambiguous Stream {selector:?}"),
            )? {
                return Ok(stream);
            }
            let prefix: Vec<Stream> = streams
                .into_iter()
                .filter(|s| s.id.starts_with(id_or_prefix))
                .collect();
            if let Some(stream) = finish_unique(
                prefix,
                |s| s.key().to_string(),
                &format!("ambiguous Stream prefix {selector:?}"),
            )? {
                return Ok(stream);
            }
            Err(CliError::NotFound(format!("no Stream {selector}")))
        }
        _ => Err(CliError::InvalidRef(format!(
            "invalid Stream selector {selector:?}; expected HOST/ID, ID, or a unique ID prefix"
        ))),
    }
}

/// Resolve a Session selector (`HOST/HARNESS/ID`, `HARNESS/ID`, `ID`, or unique ID prefix).
pub(super) fn select_session(opened: &OpenedRecords, selector: &str) -> Result<Session> {
    if selector.is_empty() {
        return Err(CliError::InvalidRef(
            "Session selector cannot be empty".into(),
        ));
    }
    let not_found = || {
        CliError::NotFound(format!(
            "no Session {selector}; run `streams session create` to enroll a conversation"
        ))
    };
    let parts: Vec<&str> = selector.split('/').collect();
    match parts.as_slice() {
        [host, harness, id] => {
            check_token(host, "Session host").map_err(|e| CliError::InvalidRef(e.to_string()))?;
            check_token(harness, "Session harness")
                .map_err(|e| CliError::InvalidRef(e.to_string()))?;
            check_token(id, "Session ID").map_err(|e| CliError::InvalidRef(e.to_string()))?;
            let found = opened
                .records()
                .with_reader(|reader| reader.read_session(host, harness, id))?;
            found.ok_or_else(not_found)
        }
        [harness, id] => {
            check_token(harness, "Session harness")
                .map_err(|e| CliError::InvalidRef(e.to_string()))?;
            check_token(id, "Session ID").map_err(|e| CliError::InvalidRef(e.to_string()))?;
            let listing = opened.listing()?;
            let exact: Vec<Session> = listing
                .records
                .into_iter()
                .filter_map(|r| match r {
                    Record::Session(s) if s.harness == *harness && s.id == *id => Some(s),
                    _ => None,
                })
                .collect();
            if let Some(session) = finish_unique(
                exact,
                |s| s.key().to_string(),
                &format!("ambiguous Session {selector:?}"),
            )? {
                return Ok(session);
            }
            Err(not_found())
        }
        [id_or_prefix] => {
            check_partial_token(id_or_prefix, "Session selector")?;
            let listing = opened.listing()?;
            let sessions: Vec<Session> = listing
                .records
                .into_iter()
                .filter_map(|r| match r {
                    Record::Session(s) => Some(s),
                    _ => None,
                })
                .collect();
            let exact: Vec<Session> = sessions
                .iter()
                .filter(|s| s.id == *id_or_prefix)
                .cloned()
                .collect();
            if let Some(session) = finish_unique(
                exact,
                |s| s.key().to_string(),
                &format!("ambiguous Session {selector:?}"),
            )? {
                return Ok(session);
            }
            let prefix: Vec<Session> = sessions
                .into_iter()
                .filter(|s| s.id.starts_with(id_or_prefix))
                .collect();
            if let Some(session) = finish_unique(
                prefix,
                |s| s.key().to_string(),
                &format!("ambiguous Session prefix {selector:?}"),
            )? {
                return Ok(session);
            }
            Err(not_found())
        }
        _ => Err(CliError::InvalidRef(format!(
            "invalid Session selector {selector:?}; expected HOST/HARNESS/ID, HARNESS/ID, ID, or a unique ID prefix"
        ))),
    }
}

/// Parse an exact `HOST/HARNESS/ID` key (for `session list --key`).
pub(super) fn parse_session_full_key(raw: &str) -> Result<RecordAddress> {
    let parts: Vec<&str> = raw.split('/').collect();
    let [host, harness, id] = parts.as_slice() else {
        return Err(CliError::InvalidRef(format!(
            "invalid Session key {raw:?}; expected HOST/HARNESS/ID"
        )));
    };
    RecordAddress::session(*host, *harness, *id).map_err(|e| CliError::InvalidRef(e.to_string()))
}

/// Parse an exact `HARNESS/ID` pair (for `session create` and `card` commands).
/// Refuses a bare ID, prefix, or full `HOST/HARNESS/ID` key.
pub(super) fn parse_exact_harness_id(raw: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = raw.split('/').collect();
    let [harness, id] = parts.as_slice() else {
        return Err(CliError::InvalidRef(format!(
            "expected exact HARNESS/ID, got {raw:?}"
        )));
    };
    check_token(harness, "Session harness").map_err(|e| CliError::InvalidRef(e.to_string()))?;
    check_token(id, "Session ID").map_err(|e| CliError::InvalidRef(e.to_string()))?;
    Ok(((*harness).to_owned(), (*id).to_owned()))
}

/// Resolve an Artifact selector (`HOST/HARNESS/ID/NAME`, or leading parts omitted).
pub(super) fn select_artifact(opened: &OpenedRecords, selector: &str) -> Result<ArtifactKey> {
    if selector.is_empty() {
        return Err(CliError::InvalidRef(
            "Artifact selector cannot be empty".into(),
        ));
    }
    let parts: Vec<&str> = selector.split('/').collect();
    match parts.as_slice() {
        [host, harness, session, name] => {
            let key = ArtifactKey {
                host: (*host).to_owned(),
                harness: (*harness).to_owned(),
                session: (*session).to_owned(),
                name: (*name).to_owned(),
            };
            key.validate()
                .map_err(|e| CliError::InvalidRef(e.to_string()))?;
            Ok(key)
        }
        [harness, session, name] => {
            check_token(harness, "Session harness")
                .map_err(|e| CliError::InvalidRef(e.to_string()))?;
            check_token(session, "Session ID").map_err(|e| CliError::InvalidRef(e.to_string()))?;
            check_artifact_name(name).map_err(|e| CliError::InvalidRef(e.to_string()))?;
            match_artifact_in_listing(
                opened,
                selector,
                |s| s.harness == *harness && s.id == *session && s.artifacts.contains_key(*name),
                name,
            )
        }
        [session_or_prefix, name] => {
            check_partial_token(session_or_prefix, "Session selector")?;
            check_artifact_name(name).map_err(|e| CliError::InvalidRef(e.to_string()))?;
            let listing = opened.listing()?;
            let exact: Vec<ArtifactKey> = listing
                .records
                .iter()
                .filter_map(|r| match r {
                    Record::Session(s)
                        if s.id == *session_or_prefix && s.artifacts.contains_key(*name) =>
                    {
                        Some(ArtifactKey {
                            host: s.host_id.clone(),
                            harness: s.harness.clone(),
                            session: s.id.clone(),
                            name: (*name).to_owned(),
                        })
                    }
                    _ => None,
                })
                .collect();
            if let Some(key) = finish_unique(
                exact,
                ArtifactKey::to_string,
                &format!("ambiguous Artifact {selector:?}"),
            )? {
                return Ok(key);
            }
            let prefix: Vec<ArtifactKey> = listing
                .records
                .into_iter()
                .filter_map(|r| match r {
                    Record::Session(s)
                        if s.id.starts_with(session_or_prefix)
                            && s.artifacts.contains_key(*name) =>
                    {
                        Some(ArtifactKey {
                            host: s.host_id,
                            harness: s.harness,
                            session: s.id,
                            name: (*name).to_owned(),
                        })
                    }
                    _ => None,
                })
                .collect();
            if let Some(key) = finish_unique(
                prefix,
                ArtifactKey::to_string,
                &format!("ambiguous Artifact {selector:?}"),
            )? {
                return Ok(key);
            }
            Err(CliError::NotFound(format!("no Artifact {selector}")))
        }
        [name] => {
            check_artifact_name(name).map_err(|e| CliError::InvalidRef(e.to_string()))?;
            match_artifact_in_listing(opened, selector, |s| s.artifacts.contains_key(*name), name)
        }
        _ => Err(CliError::InvalidRef(format!(
            "invalid Artifact selector {selector:?}; expected HOST/HARNESS/ID/NAME or a shorter suffix"
        ))),
    }
}

fn match_artifact_in_listing(
    opened: &OpenedRecords,
    selector: &str,
    predicate: impl Fn(&Session) -> bool,
    name: &str,
) -> Result<ArtifactKey> {
    let listing = opened.listing()?;
    let matches: Vec<ArtifactKey> = listing
        .records
        .into_iter()
        .filter_map(|r| match r {
            Record::Session(s) if predicate(&s) => Some(ArtifactKey {
                host: s.host_id,
                harness: s.harness,
                session: s.id,
                name: name.to_owned(),
            }),
            _ => None,
        })
        .collect();
    if let Some(key) = finish_unique(
        matches,
        ArtifactKey::to_string,
        &format!("ambiguous Artifact {selector:?}"),
    )? {
        return Ok(key);
    }
    Err(CliError::NotFound(format!("no Artifact {selector}")))
}

fn finish_unique<T>(
    mut items: Vec<T>,
    key: impl Fn(&T) -> String,
    ambiguous_message: &str,
) -> Result<Option<T>> {
    match items.len() {
        0 => Ok(None),
        1 => Ok(Some(items.remove(0))),
        _ => {
            let mut candidates: Vec<String> = items.iter().map(key).collect();
            candidates.sort();
            Err(CliError::Ambiguous {
                message: format!("{ambiguous_message}; matches: {}", candidates.join(", ")),
                candidates,
            })
        }
    }
}
