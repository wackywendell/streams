//! Effect-free projection over the supplied addressed records, never native authority.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{
    codec::{
        ascii_json, load_json, object, record_from_data, record_to_data, require_object,
        required_nullable,
    },
    model::{Record, RecordKind, Session, Stream, TitleDefaults, ValidationError, check_text},
    operations::Reconciliation,
};

use super::{
    EnrollmentRequest,
    record_update::{CapturedEdit, parse_record_update},
};

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
    title_defaults: TitleDefaults,
    #[serde(deserialize_with = "required_nullable")]
    stream: Option<Value>,
    #[serde(deserialize_with = "required_nullable")]
    session: Option<Value>,
    sessions: Vec<Value>,
    // Presentation data is preserved, not interpreted as evidence or capabilities.
    #[serde(default, rename = "checkouts")]
    _checkouts: Option<Value>,
    #[serde(default, rename = "document_locations")]
    _documents: Option<Value>,
    #[serde(default, rename = "native")]
    _native: Option<Value>,
    #[serde(default, rename = "native_inventory")]
    _native_inventory: Option<Value>,
}

struct Snapshot {
    original: Map<String, Value>,
    source: String,
    host: String,
    defaults: TitleDefaults,
    stream: Option<Stream>,
    session: Option<Session>,
}
impl Snapshot {
    fn parse(value: &Value) -> Result<Self, ValidationError> {
        let original = require_object(value, "catalog")?.clone();
        let wire: ContextWire = object(value.clone())
            .map_err(|error| ValidationError::new(format!("invalid catalog context: {error}")))?;
        check_text(&wire.catalog_source, "catalog_source", false)?;
        check_text(&wire.host_id, "host_id", false)?;
        let stream = wire
            .stream
            .as_ref()
            .map(|value| match record_from_data(RecordKind::Stream, value)? {
                Record::Stream(stream) => Ok(stream),
                _ => unreachable!(),
            })
            .transpose()?;
        let session = wire
            .session
            .as_ref()
            .map(
                |value| match record_from_data(RecordKind::Session, value)? {
                    Record::Session(session) => {
                        session.validate_portable()?;
                        Ok(session)
                    }
                    _ => unreachable!(),
                },
            )
            .transpose()?;
        let expected = Value::Array(
            session
                .iter()
                .map(|session| record_to_data(&Record::Session(session.clone())))
                .collect(),
        );
        if serde_json::to_value(&wire.sessions).expect("JSON values serialize") != expected {
            return Err(ValidationError::new(
                "catalog.sessions must contain only the exact addressed Session",
            ));
        }
        if let Some(session) = &session
            && stream.as_ref().is_none_or(|stream| {
                stream.id != session.stream_id || stream.owner_host_id != session.host_id
            })
        {
            return Err(ValidationError::new(
                "catalog Session and Stream relationship is invalid",
            ));
        }
        Ok(Self {
            original,
            source: wire.catalog_source,
            host: wire.host_id,
            defaults: wire.title_defaults,
            stream,
            session,
        })
    }
    fn require_local(&self) -> Result<(), ValidationError> {
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| stream.owner_host_id != self.host)
            || self
                .session
                .as_ref()
                .is_some_and(|session| session.host_id != self.host)
        {
            return Err(ValidationError::new("foreign records are read-only"));
        }
        Ok(())
    }
    fn project(mut self) -> Value {
        self.original.insert("stream".into(), json!(self.stream));
        self.original.insert("session".into(), json!(self.session));
        self.original.insert(
            "sessions".into(),
            json!(self.session.iter().collect::<Vec<_>>()),
        );
        Value::Object(self.original)
    }
}

fn simulated(domain: Value) -> Value {
    json!({"simulated":true,"local":"unchanged","domain":domain,
        "detail":"Card proposal simulation only; nothing persisted or published"})
}

fn apply(snapshot: &mut Snapshot, source: &str) -> Result<Value, ValidationError> {
    snapshot.require_local()?;
    let update = parse_record_update(source, &snapshot.source, snapshot.defaults)?;
    let decision = match &update.edit {
        CapturedEdit::Session(edit) => {
            let current = snapshot
                .session
                .as_ref()
                .filter(|session| *session == edit.before())
                .ok_or_else(|| {
                    ValidationError::new("captured target does not match supplied context")
                })?;
            let stream = snapshot
                .stream
                .as_ref()
                .ok_or_else(|| ValidationError::new("current Session Stream is missing"))?;
            edit.apply(current, stream, snapshot.defaults.session)
        }
        CapturedEdit::Stream(edit) => {
            let current = snapshot
                .stream
                .as_ref()
                .filter(|stream| *stream == edit.before())
                .ok_or_else(|| {
                    ValidationError::new("captured target does not match supplied context")
                })?;
            if edit
                .captured_session()
                .is_some_and(|session| Some(session) != snapshot.session.as_ref())
            {
                return Err(ValidationError::new(
                    "captured input does not match supplied context",
                ));
            }
            edit.apply(current, snapshot.session.as_ref(), snapshot.defaults.stream)
        }
    };
    let save = simulated(
        json!({"operation_id":update.operation_id(),"outcome":decision.outcome(),
        "conflicts":decision.conflicts(),"reason":decision.reason()}),
    );
    match decision {
        Reconciliation::Changes { record, .. } => match *record {
            Record::Session(session) => snapshot.session = Some(session),
            Record::Stream(stream) => snapshot.stream = Some(stream),
            Record::Host(_) => unreachable!("captured edits cannot target a Host"),
        },
        Reconciliation::NoChange { .. } => {}
        Reconciliation::Conflict { reason, .. } | Reconciliation::Superseded { reason, .. } => {
            return Err(ValidationError::new(reason));
        }
    }
    let mut receipt = json!({"simulated":true,"local":"unchanged","operation_id":update.operation_id(),"save":save});
    match &update.edit {
        CapturedEdit::Session(_) => receipt["session"] = json!(snapshot.session),
        CapturedEdit::Stream(_) => receipt["stream"] = json!(snapshot.stream),
    }
    Ok(receipt)
}

fn preview_effort_id(request: &EnrollmentRequest, host: &str) -> String {
    let name = format!(
        "streams:preview:effort:[{}, {}, {}, {}]",
        ascii_json(request.operation_id()),
        ascii_json(host),
        ascii_json(request.harness().as_ref()),
        ascii_json(request.harness_session_id())
    );
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, name.as_bytes()).to_string()
}

fn enroll(snapshot: &mut Snapshot, source: &str) -> Result<Value, ValidationError> {
    snapshot.require_local()?;
    let request = EnrollmentRequest::parse(source, &snapshot.source)?;
    if let Some(id) = request.stream_id()
        && snapshot
            .stream
            .as_ref()
            .is_none_or(|stream| stream.id != id)
    {
        return Err(ValidationError::new(
            "enrollment stream_id must match supplied context",
        ));
    }
    if let Some(session) = &snapshot.session
        && session.native_identity()
            != (
                snapshot.host.as_str(),
                request.harness().as_ref(),
                request.harness_session_id(),
            )
    {
        return Err(ValidationError::new(
            "enrollment native identity does not match supplied Session",
        ));
    }
    let mut saves = Vec::new();
    if request.stream_proposal().is_some() {
        if snapshot.stream.is_some() {
            return Err(ValidationError::new(
                "effort already exists; preview has no retained creation receipt",
            ));
        }
        snapshot.stream =
            Some(request.stream(&snapshot.host, &preview_effort_id(&request, &snapshot.host))?);
        saves.push(
            json!({"operation_id":format!("{}:stream",request.operation_id()),
            "save":simulated(json!({"outcome":"changes"}))}),
        );
    } else if request.stream_id().is_none() && snapshot.session.is_none() {
        return Err(ValidationError::new(
            "capture an explicit existing Stream or propose a new effort",
        ));
    }
    let stream = snapshot
        .stream
        .as_ref()
        .ok_or_else(|| ValidationError::new("enrollment requires its supplied Stream"))?;
    let outcome = if snapshot.session.is_some() {
        "no-change"
    } else {
        snapshot.session = Some(request.session(&snapshot.host, &stream.id)?);
        "changes"
    };
    saves.push(
        json!({"operation_id":format!("{}:session",request.operation_id()),
        "save":simulated(json!({"outcome":outcome,"stream_id":stream.id}))}),
    );
    Ok(
        json!({"simulated":true,"local":"unchanged","catalog_source":snapshot.source,"saves":saves,
        "stream":snapshot.stream,"session":snapshot.session}),
    )
}

/// Project one exact captured v2 request without reading or changing local state.
pub fn preview(source: &str) -> Result<Value, ValidationError> {
    let value = load_json(source)?;
    require_object(&value, "agent preview")?;
    let wire: PreviewWire = serde_json::from_value(value)
        .map_err(|error| ValidationError::new(format!("invalid agent preview: {error}")))?;
    let mut snapshot = Snapshot::parse(&wire.catalog)?;
    let request = wire.request.to_string();
    let receipt = match wire.action {
        Action::Apply => apply(&mut snapshot, &request)?,
        Action::Enroll => enroll(&mut snapshot, &request)?,
    };
    Ok(json!({"simulated":true,"local":"unchanged","catalog":snapshot.project(),"receipt":receipt}))
}
