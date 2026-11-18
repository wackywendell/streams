//! Typed field changes to a Stream or Session. A field left `None` is unchanged; an
//! optional field set to `Some(None)` is cleared. Each field names who may set it.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value, json};
use strum::VariantNames;

use super::Actor;
use crate::model::{
    NamingMode, Record, Repository, Session, Status, Stream, TitleDefaults, TitleMode,
    ValidationError,
};

/// Who may set a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Prose an automatic report may write, unless the record protects it as manual.
    Report,
    /// Lifecycle and naming modes: user intent only.
    User,
    /// Edited only by direct CLI commands, never through an agent request.
    Cli,
}

impl Access {
    /// Whether `actor` may set a field with this access. `agent` marks a captured agent
    /// request rather than a direct command.
    pub fn permits(self, actor: Actor, agent: bool) -> bool {
        match (self, actor) {
            (_, Actor::Native) => false,
            (Self::Report, _) => true,
            (Self::User, Actor::User) => true,
            (Self::Cli, Actor::User) => !agent,
            (Self::User | Self::Cli, Actor::Automatic) => false,
        }
    }
}

/// The JSON schema an agent request's value for a field must match.
trait FieldSchema {
    fn schema() -> Value;
}
impl FieldSchema for String {
    fn schema() -> Value {
        json!({"type": "string"})
    }
}
impl FieldSchema for Option<String> {
    fn schema() -> Value {
        json!({"anyOf": [{"type": "string"}, {"type": "null"}]})
    }
}
macro_rules! choice_schema {
    ($($ty:ty),*) => {
        $(impl FieldSchema for $ty {
            fn schema() -> Value {
                json!({"type": "string", "enum": <$ty>::VARIANTS})
            }
        })*
    };
}
choice_schema!(Status, TitleMode, NamingMode);
impl FieldSchema for BTreeMap<String, Repository> {
    fn schema() -> Value {
        json!({"type": "object"})
    }
}

/// A present JSON field, including an explicit null, sets the patch field.
fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// One settable field of a patch.
pub trait PatchField: Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    /// The wire name, as conflicts and agent requests spell it.
    fn name(self) -> &'static str;
    fn access(self) -> Access;
    fn schema(self) -> Value;
}

/// What edits need from either patch, so one reconciliation serves both record kinds.
pub trait Patch: Clone + Default + PartialEq {
    type Record: Clone + PartialEq;
    type Field: PatchField;
    /// The patch that turns `before` into `after`.
    fn between(before: &Self::Record, after: &Self::Record) -> Self;
    fn apply(&self, record: &mut Self::Record);
    fn fields(&self) -> Vec<Self::Field>;
    fn remove(&mut self, field: Self::Field);
    fn check(record: &Self::Record) -> Result<(), ValidationError>;
    fn revision(record: &Self::Record) -> u64;
    fn into_record(record: Self::Record) -> Record;

    fn is_empty(&self) -> bool {
        self.fields().is_empty()
    }
    /// Whether both patches set some field.
    fn overlaps(&self, other: &Self) -> bool {
        let other = other.fields();
        self.fields().iter().any(|field| other.contains(field))
    }
}

macro_rules! patch {
    (
        $(#[$meta:meta])*
        $patch:ident, $field_enum:ident for $record:ident, check $check:ident {
            $($field:ident => $variant:ident: $ty:ty, $access:ident;)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $patch {
            $(
                #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
                pub $field: Option<$ty>,
            )*
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $field_enum {
            $($variant,)*
        }

        impl PatchField for $field_enum {
            const ALL: &'static [Self] = &[$(Self::$variant,)*];

            fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => stringify!($field),)*
                }
            }

            fn access(self) -> Access {
                match self {
                    $(Self::$variant => Access::$access,)*
                }
            }

            fn schema(self) -> Value {
                match self {
                    $(Self::$variant => <$ty as FieldSchema>::schema(),)*
                }
            }
        }

        impl Patch for $patch {
            type Record = $record;
            type Field = $field_enum;

            fn between(before: &$record, after: &$record) -> Self {
                Self {
                    $($field: (before.$field != after.$field).then(|| after.$field.clone()),)*
                }
            }

            fn apply(&self, record: &mut $record) {
                $(if let Some(value) = &self.$field {
                    record.$field = value.clone();
                })*
            }

            fn fields(&self) -> Vec<$field_enum> {
                let mut fields = Vec::new();
                $(if self.$field.is_some() {
                    fields.push($field_enum::$variant);
                })*
                fields
            }

            fn remove(&mut self, field: $field_enum) {
                match field {
                    $($field_enum::$variant => self.$field = None,)*
                }
            }

            fn check(record: &$record) -> Result<(), ValidationError> {
                record.$check()
            }

            fn revision(record: &$record) -> u64 {
                record.revision
            }

            fn into_record(record: $record) -> Record {
                Record::$record(record)
            }
        }
    };
}

patch! {
    /// Changes to a Stream's own fields. References change through `ReferenceChanges`.
    StreamPatch, StreamField for Stream, check validate {
        title => Title: String, Report;
        title_mode => TitleMode: TitleMode, User;
        goal => Goal: Option<String>, Report;
        goal_short => GoalShort: Option<String>, Report;
        goal_mode => GoalMode: NamingMode, User;
        status => Status: Status, User;
        repositories => Repositories: BTreeMap<String, Repository>, Cli;
    }
}

patch! {
    /// Changes to a Session's own fields. Membership changes through the move operation.
    SessionPatch, SessionField for Session, check validate_portable {
        title => Title: String, Report;
        title_mode => TitleMode: TitleMode, User;
        status => Status: Status, Report;
        recap => Recap: String, Report;
        recap_short => RecapShort: Option<String>, Report;
        state => State: String, Report;
        state_short => StateShort: Option<String>, Report;
        remaining => Remaining: String, Report;
        remaining_short => RemainingShort: Option<String>, Report;
    }
}

impl StreamPatch {
    /// Report fields the record holds as manual text.
    pub fn protected(before: &Stream, title_default: NamingMode) -> Vec<StreamField> {
        let mut fields = Vec::new();
        if before.title_mode.resolve(title_default) == NamingMode::Manual {
            fields.push(StreamField::Title);
        }
        if before.goal_mode == NamingMode::Manual {
            fields.extend([StreamField::Goal, StreamField::GoalShort]);
        }
        fields
    }

    /// A user's text edit makes that text manual, and a changed goal clears its short form.
    /// An explicitly supplied mode or short form is kept.
    pub fn normalized(mut self, before: &Stream, actor: Actor) -> Self {
        if actor == Actor::User {
            if self.title.is_some() {
                self.title_mode.get_or_insert(TitleMode::Manual);
            }
            if self.goal.is_some() {
                self.goal_mode.get_or_insert(NamingMode::Manual);
            }
        }
        if self.goal.as_ref().is_some_and(|goal| *goal != before.goal) {
            self.goal_short.get_or_insert(None);
        }
        self
    }
}

impl SessionPatch {
    /// Report fields the record holds as manual text.
    pub fn protected(before: &Session, title_default: NamingMode) -> Vec<SessionField> {
        if before.title_mode.resolve(title_default) == NamingMode::Manual {
            vec![SessionField::Title]
        } else {
            Vec::new()
        }
    }

    /// A user's title edit makes it manual, and changed prose clears its short form.
    /// An explicitly supplied mode or short form is kept.
    pub fn normalized(mut self, before: &Session, actor: Actor) -> Self {
        if actor == Actor::User && self.title.is_some() {
            self.title_mode.get_or_insert(TitleMode::Manual);
        }
        for (text, current, short) in [
            (&self.recap, &before.recap, &mut self.recap_short),
            (&self.state, &before.state, &mut self.state_short),
            (
                &self.remaining,
                &before.remaining,
                &mut self.remaining_short,
            ),
        ] {
            if text.as_ref().is_some_and(|text| text != current) {
                short.get_or_insert(None);
            }
        }
        self
    }
}

/// The fields an agent request for `actor` may change, in declaration order, each with the
/// JSON schema of its value, without the fields the record protects from automatic reports.
/// A list keeps the order a frontend shows them in.
fn agent_schema<F: PatchField>(actor: Actor, protected: &[F]) -> Value {
    F::ALL
        .iter()
        .copied()
        .filter(|field| field.access().permits(actor, true))
        .filter(|field| actor != Actor::Automatic || !protected.contains(field))
        .map(|field| json!({"field": field.name(), "schema": field.schema()}))
        .collect()
}

fn actor_schemas<F: PatchField>(protected: &[F]) -> Value {
    json!({
        "automatic": agent_schema(Actor::Automatic, protected),
        "user": agent_schema(Actor::User, protected),
    })
}

/// What an agent may change in the captured records, per record kind and actor. A frontend
/// builds its model tools and edit forms from these; apply still checks every request.
pub fn agent_patch_schemas(
    stream: Option<&Stream>,
    session: Option<&Session>,
    defaults: TitleDefaults,
) -> Value {
    let mut schemas = Map::new();
    if let Some(stream) = stream {
        let protected = StreamPatch::protected(stream, defaults.stream);
        schemas.insert("stream".into(), actor_schemas(&protected));
    }
    if let Some(session) = session {
        let protected = SessionPatch::protected(session, defaults.session);
        schemas.insert("session".into(), actor_schemas(&protected));
    }
    Value::Object(schemas)
}

#[cfg(test)]
mod tests;
