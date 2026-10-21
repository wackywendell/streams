//! Bounded replay over caller-supplied records. No backend or live save capability enters here.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    codec::{
        ascii_json, load_json, object, record_from_data, record_to_data, require_object,
        required_nullable,
    },
    model::{Catalog, Record, RecordKind, Session, Stream, ValidationError, check_text},
    operations::{Actor, Authority, Operation, Reconciliation, reconcile},
};

use super::{enrollment::EnrollmentRequest, parse_update};

#[derive(strum::EnumString, strum::VariantNames)]
#[strum(serialize_all = "kebab-case")]
enum Action {
    Apply,
    Enroll,
}

crate::codec::deserialize_string_enum!(Action);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreviewWire {
    catalog: Value,
    action: Action,
    request: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ContextWire {
    catalog_source: String,
    host_id: String,
    #[serde(deserialize_with = "required_nullable")]
    stream: Option<Value>,
    #[serde(deserialize_with = "required_nullable")]
    session: Option<Value>,
    sessions: Vec<Value>,
    #[serde(deserialize_with = "required_nullable")]
    authority: Option<Value>,
    // Opaque presentation data is retained, not interpreted as evidence or capabilities.
    #[serde(default, rename = "checkouts")]
    _checkouts: Option<Value>,
    #[serde(default, rename = "native")]
    _native: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityWire {
    host_id: String,
    token: String,
}

struct Snapshot {
    original: Map<String, Value>,
    catalog_source: String,
    host_id: String,
    catalog: Catalog,
    session_id: Option<String>,
    session_order: Vec<(String, String)>,
    authority: Option<Authority>,
}

fn stream_record(value: &Value) -> Result<Stream, ValidationError> {
    match record_from_data(RecordKind::Stream, value)? {
        Record::Stream(stream) => Ok(stream),
        _ => unreachable!("stream codec returns stream records"),
    }
}

fn session_record(value: &Value) -> Result<Session, ValidationError> {
    match record_from_data(RecordKind::Session, value)? {
        Record::Session(session) => Ok(session),
        _ => unreachable!("session codec returns session records"),
    }
}

impl Snapshot {
    fn parse(value: &Value) -> Result<Self, ValidationError> {
        let original = require_object(value, "catalog")?.clone();
        let wire: ContextWire = object(value.clone())
            .map_err(|e| ValidationError::new(format!("invalid catalog context: {e}")))?;
        check_text(&wire.catalog_source, "catalog_source", false)?;
        check_text(&wire.host_id, "host_id", false)?;
        let stream = wire.stream.as_ref().map(stream_record).transpose()?;
        let session = wire.session.as_ref().map(session_record).transpose()?;
        let sessions = wire
            .sessions
            .iter()
            .map(session_record)
            .collect::<Result<Vec<_>, _>>()?;
        let catalog = Catalog::new(stream.iter().cloned().collect(), sessions.clone(), vec![])?;
        if let Some(primary) = &session
            && (primary.host_id != wire.host_id
                || catalog.session(&primary.host_id, &primary.id) != Some(primary))
        {
            return Err(ValidationError::new(
                "catalog.session must match a local-host session in catalog.sessions",
            ));
        }
        let authority = wire
            .authority
            .map(|value| {
                let wire: AuthorityWire = object(value)
                    .map_err(|e| ValidationError::new(format!("invalid catalog authority: {e}")))?;
                Authority::new(wire.host_id, wire.token)
            })
            .transpose()?;
        if authority.is_some() && stream.is_none() {
            return Err(ValidationError::new("catalog authority requires a stream"));
        }
        Ok(Self {
            original,
            catalog_source: wire.catalog_source,
            host_id: wire.host_id,
            catalog,
            session_id: session.map(|s| s.id),
            session_order: sessions.into_iter().map(|s| (s.host_id, s.id)).collect(),
            authority,
        })
    }

    fn stream(&self) -> Option<&Stream> {
        self.catalog.streams().first()
    }

    fn session(&self) -> Option<&Session> {
        self.session_id
            .as_deref()
            .and_then(|id| self.catalog.session(&self.host_id, id))
    }

    fn project(mut self) -> Value {
        // Catalog canonicalizes records; a replay response retains captured presentation order.
        let mut sessions: Vec<Session> = self
            .session_order
            .iter()
            .map(|(host, id)| {
                self.catalog
                    .session(host, id)
                    .expect("replay never removes captured sessions")
                    .clone()
            })
            .collect();
        if let Some(primary) = self.session()
            && !sessions.contains(primary)
        {
            sessions.push(primary.clone());
        }
        let stream = self
            .stream()
            .map(|s| record_to_data(&Record::Stream(s.clone())));
        let session = self
            .session()
            .map(|s| record_to_data(&Record::Session(s.clone())));
        self.original
            .insert("stream".into(), stream.unwrap_or(Value::Null));
        self.original
            .insert("session".into(), session.unwrap_or(Value::Null));
        self.original.insert(
            "sessions".into(),
            Value::Array(
                sessions
                    .into_iter()
                    .map(|s| record_to_data(&Record::Session(s)))
                    .collect(),
            ),
        );
        self.original
            .insert("authority".into(), json!(self.authority));
        Value::Object(self.original)
    }
}

fn simulate(
    catalog: &mut Catalog,
    operation: &Operation,
    authority: Option<&Authority>,
    host_id: &str,
) -> Result<Value, ValidationError> {
    operation.check_producer(host_id)?;
    let result = reconcile(catalog, operation, authority);
    match &result {
        Reconciliation::Changes { record, .. } => {
            *catalog = catalog.replacing(record.as_ref().clone())?;
        }
        Reconciliation::NoChange { .. } => {}
        Reconciliation::Conflict { reason, .. } | Reconciliation::Superseded { reason, .. } => {
            return Err(ValidationError::new(reason));
        }
    }
    Ok(json!({
        "simulated": true, "local": "unchanged", "sharing": "local-only",
        "domain": {"operation_id": operation.id(), "outcome": result.outcome(),
                   "conflicts": result.conflicts(), "reason": result.reason()},
        "detail": "Card proposal simulation only; nothing persisted or published",
    }))
}

fn apply(snapshot: &mut Snapshot, source: &str) -> Result<Value, ValidationError> {
    let operation = parse_update(source, &snapshot.catalog_source)?;
    let target = match operation.before() {
        Some(Record::Session(_)) => snapshot.session().cloned().map(Record::Session),
        _ => snapshot.stream().cloned().map(Record::Stream),
    };
    if target.is_none() || operation.before() != target.as_ref() {
        return Err(ValidationError::new(
            "captured target does not match supplied context",
        ));
    }
    for captured in operation.inputs() {
        if snapshot.catalog.get(&captured.key()).as_ref() != Some(captured) {
            return Err(ValidationError::new(
                "captured input does not match supplied context",
            ));
        }
    }
    let save = simulate(
        &mut snapshot.catalog,
        &operation,
        snapshot.authority.as_ref(),
        &snapshot.host_id,
    )?;
    let record = snapshot
        .catalog
        .get(&operation.target())
        .expect("accepted replay retains target");
    let mut receipt = json!({
        "simulated": true, "local": "unchanged", "sharing": "local-only",
        "operation_id": operation.id(), "save": save,
    });
    match &record {
        Record::Session(_) => receipt["session"] = record_to_data(&record),
        Record::Stream(_) => receipt["stream"] = record_to_data(&record),
        Record::Host(_) => unreachable!("parse_update rejects host targets"),
    }
    Ok(receipt)
}

fn preview_effort_id(request: &EnrollmentRequest, host_id: &str) -> String {
    let name = format!(
        "streams:preview:effort:[{}, {}, {}, {}]",
        ascii_json(request.operation_id()),
        ascii_json(host_id),
        ascii_json(request.harness().as_ref()),
        ascii_json(request.harness_session_id()),
    );
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, name.as_bytes()).to_string()
}

fn enroll(snapshot: &mut Snapshot, source: &str) -> Result<Value, ValidationError> {
    let request = EnrollmentRequest::parse(source, &snapshot.catalog_source)?;
    if let Some(id) = request.stream_id()
        && snapshot.stream().is_none_or(|stream| stream.id != id)
    {
        return Err(ValidationError::new(
            "enrollment stream_id must match supplied context",
        ));
    }
    let harness = request.harness();
    let identity = (
        snapshot.host_id.as_str(),
        harness.as_ref(),
        request.harness_session_id(),
    );
    if let Some(session) = snapshot.session() {
        if session.native_identity() != identity {
            return Err(ValidationError::new(
                "enrollment native identity does not match supplied session",
            ));
        }
    } else if snapshot
        .catalog
        .sessions()
        .iter()
        .any(|session| session.native_identity() == identity)
    {
        return Err(ValidationError::new(
            "enrollment identity already exists outside catalog.session",
        ));
    }
    let mut saves = Vec::new();
    if snapshot.stream().is_none() {
        let stream = request.stream(
            &snapshot.host_id,
            &preview_effort_id(&request, &snapshot.host_id),
        )?;
        let operation = Operation::new(
            format!("{}:stream", request.operation_id()),
            None,
            Record::Stream(stream.clone()),
            request.actor(),
            None,
            vec![],
        )?;
        let save = simulate(&mut snapshot.catalog, &operation, None, &snapshot.host_id)?;
        saves.push(json!({"operation_id": operation.id(), "save": save}));
        snapshot.authority = Some(Authority::new(
            &snapshot.host_id,
            "[redacted-preview-authority]",
        )?);
    } else if request.actor() == Actor::User && request.stream_proposal().is_some() {
        return Err(ValidationError::new(
            "effort already exists here; attach without a stream proposal",
        ));
    }
    let authority = snapshot
        .authority
        .as_ref()
        .filter(|auth| auth.host_id == snapshot.host_id)
        .ok_or_else(|| {
            ValidationError::new(
                "effective ownership on this host is required to attach a conversation",
            )
        })?;
    let stream_id = snapshot
        .stream()
        .expect("enrollment has a stream")
        .id
        .clone();
    if snapshot.session().is_none() {
        let session = request.session(&snapshot.host_id, &stream_id)?;
        let operation = Operation::new(
            format!("{}:session", request.operation_id()),
            None,
            Record::Session(session.clone()),
            request.actor(),
            Some(authority.token.clone()),
            vec![],
        )?;
        let save = simulate(
            &mut snapshot.catalog,
            &operation,
            Some(authority),
            &snapshot.host_id,
        )?;
        saves.push(json!({"operation_id": operation.id(), "save": save}));
        snapshot.session_id = Some(session.id);
    }
    let mut receipt = json!({
        "simulated": true, "local": "unchanged", "sharing": "local-only",
        "catalog_source": snapshot.catalog_source, "saves": saves,
        "stream": record_to_data(&Record::Stream(snapshot.stream().cloned().expect("enrollment has a stream"))),
        "session": record_to_data(&Record::Session(snapshot.session().cloned().expect("enrollment has a session"))),
        "authority": authority,
    });
    if saves.is_empty() {
        // StreamCli requires a publication result for an accepted no-save enrollment.
        receipt["publication"] = json!({
            "simulated": true, "local": "unchanged", "sharing": "local-only", "domain": null,
            "detail": "Simulation only; no publication attempted",
        });
    }
    Ok(receipt)
}

/// Project one exact captured apply/enroll request without opening any local state.
pub fn preview(source: &str) -> Result<Value, ValidationError> {
    let value = load_json(source)?;
    require_object(&value, "agent preview")?;
    let wire: PreviewWire = serde_json::from_value(value)
        .map_err(|e| ValidationError::new(format!("invalid agent preview: {e}")))?;
    let mut snapshot = Snapshot::parse(&wire.catalog)?;
    let request = wire.request.to_string();
    let receipt = match wire.action {
        Action::Apply => apply(&mut snapshot, &request)?,
        Action::Enroll => enroll(&mut snapshot, &request)?,
    };
    Ok(json!({
        "simulated": true, "local": "unchanged", "sharing": "local-only",
        "catalog": snapshot.project(), "receipt": receipt,
    }))
}
