//! Read-only replay inputs from frozen local inspection and caller-observed topology.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::storage::records::RecordStore;
use crate::{
    application::OpenedRecords,
    codec::{object, record_to_data},
    frontend::herdr::{HerdrAgent, HerdrNativeSnapshot, HerdrTopology},
    model::{Record, Session, Stream, TitleDefaults, ValidationError, check_text},
    runtime::{
        DirectoryBindings,
        conversations::{
            ConversationRoots, Harness, NativeIdentity, NativeSessionMetadata, conversation_file,
            native_identity,
        },
        native_markers::{selected_stream, stream_tokens},
    },
};

/// Exact selection only. A workspace must occur in the supplied valid native inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget<'a> {
    Inventory,
    Stream(&'a str),
    Workspace(&'a str),
}

struct VerifiedAgent<'a> {
    agent: &'a HerdrAgent,
    harness: Harness,
    identity: NativeIdentity,
}

fn workspace_agents<'a>(
    topology: &'a HerdrTopology,
    workspace: &str,
    unresolved: &mut Vec<Value>,
    errors: &mut Vec<String>,
) -> Vec<VerifiedAgent<'a>> {
    let mut candidates = Vec::new();
    let mut counts = BTreeMap::new();
    for agent in topology
        .agents
        .iter()
        .filter(|a| a.workspace_id == workspace)
    {
        let harness = match agent.native.get("agent").and_then(Value::as_str) {
            Some("pi") => Harness::Pi,
            Some("codex") => Harness::Codex,
            Some("claude") => Harness::Claude,
            _ => continue,
        };
        let identity = (|| {
            let metadata = agent
                .native
                .get("agent_session")
                .ok_or_else(|| ValidationError::new("missing native agent_session"))?;
            let metadata: NativeSessionMetadata = object(metadata.clone())
                .map_err(|e| ValidationError::new(format!("invalid native agent_session: {e}")))?;
            native_identity(&metadata, harness).map_err(|e| ValidationError::new(e.to_string()))
        })();
        match identity {
            Ok(identity) => {
                *counts
                    .entry((harness.as_ref().to_owned(), identity.id.clone()))
                    .or_insert(0) += 1;
                candidates.push(VerifiedAgent {
                    agent,
                    harness,
                    identity,
                });
            }
            Err(error) => unresolved_agent(agent, harness, &error.to_string(), unresolved, errors),
        }
    }
    // Only verified identities can conflict; unidentified neighbors cannot hide them.
    candidates
        .into_iter()
        .filter(|candidate| {
            if counts[&(
                candidate.harness.as_ref().to_owned(),
                candidate.identity.id.clone(),
            )] > 1
            {
                unresolved_agent(
                    candidate.agent,
                    candidate.harness,
                    "ambiguous live native conversation identity",
                    unresolved,
                    errors,
                );
                false
            } else {
                true
            }
        })
        .collect()
}

fn unresolved_agent(
    agent: &HerdrAgent,
    harness: Harness,
    error: &str,
    unresolved: &mut Vec<Value>,
    errors: &mut Vec<String>,
) {
    unresolved.push(json!({
        "workspace_id": agent.workspace_id, "tab_id": agent.tab_id,
        "pane_id": agent.pane_id, "terminal_id": agent.terminal_id,
        "harness": harness, "error": error,
    }));
    errors.push(format!(
        "workspace {:?}, pane {:?}: {error}",
        agent.workspace_id, agent.pane_id,
    ));
}

fn topology_error(errors: &[String]) -> Option<String> {
    let message = errors.join("; ");
    (!message.is_empty()).then_some(message)
}

fn read_stream(opened: &OpenedRecords, id: &str) -> Result<Stream, ValidationError> {
    opened
        .records()
        .with_reader::<_, super::RecordContextError>(|reader| {
            Ok(reader
                .read_stream(opened.host().as_str(), id)?
                .ok_or_else(|| ValidationError::new("addressed Stream is missing"))?)
        })
        .map_err(|error| ValidationError::new(error.to_string()))
}

fn sample_context(
    opened: &OpenedRecords,
    key: &str,
    fallback: Option<&Stream>,
    defaults: TitleDefaults,
) -> Result<super::RecordContext, ValidationError> {
    let host = opened.host().as_str();
    opened
        .records()
        .with_reader::<_, super::RecordContextError>(|reader| {
            let session = reader.read_session(host, key)?;
            let stream = match session.as_ref() {
                Some(session) if fallback.is_some_and(|stream| stream.id == session.stream_id) => {
                    fallback.cloned()
                }
                Some(session) => Some(
                    reader
                        .read_stream(host, &session.stream_id)
                        .map_err(|error| ValidationError::new(error.to_string()))?
                        .ok_or_else(|| {
                            ValidationError::new("Session's current Stream is missing")
                        })?,
                ),
                None => fallback.cloned(),
            };
            let members: Vec<&Session> = session.iter().collect();
            let stream_status = stream
                .as_ref()
                .map(|stream| crate::operations::derived_stream_status(stream, &members, None));
            Ok(super::RecordContext {
                catalog_source: opened.source_id().into(),
                host_id: host.into(),
                title_defaults: defaults,
                stream,
                stream_status,
                sessions: session.iter().cloned().collect(),
                session,
            })
        })
        .map_err(|error| ValidationError::new(error.to_string()))
}

/// Explicit diagnostic inventory or exact Stream/workspace inputs, never a catalog roster.
/// Native acquisition is supplied; this function cannot dispatch frontend effects.
pub fn capture_inputs(
    opened: &OpenedRecords,
    native: Result<&HerdrNativeSnapshot, String>,
    target: CaptureTarget<'_>,
    defaults: TitleDefaults,
    locations: &DirectoryBindings,
    roots: &ConversationRoots,
    document_hints: impl Fn(&Stream, &Session) -> Value,
) -> Result<Value, ValidationError> {
    let host = opened.host().as_str();
    check_text(opened.source_id(), "catalog_source", false)?;
    let mut errors = Vec::new();
    let native = match native {
        Ok(native) => Some(native),
        Err(error) => {
            errors.push(error);
            None
        }
    };
    let inventory = matches!(target, CaptureTarget::Inventory);
    let mut selected = match target {
        CaptureTarget::Stream(id) => Some(read_stream(opened, id)?),
        _ => None,
    };
    let mut bindings = BTreeMap::new();
    let workspace_ids: Vec<String> = match target {
        CaptureTarget::Inventory => native
            .map(|snapshot| {
                snapshot
                    .topology()
                    .workspaces
                    .iter()
                    .map(|workspace| workspace.id.clone())
                    .collect()
            })
            .unwrap_or_default(),
        CaptureTarget::Workspace(id) => {
            if native.is_none_or(|snapshot| snapshot.workspace_tokens(id).is_none()) {
                return Err(ValidationError::new(
                    "workspace not observed in valid Herdr snapshot",
                ));
            }
            vec![id.into()]
        }
        CaptureTarget::Stream(id) => {
            let desired = stream_tokens(opened.source_id(), host, id);
            match native {
                Some(snapshot) => snapshot
                    .topology()
                    .workspaces
                    .iter()
                    .filter(|workspace| {
                        snapshot
                            .workspace_tokens(&workspace.id)
                            .is_some_and(|tokens| {
                                desired
                                    .iter()
                                    .all(|(key, value)| tokens.get(key) == Some(value))
                            })
                    })
                    .map(|workspace| workspace.id.clone())
                    .collect(),
                None => Vec::new(),
            }
        }
    };
    if let Some(snapshot) = native {
        for workspace in &workspace_ids {
            match selected_stream(snapshot, workspace, opened.source_id(), host) {
                Ok(Some(id)) => {
                    bindings.insert(workspace.clone(), id);
                }
                Ok(None) => {}
                Err(error) => errors.push(error.to_string()),
            }
        }
        if let CaptureTarget::Workspace(id) = target
            && let Some(stream) = bindings.get(id)
        {
            selected = Some(read_stream(opened, stream)?);
        }
    }
    let streams: Vec<_> = if inventory {
        opened
            .inventory()
            .map_err(|error| ValidationError::new(error.to_string()))?
            .into_iter()
            .filter_map(|read| match read.record {
                Record::Stream(stream) if stream.owner_host_id == host => {
                    Some(record_to_data(&Record::Stream(stream)))
                }
                _ => None,
            })
            .collect()
    } else {
        selected
            .iter()
            .cloned()
            .map(|stream| record_to_data(&Record::Stream(stream)))
            .collect()
    };
    let workspaces: Vec<_> = native.into_iter().flat_map(|snapshot| &snapshot.topology().workspaces)
        .filter(|workspace| inventory || workspace_ids.contains(&workspace.id))
        .map(|workspace| json!({"id":workspace.id,"label":workspace.label,"stream_id":bindings.get(&workspace.id)})).collect();
    let mut result = json!({"version":1,"host_id":host,"streams":streams,"workspaces":workspaces,"topology_error":topology_error(&errors)});
    if inventory {
        return Ok(result);
    }
    result["target"] = match target {
        CaptureTarget::Stream(id) => {
            json!({"kind":"stream","id":id,"title":selected.as_ref().expect("exact Stream").title})
        }
        CaptureTarget::Workspace(id) => {
            json!({"kind":"workspace","id":id,"title":native.expect("observed workspace").topology().workspaces.iter().find(|workspace| workspace.id == id).expect("observed workspace").label})
        }
        CaptureTarget::Inventory => unreachable!("inventory returned"),
    };
    let mut live: BTreeMap<String, Vec<VerifiedAgent<'_>>> = BTreeMap::new();
    let mut unsupported = Vec::new();
    let mut unresolved = Vec::new();
    if let Some(snapshot) = native {
        for workspace in &workspace_ids {
            for agent in
                workspace_agents(snapshot.topology(), workspace, &mut unresolved, &mut errors)
            {
                if agent.harness != Harness::Pi {
                    unsupported.push(json!({"harness":agent.harness,"native_session_id":agent.identity.id,"workspace_id":workspace}));
                } else {
                    live.entry(agent.identity.id.clone())
                        .or_default()
                        .push(agent);
                }
            }
        }
    }
    let keys: BTreeSet<_> = live.keys().cloned().collect();
    let mut samples = Vec::new();
    for id in keys {
        let agents = live.get(&id).map(Vec::as_slice).unwrap_or_default();
        let key = Session::id_for(host, "pi", &id);
        let context = sample_context(opened, &key, selected.as_ref(), defaults)?;
        let locators: BTreeSet<_> = agents
            .iter()
            .filter_map(|agent| agent.identity.locator.as_deref())
            .collect();
        let (transcript, unavailable) = if locators.len() > 1 {
            (
                None,
                Some("ambiguous observed conversation locators".to_owned()),
            )
        } else if let Some(path) = locators.first() {
            (Some((*path).to_owned()), None)
        } else if let Some(session) = &context.session
            && let Some(path) = locations
                .transcript_file(&session.stream_id, &session.id)
                .map_err(|error| ValidationError::new(error.to_string()))?
        {
            match conversation_file(Harness::Pi, &id, Some(&path), roots) {
                Ok(path) => (Some(path), None),
                Err(error) => (None, Some(error.to_string())),
            }
        } else {
            (None,Some("no retained exact transcript locator; enroll the conversation before replaying".into()))
        };
        let locations: Vec<_> = agents.iter().map(|agent| json!({"workspace_id":agent.agent.workspace_id,"tab_id":agent.agent.tab_id,"pane_id":agent.agent.pane_id,"terminal_id":agent.agent.terminal_id})).collect();
        let hints = match (&context.stream, &context.session) {
            (Some(stream), Some(session)) => document_hints(stream, session),
            _ => json!({"checkouts":[],"document_locations":[]}),
        };
        let mut catalog = serde_json::to_value(context.wire()).expect("typed capture context");
        catalog["checkouts"] = hints["checkouts"].clone();
        catalog["document_locations"] = hints["document_locations"].clone();
        samples.push(json!({"host_id":host,"harness":"pi","native_session_id":id,"open":true,"locations":locations,"catalog":catalog,"transcript_file":transcript,"unavailable":unavailable}));
    }
    result["samples"] = json!(samples);
    result["unsupported"] = json!(unsupported);
    result["unresolved"] = json!(unresolved);
    result["topology_error"] = json!(topology_error(&errors));
    Ok(result)
}
