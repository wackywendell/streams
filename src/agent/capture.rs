//! Read-only replay inputs from frozen local inspection and caller-observed topology.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use crate::{
    application::InspectedCatalog,
    codec::{object, record_to_data},
    frontend::herdr::{HerdrAgent, HerdrTopology},
    model::{Record, Session, Stream, ValidationError, check_text},
    runtime::{
        conversations::{
            ConversationRoots, Harness, NativeIdentity, NativeSessionMetadata, conversation_file,
            native_identity,
        },
        state::RuntimeState,
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

fn sample_context(
    inspected: &InspectedCatalog,
    session: Option<&Session>,
    fallback: Option<&Stream>,
) -> Value {
    let catalog = &inspected.inspection.snapshot.catalog;
    let stream = session
        .and_then(|s| catalog.stream(&s.stream_id))
        .or(fallback);
    let authority = stream
        .and_then(|s| inspected.inspection.authorities.get(&s.id))
        .map(|a| json!({"host_id": a.host_id, "token": "[redacted]"}));
    json!({
        "catalog_source": inspected.source, "host_id": inspected.host.as_str(),
        "stream": stream.map(|s| record_to_data(&Record::Stream(s.clone()))),
        "session": session.map(|s| record_to_data(&Record::Session(s.clone()))),
        "sessions": catalog.sessions().iter()
            .filter(|s| stream.is_some_and(|stream| s.stream_id == stream.id))
            .map(|s| record_to_data(&Record::Session(s.clone()))).collect::<Vec<_>>(),
        "authority": authority,
    })
}

/// Capture actual local records and bounded saved-file identities, never projected state.
/// All native/backend acquisition belongs to the caller; this function takes no effect capability.
pub fn capture_inputs(
    inspected: &InspectedCatalog,
    state: &RuntimeState,
    native: Result<(&HerdrTopology, &str), String>,
    target: CaptureTarget<'_>,
    roots: &ConversationRoots,
) -> Result<Value, ValidationError> {
    let host_id = inspected.host.as_str();
    check_text(&inspected.source, "catalog_source", false)?;
    if state.host_id() != host_id {
        return Err(ValidationError::new(
            "runtime state belongs to another host",
        ));
    }
    let catalog = &inspected.inspection.snapshot.catalog;
    let mut selected = match target {
        CaptureTarget::Stream(id) => Some(
            catalog
                .stream(id)
                .ok_or_else(|| ValidationError::new(format!("no stream with exact ID {id:?}")))?,
        ),
        _ => None,
    };
    let mut errors = Vec::new();
    let mut bindings = BTreeMap::new();
    let topology = match native {
        Err(error) => {
            errors.push(error);
            None
        }
        Ok((topology, scope)) => {
            let inventory = (|| {
                let mut bindings = BTreeMap::new();
                for scoped in state
                    .associations(Some(scope))
                    .map_err(|e| ValidationError::new(e.to_string()))?
                {
                    let binding = scoped.record();
                    if !binding.ready() {
                        continue;
                    }
                    if catalog.stream(binding.stream_id()).is_none()
                        || bindings
                            .insert(
                                binding.workspace_id().to_owned(),
                                binding.stream_id().to_owned(),
                            )
                            .is_some()
                    {
                        return Err(ValidationError::new(
                            "ambiguous or dangling ready workspace association",
                        ));
                    }
                }
                Ok(bindings)
            })();
            match inventory {
                Ok(found) => {
                    bindings = found;
                    Some(topology)
                }
                Err(error) => {
                    errors.push(error.to_string());
                    None
                }
            }
        }
    };
    let workspaces: Vec<Value> = topology
        .into_iter()
        .flat_map(|t| &t.workspaces)
        .map(|w| json!({"id": w.id, "label": w.label, "stream_id": bindings.get(&w.id)}))
        .collect();
    let mut result = json!({
        "version": 1, "host_id": host_id,
        "streams": catalog.streams().iter().map(|s| record_to_data(&Record::Stream(s.clone()))).collect::<Vec<_>>(),
        "workspaces": workspaces,
        "topology_error": topology_error(&errors),
    });
    let (target, workspace_ids) = match target {
        CaptureTarget::Inventory => return Ok(result),
        CaptureTarget::Workspace(id) => {
            let workspace = topology
                .and_then(|t| t.workspaces.iter().find(|w| w.id == id))
                .ok_or_else(|| {
                    ValidationError::new(format!(
                        "workspace {id:?} not observed in valid Herdr topology"
                    ))
                })?;
            selected = bindings.get(id).and_then(|id| catalog.stream(id));
            (
                json!({"kind": "workspace", "id": id, "title": workspace.label}),
                vec![id.to_owned()],
            )
        }
        CaptureTarget::Stream(_) => {
            let stream = selected.expect("exact stream target was checked");
            let workspace_ids = topology
                .into_iter()
                .flat_map(|t| &t.workspaces)
                .filter(|w| bindings.get(&w.id) == Some(&stream.id))
                .map(|w| w.id.clone())
                .collect();
            (
                json!({"kind": "stream", "id": stream.id, "title": stream.title}),
                workspace_ids,
            )
        }
    };
    let mut live: BTreeMap<String, Vec<VerifiedAgent<'_>>> = BTreeMap::new();
    let mut unsupported = Vec::new();
    let mut unresolved = Vec::new();
    if let Some(topology) = topology {
        for workspace in &workspace_ids {
            for agent in workspace_agents(topology, workspace, &mut unresolved, &mut errors) {
                if agent.harness != Harness::Pi {
                    unsupported.push(json!({
                        "harness": agent.harness, "native_session_id": agent.identity.id,
                        "workspace_id": workspace,
                    }));
                } else {
                    live.entry(agent.identity.id.clone())
                        .or_default()
                        .push(agent);
                }
            }
        }
    }
    let mut known = BTreeMap::new();
    let mut samples = BTreeMap::new();
    for session in catalog.sessions() {
        if known.insert(session.native_identity(), session).is_some() {
            return Err(ValidationError::new(
                "ambiguous catalog native conversation identity",
            ));
        }
        if selected.is_some_and(|stream| stream.id == session.stream_id) {
            if session.harness != "pi" {
                unsupported.push(json!({
                    "host_id": session.host_id, "harness": session.harness,
                    "native_session_id": session.harness_session_id,
                }));
            } else {
                samples.insert(
                    (session.host_id.clone(), session.harness_session_id.clone()),
                    Some(session),
                );
            }
        }
    }
    for native_id in live.keys() {
        samples.insert(
            (host_id.to_owned(), native_id.clone()),
            known.get(&(host_id, "pi", native_id.as_str())).copied(),
        );
    }
    if topology.is_some_and(|t| {
        t.agents.iter().any(|a| {
            a.native.get("agent").and_then(Value::as_str) == Some("pi")
                && !workspace_ids.contains(&a.workspace_id)
        })
    }) && samples
        .keys()
        .any(|(host, native)| host == host_id && !live.contains_key(native))
    {
        errors.push("Pi identities outside selected workspaces were not verified; historical open state unknown".into());
    }
    let mut captured = Vec::new();
    for ((host, native_id), session) in samples {
        let local = host == host_id;
        let agents = if local {
            live.get(&native_id).map(Vec::as_slice).unwrap_or_default()
        } else {
            &[]
        };
        let locations: Vec<Value> = agents
            .iter()
            .map(|a| {
                json!({
                    "workspace_id": a.agent.workspace_id, "tab_id": a.agent.tab_id,
                    "pane_id": a.agent.pane_id, "terminal_id": a.agent.terminal_id,
                })
            })
            .collect();
        let fallback_ids: BTreeSet<_> = agents
            .iter()
            .map(|a| bindings.get(&a.agent.workspace_id).map(String::as_str))
            .collect();
        if session.is_none() && fallback_ids.len() > 1 {
            return Err(ValidationError::new(
                "ambiguous workspace stream context for uncarded conversation",
            ));
        }
        let fallback = fallback_ids
            .iter()
            .next()
            .copied()
            .flatten()
            .and_then(|id| catalog.stream(id));
        let (transcript, unavailable) = if !local {
            (None, Some("foreign host".to_owned()))
        } else {
            let locators: BTreeSet<_> = agents
                .iter()
                .filter_map(|a| a.identity.locator.as_deref())
                .collect();
            if locators.len() > 1 {
                (
                    None,
                    Some("ambiguous saved conversation locators".to_owned()),
                )
            } else {
                match conversation_file(
                    Harness::Pi,
                    &native_id,
                    locators.iter().next().copied(),
                    roots,
                ) {
                    Ok(path) => (Some(path), None),
                    Err(error) => (None, Some(error.to_string())),
                }
            }
        };
        let open = if !agents.is_empty() {
            Some(true)
        } else if !errors.is_empty() || !local {
            None
        } else {
            Some(false)
        };
        captured.push(json!({
            "host_id": host, "harness": "pi", "native_session_id": native_id,
            "open": open, "locations": locations, "catalog": sample_context(inspected, session, fallback),
            "transcript_file": transcript, "unavailable": unavailable,
        }));
    }
    result["target"] = target;
    result["samples"] = json!(captured);
    result["unsupported"] = json!(unsupported);
    result["unresolved"] = json!(unresolved);
    result["topology_error"] = json!(topology_error(&errors));
    Ok(result)
}
