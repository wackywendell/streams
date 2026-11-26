//! Entity field catalog, `--fields` parsing, computed field evaluation, and JSON/text projection.
//!
//! Every Stream, Session and Artifact field has one name here, used for `show`, `list`,
//! `--fields`, edit validation, and text rendering.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    artifacts::{ArtifactKey, summaries as session_artifact_summaries},
    model::{ReferenceKind, Session, Status, Stream, ValidationError},
    operations::{derived_stream_status, stream_activity_ms},
};

use super::{
    ApplicationError,
    documents::{self, DocumentLocation},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldClass {
    Editable,
    ReadOnly,
    Computed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: &'static str,
    pub class: FieldClass,
    pub is_collection: bool,
}

pub const STREAM_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "host_id",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "host_label",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "id",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "title",
        class: FieldClass::Editable,
        is_collection: false,
    },
    FieldSpec {
        name: "goal",
        class: FieldClass::Editable,
        is_collection: false,
    },
    FieldSpec {
        name: "status",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "active_sessions",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "total_sessions",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "last_activity_ms",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "sessions",
        class: FieldClass::Computed,
        is_collection: true,
    },
    FieldSpec {
        name: "artifacts",
        class: FieldClass::Computed,
        is_collection: true,
    },
    FieldSpec {
        name: "revision",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "updated_at_ms",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
];

pub const STREAM_LIST_DEFAULTS: &[&str] =
    &["key", "host_label", "status", "title", "last_activity_ms"];

pub const SESSION_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "host_id",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "host_label",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "harness",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "id",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "stream_id",
        class: FieldClass::Editable,
        is_collection: false,
    },
    FieldSpec {
        name: "title",
        class: FieldClass::Editable,
        is_collection: false,
    },
    FieldSpec {
        name: "status",
        class: FieldClass::Editable,
        is_collection: false,
    },
    FieldSpec {
        name: "recap",
        class: FieldClass::Editable,
        is_collection: false,
    },
    FieldSpec {
        name: "next",
        class: FieldClass::Editable,
        is_collection: false,
    },
    FieldSpec {
        name: "directory",
        class: FieldClass::Editable,
        is_collection: false,
    },
    FieldSpec {
        name: "references",
        class: FieldClass::Editable,
        is_collection: true,
    },
    FieldSpec {
        name: "artifacts",
        class: FieldClass::ReadOnly,
        is_collection: true,
    },
    FieldSpec {
        name: "document_files",
        class: FieldClass::Computed,
        is_collection: true,
    },
    FieldSpec {
        name: "revision",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "updated_at_ms",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
];

pub const SESSION_LIST_DEFAULTS: &[&str] =
    &["key", "host_label", "status", "title", "updated_at_ms"];

pub const ARTIFACT_FIELDS: &[FieldSpec] = &[
    FieldSpec {
        name: "key",
        class: FieldClass::Computed,
        is_collection: false,
    },
    FieldSpec {
        name: "host_id",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "harness",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "session_id",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "stream_id",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "name",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
    FieldSpec {
        name: "description",
        class: FieldClass::ReadOnly,
        is_collection: false,
    },
];

pub const ARTIFACT_LIST_DEFAULTS: &[&str] = &["key", "name", "description"];

pub fn classify_editable(
    specs: &[FieldSpec],
    entity: &str,
    field: &str,
) -> Result<(), ValidationError> {
    match specs.iter().find(|spec| spec.name == field) {
        Some(spec) if spec.class == FieldClass::Editable => Ok(()),
        Some(spec) if spec.class == FieldClass::ReadOnly => Err(ValidationError::new(format!(
            "{entity} field {field:?} is read-only and cannot be edited"
        ))),
        Some(_) => Err(ValidationError::new(format!(
            "{entity} field {field:?} is computed and cannot be edited"
        ))),
        None => {
            let valid: Vec<&str> = specs
                .iter()
                .filter(|spec| spec.class == FieldClass::Editable)
                .map(|spec| spec.name)
                .collect();
            Err(ValidationError::new(format!(
                "unknown {entity} field {field:?}; editable fields: {}",
                valid.join(", ")
            )))
        }
    }
}

/// Resolve a `--fields` argument against `specs`. Always includes `"key"` (prepended when
/// omitted), preserves requested order, deduplicates, and rejects unknown names while listing
/// all valid names.
pub fn resolve_fields(
    specs: &[FieldSpec],
    entity: &str,
    requested: Option<&str>,
    default_fields: &[&str],
) -> Result<Vec<&'static str>, ApplicationError> {
    let Some(raw) = requested else {
        return Ok(default_fields
            .iter()
            .map(|name| {
                specs
                    .iter()
                    .find(|spec| spec.name == *name)
                    .expect("default field exists in specs")
                    .name
            })
            .collect());
    };
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for part in raw.split(',') {
        let name = part.trim();
        if name.is_empty() {
            continue;
        }
        let Some(spec) = specs.iter().find(|spec| spec.name == name) else {
            let valid: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
            return Err(ApplicationError::invalid(format!(
                "unknown {entity} field {name:?}; valid fields: {}",
                valid.join(", ")
            )));
        };
        if seen.insert(spec.name) {
            out.push(spec.name);
        }
    }
    if out.is_empty() {
        let valid: Vec<&str> = specs.iter().map(|spec| spec.name).collect();
        return Err(ApplicationError::invalid(format!(
            "--fields must name at least one {entity} field; valid fields: {}",
            valid.join(", ")
        )));
    }
    if !seen.contains("key") {
        out.insert(0, "key");
    }
    Ok(out)
}

pub fn all_field_names(specs: &[FieldSpec]) -> Vec<&'static str> {
    specs.iter().map(|spec| spec.name).collect()
}

pub fn check_list_text_fields(
    specs: &[FieldSpec],
    fields: &[&str],
) -> Result<(), ApplicationError> {
    for &field in fields {
        if let Some(spec) = specs.iter().find(|s| s.name == field)
            && spec.is_collection
        {
            return Err(ApplicationError::invalid(format!(
                "field {field:?} is a collection and cannot be rendered as a list column; pass --json"
            )));
        }
    }
    Ok(())
}

/// Summary row inside `Stream.sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamSessionSummary {
    pub key: String,
    pub title: crate::model::Title,
    pub status: Status,
    pub harness: String,
    pub recap: Option<crate::model::TextPair>,
    pub next: Option<crate::model::TextPair>,
}

/// Artifact row inside `Stream.artifacts` and `artifact list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArtifactRow {
    pub key: String,
    pub host_id: String,
    pub harness: String,
    pub session_id: String,
    pub stream_id: String,
    pub name: String,
    pub description: String,
}

impl ArtifactRow {
    pub fn from_session(session: &Session, key: &ArtifactKey, description: &str) -> Self {
        Self {
            key: key.to_string(),
            host_id: session.host_id.clone(),
            harness: session.harness.clone(),
            session_id: session.id.clone(),
            stream_id: session.stream_id.clone(),
            name: key.name.clone(),
            description: description.to_owned(),
        }
    }

    pub fn project(&self, fields: &[&str]) -> Map<String, Value> {
        let mut obj = Map::new();
        for &field in fields {
            let val = match field {
                "key" => Value::String(self.key.clone()),
                "host_id" => Value::String(self.host_id.clone()),
                "harness" => Value::String(self.harness.clone()),
                "session_id" => Value::String(self.session_id.clone()),
                "stream_id" => Value::String(self.stream_id.clone()),
                "name" => Value::String(self.name.clone()),
                "description" => Value::String(self.description.clone()),
                _ => continue,
            };
            obj.insert(field.into(), val);
        }
        obj
    }
}

/// Project a Stream and its member Sessions to the requested field set.
pub fn project_stream(
    stream: &Stream,
    members: &[&Session],
    host_label: Option<&str>,
    cutoff_ms: Option<u64>,
    fields: &[&str],
) -> Map<String, Value> {
    let mut sorted_members: Vec<&Session> = members.to_vec();
    sorted_members.sort_by(|left, right| {
        right
            .updated_at_ms
            .cmp(&left.updated_at_ms)
            .then_with(|| left.key().to_string().cmp(&right.key().to_string()))
    });

    let mut obj = Map::new();
    for &field in fields {
        let value = match field {
            "key" => Value::String(stream.key().to_string()),
            "host_id" => Value::String(stream.host_id.clone()),
            "host_label" => host_label
                .map(|label| Value::String(label.to_owned()))
                .unwrap_or(Value::Null),
            "id" => Value::String(stream.id.clone()),
            "title" => serde_json::to_value(&stream.title).expect("title json"),
            "goal" => serde_json::to_value(&stream.goal).expect("goal json"),
            "status" => serde_json::to_value(derived_stream_status(stream, members, cutoff_ms))
                .expect("status json"),
            "active_sessions" => {
                let count = members.iter().filter(|s| s.status.is_open()).count();
                Value::from(count)
            }
            "total_sessions" => Value::from(members.len()),
            "last_activity_ms" => Value::from(stream_activity_ms(stream, members)),
            "sessions" => {
                let rows: Vec<StreamSessionSummary> = sorted_members
                    .iter()
                    .map(|session| StreamSessionSummary {
                        key: session.key().to_string(),
                        title: session.title.clone(),
                        status: session.status,
                        harness: session.harness.clone(),
                        recap: session.recap.clone(),
                        next: session.next.clone(),
                    })
                    .collect();
                serde_json::to_value(rows).expect("sessions json")
            }
            "artifacts" => {
                let mut rows: Vec<Value> = Vec::new();
                let mut all_artifacts: Vec<(String, String, String)> = Vec::new();
                for session in &sorted_members {
                    for summary in session_artifact_summaries(session) {
                        all_artifacts.push((
                            summary.key.to_string(),
                            summary.key.name,
                            summary.description,
                        ));
                    }
                }
                all_artifacts.sort_by(|a, b| a.0.cmp(&b.0));
                for (key, name, description) in all_artifacts {
                    let mut row = Map::new();
                    row.insert("key".into(), Value::String(key));
                    row.insert("name".into(), Value::String(name));
                    row.insert("description".into(), Value::String(description));
                    rows.push(Value::Object(row));
                }
                Value::Array(rows)
            }
            "revision" => Value::from(stream.revision),
            "updated_at_ms" => Value::from(stream.updated_at_ms),
            _ => continue,
        };
        obj.insert(field.into(), value);
    }
    obj
}

/// Project a Session to the requested field set. Computes `document_files` only when named in `fields`.
pub fn project_session(
    local_host: &str,
    session: &Session,
    host_label: Option<&str>,
    fields: &[&str],
) -> Result<Map<String, Value>, ApplicationError> {
    let mut obj = Map::new();
    for &field in fields {
        let value = match field {
            "key" => Value::String(session.key().to_string()),
            "host_id" => Value::String(session.host_id.clone()),
            "host_label" => host_label
                .map(|label| Value::String(label.to_owned()))
                .unwrap_or(Value::Null),
            "harness" => Value::String(session.harness.clone()),
            "id" => Value::String(session.id.clone()),
            "stream_id" => Value::String(session.stream_id.clone()),
            "title" => serde_json::to_value(&session.title).expect("title json"),
            "status" => serde_json::to_value(session.status).expect("status json"),
            "recap" => serde_json::to_value(&session.recap).expect("recap json"),
            "next" => serde_json::to_value(&session.next).expect("next json"),
            "directory" => serde_json::to_value(&session.directory).expect("directory json"),
            "references" => serde_json::to_value(&session.references).expect("references json"),
            "artifacts" => serde_json::to_value(&session.artifacts).expect("artifacts json"),
            "document_files" => {
                let files = compute_document_files(local_host, session)?;
                serde_json::to_value(files).expect("document_files json")
            }
            "revision" => Value::from(session.revision),
            "updated_at_ms" => Value::from(session.updated_at_ms),
            _ => continue,
        };
        obj.insert(field.into(), value);
    }
    Ok(obj)
}

pub fn compute_document_files(
    local_host: &str,
    session: &Session,
) -> Result<BTreeMap<String, DocumentLocation>, ApplicationError> {
    let mut map = BTreeMap::new();
    for (key, entry) in &session.references {
        if entry.kind == ReferenceKind::Document {
            map.insert(key.clone(), documents::locate(local_host, session, key)?);
        }
    }
    Ok(map)
}

/// Format a projected JSON field value as a compact single-line text cell for `list` tables.
pub fn format_cell(field: &str, value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Object(map) => match field {
            "title" | "goal" | "recap" | "next" => map
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_owned(),
            "directory" => map
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("-")
                .to_owned(),
            _ => value.to_string(),
        },
        Value::Array(_) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        ArtifactEntry, Goal, ReferenceEntry, ReferenceRole, ReferenceStatus, RepositoryInfo,
        SessionDirectory, TextPair, Title,
    };

    #[test]
    fn every_stored_field_has_a_class() {
        let stream = Stream {
            host_id: "h".into(),
            id: "s1".into(),
            title: Title::new("Stream", false),
            goal: Some(Goal::new("Goal", Some("g".into()), true)),
            revision: 1,
            updated_at_ms: 100,
        };
        let stream_obj = serde_json::to_value(&stream).unwrap();
        for key in stream_obj.as_object().unwrap().keys() {
            let spec = STREAM_FIELDS
                .iter()
                .find(|s| s.name == key)
                .unwrap_or_else(|| {
                    panic!("Stream stored field {key:?} missing from STREAM_FIELDS")
                });
            assert!(
                matches!(spec.class, FieldClass::Editable | FieldClass::ReadOnly),
                "Stream stored field {key:?} must be Editable or ReadOnly"
            );
        }

        let mut references = BTreeMap::new();
        references.insert(
            "docs/a.md".into(),
            ReferenceEntry {
                kind: ReferenceKind::Document,
                name: "A".into(),
                purpose: "Design".into(),
                role: ReferenceRole::Brief,
                status: ReferenceStatus::Active,
                manual: false,
            },
        );
        let mut artifacts = BTreeMap::new();
        artifacts.insert(
            "notes.md".into(),
            ArtifactEntry {
                description: "Notes".into(),
            },
        );
        let session = Session {
            host_id: "h".into(),
            harness: "pi".into(),
            id: "sess1".into(),
            stream_id: "s1".into(),
            title: Title::new("Sess", false),
            status: Status::Active,
            recap: Some(TextPair::new("Recap", None)),
            next: Some(TextPair::new("Next", Some("nx".into()))),
            directory: Some(SessionDirectory {
                path: "/tmp/work".into(),
                repository: Some(RepositoryInfo {
                    url: "git@example.com:o/r.git".into(),
                    within: None,
                    branch: Some("main".into()),
                }),
            }),
            references,
            artifacts,
            revision: 1,
            updated_at_ms: 100,
        };
        let session_obj = serde_json::to_value(&session).unwrap();
        for key in session_obj.as_object().unwrap().keys() {
            let spec = SESSION_FIELDS
                .iter()
                .find(|s| s.name == key)
                .unwrap_or_else(|| {
                    panic!("Session stored field {key:?} missing from SESSION_FIELDS")
                });
            assert!(
                matches!(spec.class, FieldClass::Editable | FieldClass::ReadOnly),
                "Session stored field {key:?} must be Editable or ReadOnly"
            );
        }
    }
}
