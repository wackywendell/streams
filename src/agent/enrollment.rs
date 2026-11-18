//! Validated enrollment intent shared by live adapters and effect-free replay.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    codec::{ascii_json, object, optional_object, require_object},
    model::{NamingMode, Session, Stream, TitleMode, ValidationError, check_text},
    operations::Actor,
    runtime::conversations::Harness,
};

use super::session_id_for;

/// Initial effort text, not an update or a claim of effective ownership.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamProposal {
    title: String,
    goal: Option<String>,
    goal_short: Option<String>,
}

impl StreamProposal {
    pub fn title(&self) -> &str {
        &self.title
    }
    pub fn goal(&self) -> Option<&str> {
        self.goal.as_deref()
    }
    pub fn goal_short(&self) -> Option<&str> {
        self.goal_short.as_deref()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SessionProposal {
    title: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnrollmentWire {
    version: u32,
    catalog_source: String,
    harness: Harness,
    harness_session_id: String,
    stream_id: Option<String>,
    #[serde(default, deserialize_with = "optional_object")]
    stream: Option<StreamProposal>,
    #[serde(deserialize_with = "object")]
    session: SessionProposal,
    actor: Actor,
    cwd: String,
}

/// Parsed and locally validated intent. It neither attests native identity nor resolves cwd.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnrollmentRequest {
    catalog_source: String,
    harness: Harness,
    harness_session_id: String,
    stream_id: Option<String>,
    stream: Option<StreamProposal>,
    session_title: String,
    actor: Actor,
    cwd: String,
}

impl EnrollmentRequest {
    /// Preserve the enrollment wire shape while fencing the physical catalog source.
    pub fn parse(value: Value, catalog_source: &str) -> Result<Self, ValidationError> {
        require_object(&value, "agent enrollment")?;
        let wire: EnrollmentWire = serde_json::from_value(value)
            .map_err(|e| ValidationError::new(format!("invalid agent enrollment: {e}")))?;
        if wire.version != 2 {
            return Err(ValidationError::new(
                "captured enrollment requires version 2",
            ));
        }
        if wire.stream_id.is_some() && wire.stream.is_some() {
            return Err(ValidationError::new(
                "select an existing Stream or propose a new one, not both",
            ));
        }
        if wire.actor == Actor::Native {
            return Err(ValidationError::new(
                "native operations are library-owned, not enrollment requests",
            ));
        }
        check_text(&wire.catalog_source, "catalog_source", false)?;
        if wire.catalog_source != catalog_source {
            return Err(ValidationError::new(
                "catalog source changed since capture; inspect the current catalog before making a new request",
            ));
        }
        check_text(&wire.harness_session_id, "harness_session_id", false)?;
        check_text(&wire.cwd, "cwd", false)?;
        if let Some(id) = &wire.stream_id {
            check_text(id, "stream_id", false)?;
        }
        let request = Self {
            catalog_source: wire.catalog_source,
            harness: wire.harness,
            harness_session_id: wire.harness_session_id,
            stream_id: wire.stream_id,
            stream: wire.stream,
            session_title: wire.session.title,
            actor: wire.actor,
            cwd: wire.cwd,
        };
        // Validate all proposed text even when an adapter later finds existing records.
        // Placeholder identities have no significance and never leave this pure parser.
        request.session("validation-host", "validation-stream")?;
        if request.stream.is_some() {
            request.stream("validation-host", "validation-stream")?;
        }
        Ok(request)
    }

    pub fn catalog_source(&self) -> &str {
        &self.catalog_source
    }
    pub fn harness(&self) -> Harness {
        self.harness
    }
    pub fn harness_session_id(&self) -> &str {
        &self.harness_session_id
    }
    pub fn stream_id(&self) -> Option<&str> {
        self.stream_id.as_deref()
    }
    pub fn stream_proposal(&self) -> Option<&StreamProposal> {
        self.stream.as_ref()
    }
    pub fn session_title(&self) -> &str {
        &self.session_title
    }
    pub fn actor(&self) -> Actor {
        self.actor
    }
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// Build a new effort through normal model validation and user naming semantics.
    pub fn stream(&self, host_id: &str, id: &str) -> Result<Stream, ValidationError> {
        let proposal = self
            .stream
            .as_ref()
            .ok_or_else(|| ValidationError::new("new effort enrollment requires a stream title"))?;
        let mut stream = Stream::new(id, &proposal.title, proposal.goal.clone(), host_id)?;
        stream.goal_short = proposal.goal_short.clone();
        if self.actor == Actor::User {
            stream.title_mode = TitleMode::Manual;
            if stream.goal.is_some() {
                stream.goal_mode = NamingMode::Manual;
            }
        }
        stream.validated()
    }

    /// Build a stable native conversation card, without resolving or binding its cwd.
    pub fn session(&self, host_id: &str, stream_id: &str) -> Result<Session, ValidationError> {
        let mut session = Session::new(
            session_id_for(host_id, self.harness, &self.harness_session_id),
            host_id,
            stream_id,
            self.harness.as_ref(),
            &self.harness_session_id,
            &self.session_title,
        )?;
        if self.actor == Actor::User {
            session.title_mode = TitleMode::Manual;
        }
        session.validated()
    }
}

/// Stable live effort identity matching Python UUIDv5 and json.dumps ASCII spelling.
pub fn effort_id_for(host_id: &str, harness: Harness, native_id: &str) -> String {
    let identity = format!(
        "streams:effort:[{}, {}, {}]",
        ascii_json(host_id),
        ascii_json(harness.as_ref()),
        ascii_json(native_id),
    );
    uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, identity.as_bytes()).to_string()
}
