//! Pi child v2 application messages. Transport activation is separate from this schema.
//!
//! Catalog packets and results retain their domain codecs; an object here is not an
//! admitted intent. Neither UI facts nor request fields carry native caller authority.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    codec,
    model::{NamingMode, ValidationError},
};

pub type DomainObject = Map<String, Value>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct Warnings(Vec<String>);

impl Warnings {
    pub fn new(messages: Vec<String>) -> Result<Self, ValidationError> {
        if messages.is_empty() {
            return Err(ValidationError::new(
                "saved-with-warnings requires a warning",
            ));
        }
        Ok(Self(messages))
    }

    pub fn messages(&self) -> &[String] {
        &self.0
    }
}

impl TryFrom<Vec<String>> for Warnings {
    type Error = ValidationError;
    fn try_from(messages: Vec<String>) -> Result<Self, Self::Error> {
        Self::new(messages)
    }
}

impl From<Warnings> for Vec<String> {
    fn from(warnings: Warnings) -> Self {
        warnings.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u64", into = "u64")]
pub struct RequestId(u64);

impl RequestId {
    pub fn new(value: u64) -> Result<Self, ValidationError> {
        if !(1..=9_007_199_254_740_991).contains(&value) {
            return Err(ValidationError::new(
                "request ID must be a positive JavaScript-safe integer",
            ));
        }
        Ok(Self(value))
    }

    pub fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<u64> for RequestId {
    type Error = ValidationError;
    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<RequestId> for u64 {
    fn from(id: RequestId) -> Self {
        id.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub struct Version2;

impl TryFrom<u8> for Version2 {
    type Error = ValidationError;
    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value != 2 {
            return Err(ValidationError::new("unsupported Pi child protocol"));
        }
        Ok(Self)
    }
}

impl From<Version2> for u8 {
    fn from(_: Version2) -> Self {
        2
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClientFrame {
    Request {
        id: RequestId,
        request: Request,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pi_context: Option<Box<PiRequestContext>>,
    },
    Cancel {
        id: RequestId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PiRequestContext {
    pub conversation: crate::integration::ConversationId,
    #[serde(deserialize_with = "codec::required_nullable")]
    pub session_file: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    SessionStarted {},
    Attach {},
    OpenCard {},
    CloseCard {},
    CardSettings {},
    SetCardMode {
        target: CardTarget,
        mode: crate::config::CardMode,
    },
    Context {
        #[serde(skip_serializing_if = "Option::is_none")]
        stream_id: Option<String>,
    },
    CaptureInputs {
        target: CaptureTarget,
    },
    Preview {
        catalog: DomainObject,
        action: PreviewAction,
        request: DomainObject,
    },
    Enroll {
        request: DomainObject,
    },
    Apply {
        request: DomainObject,
    },
    ListStreams {},
    Artifact {
        request: crate::artifacts::ArtifactRequest,
    },
    SetTitleDefault {
        target: TitleTarget,
        mode: NamingMode,
    },
    GetConfig {},
    SetConfig {
        key: String,
        value: String,
    },
    UnsetConfig {
        key: String,
    },
    Sync {},
    Lifecycle {
        action: LifecycleAction,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum LifecycleAction {
    Resume,
    Park,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum TitleTarget {
    Stream,
    Session,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureTarget {
    Inventory {},
    Stream { stream_id: String },
    Workspace { workspace_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum PreviewAction {
    Apply,
    Enroll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, strum::EnumString, strum::VariantNames)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum CardTarget {
    Global,
    Session,
}

crate::codec::deserialize_string_enum!(PreviewAction, TitleTarget, CardTarget, LifecycleAction);

/// A desired label for the bound Pi session, not a native scope or work selector.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiFacts {
    #[serde(deserialize_with = "codec::required_nullable")]
    pub desired_pi_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ServerFrame {
    Ready {
        protocol: Version2,
    },
    Response {
        id: RequestId,
        outcome: Outcome,
    },
    View {
        #[serde(deserialize_with = "codec::object")]
        view: UiFacts,
    },
    Warning {
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Success {
        result: DomainObject,
    },
    SavedWithWarnings {
        result: DomainObject,
        warnings: Warnings,
    },
    Error {
        #[serde(deserialize_with = "codec::object")]
        error: RequestError,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestError {
    pub kind: String,
    pub message: String,
}

pub fn decode_client(line: &str) -> Result<ClientFrame, ValidationError> {
    decode(line)
}

pub fn decode_server(line: &str) -> Result<ServerFrame, ValidationError> {
    decode(line)
}

fn decode<T: serde::de::DeserializeOwned>(line: &str) -> Result<T, ValidationError> {
    let value = codec::load_json(line)?;
    codec::object(value)
        .map_err(|error| ValidationError::new(format!("invalid child frame: {error}")))
}

#[cfg(test)]
mod tests;
