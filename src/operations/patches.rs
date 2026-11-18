//! Typed field changes to a Stream or Session. A field left `None` is unchanged; an
//! optional field set to `Some(None)` is cleared. Each field names who may set it.

use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use strum::VariantNames;

use super::Actor;
use crate::model::{
    NamingMode, Record, Repository, SHORT_TEXT_CHARS, Session, Status, Stream, TitleDefaults,
    TitleMode, ValidationError, short_text,
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

/// How an edit reached Streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// A captured agent request from a frontend.
    Agent,
    /// A direct CLI command.
    Cli,
}

impl Access {
    /// Whether `actor` may set a field with this access through `channel`.
    pub fn permits(self, actor: Actor, channel: Channel) -> bool {
        match (self, actor) {
            (_, Actor::Native) => false,
            (Self::Report, _) => true,
            (Self::User, Actor::User) => true,
            (Self::Cli, Actor::User) => channel == Channel::Cli,
            (Self::User | Self::Cli, Actor::Automatic) => false,
        }
    }
}

/// A JSON value type a field schema names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum JsonType {
    String,
    Object,
    Null,
}

/// The JSON schema of one value: the subset of JSON Schema frontends read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ValueSchema {
    #[serde(rename = "type")]
    pub kind: JsonType,
    #[serde(rename = "enum", skip_serializing_if = "<[_]>::is_empty")]
    pub choices: &'static [&'static str],
    #[serde(rename = "minLength", skip_serializing_if = "Option::is_none")]
    pub min_length: Option<usize>,
    #[serde(rename = "maxLength", skip_serializing_if = "Option::is_none")]
    pub max_length: Option<usize>,
}

impl ValueSchema {
    const fn of(kind: JsonType) -> Self {
        Self {
            kind,
            choices: &[],
            min_length: None,
            max_length: None,
        }
    }
    /// Any text, including empty text.
    pub const TEXT: Self = Self::of(JsonType::String);
    pub const NONEMPTY: Self = Self {
        min_length: Some(1),
        ..Self::TEXT
    };
    /// A short form, which names tabs and cards. Longer text is cut to fit.
    pub const SHORT: Self = Self {
        max_length: Some(SHORT_TEXT_CHARS),
        ..Self::NONEMPTY
    };
    pub const OBJECT: Self = Self::of(JsonType::Object);
    pub const NULL: Self = Self::of(JsonType::Null);
    pub const fn choice(choices: &'static [&'static str]) -> Self {
        Self {
            choices,
            ..Self::TEXT
        }
    }
}

/// The JSON schema an agent request's value for a field must match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum FieldSchema {
    Value(ValueSchema),
    /// The value, or null to clear the field.
    Nullable {
        #[serde(rename = "anyOf")]
        any_of: [ValueSchema; 2],
    },
}

impl FieldSchema {
    /// The schema of a value other than null.
    pub fn value(self) -> ValueSchema {
        match self {
            Self::Value(value) | Self::Nullable { any_of: [value, _] } => value,
        }
    }
}

/// A field's schema is its value's, and also allows null when the field is optional.
trait Optionality {
    fn schema(value: ValueSchema) -> FieldSchema {
        FieldSchema::Value(value)
    }
}
impl Optionality for String {}
impl Optionality for Status {}
impl Optionality for TitleMode {}
impl Optionality for NamingMode {}
impl Optionality for BTreeMap<String, Repository> {}
impl<T> Optionality for Option<T> {
    fn schema(value: ValueSchema) -> FieldSchema {
        FieldSchema::Nullable {
            any_of: [value, ValueSchema::NULL],
        }
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
    fn schema(self) -> FieldSchema;
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
            $($field:ident => $variant:ident: $ty:ty, $access:ident, $schema:expr;)*
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

            fn schema(self) -> FieldSchema {
                match self {
                    $(Self::$variant => <$ty as Optionality>::schema($schema),)*
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
        title => Title: String, Report, ValueSchema::NONEMPTY;
        title_mode => TitleMode: TitleMode, User, ValueSchema::choice(TitleMode::VARIANTS);
        goal => Goal: Option<String>, Report, ValueSchema::NONEMPTY;
        goal_short => GoalShort: Option<String>, Report, ValueSchema::SHORT;
        goal_mode => GoalMode: NamingMode, User, ValueSchema::choice(NamingMode::VARIANTS);
        repositories => Repositories: BTreeMap<String, Repository>, Cli, ValueSchema::OBJECT;
    }
}

patch! {
    /// Changes to a Session's own fields. Membership changes through the move operation.
    SessionPatch, SessionField for Session, check validate_portable {
        title => Title: String, Report, ValueSchema::NONEMPTY;
        title_mode => TitleMode: TitleMode, User, ValueSchema::choice(TitleMode::VARIANTS);
        status => Status: Status, Report, ValueSchema::choice(Status::VARIANTS);
        recap => Recap: String, Report, ValueSchema::TEXT;
        recap_short => RecapShort: Option<String>, Report, ValueSchema::SHORT;
        state => State: String, Report, ValueSchema::TEXT;
        state_short => StateShort: Option<String>, Report, ValueSchema::SHORT;
        remaining => Remaining: String, Report, ValueSchema::TEXT;
        remaining_short => RemainingShort: Option<String>, Report, ValueSchema::SHORT;
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
    /// An explicitly supplied mode or short form is kept, cut to a short form's length. An
    /// automatic report's short form of a missing goal is dropped.
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
        let goal = self.goal.as_ref().unwrap_or(&before.goal);
        if actor == Actor::Automatic && goal.is_none() && matches!(self.goal_short, Some(Some(_))) {
            self.goal_short = Some(None);
        }
        shorten(&mut self.goal_short);
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
    /// An explicitly supplied mode or short form is kept, cut to a short form's length.
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
            shorten(short);
        }
        self
    }
}

fn shorten(short: &mut Option<Option<String>>) {
    if let Some(Some(text)) = short {
        *text = short_text(text);
    }
}

/// One field an agent request may change, and the schema of its value. `short` marks a
/// short form, the fields cut to `SHORT_TEXT_CHARS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldRow {
    pub field: &'static str,
    pub schema: FieldSchema,
    pub short: bool,
}

/// The fields each actor may change in one record, in declaration order. A list keeps the
/// order a frontend shows them in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ActorSchemas {
    pub automatic: Vec<FieldRow>,
    pub user: Vec<FieldRow>,
}

/// What an agent may change in the captured records, per record kind and actor. A frontend
/// builds its model tools and edit forms from these; apply still checks every request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PatchSchemas {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<ActorSchemas>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<ActorSchemas>,
}

impl ActorSchemas {
    /// Automatic rows leave out the fields the record protects from automatic reports.
    fn new<F: PatchField>(protected: &[F], schema_for: impl Fn(F, Actor) -> FieldSchema) -> Self {
        let rows = |actor: Actor| {
            F::ALL
                .iter()
                .copied()
                .filter(|field| field.access().permits(actor, Channel::Agent))
                .filter(|field| actor != Actor::Automatic || !protected.contains(field))
                .map(|field| {
                    let schema = schema_for(field, actor);
                    FieldRow {
                        field: field.name(),
                        short: schema.value() == ValueSchema::SHORT,
                        schema,
                    }
                })
                .collect()
        };
        Self {
            automatic: rows(Actor::Automatic),
            user: rows(Actor::User),
        }
    }
}

impl PatchSchemas {
    pub fn new(
        stream: Option<&Stream>,
        session: Option<&Session>,
        defaults: TitleDefaults,
    ) -> Self {
        Self {
            stream: stream.map(|stream| {
                ActorSchemas::new(
                    &StreamPatch::protected(stream, defaults.stream),
                    |field, _| field.schema(),
                )
            }),
            session: session.map(|session| {
                ActorSchemas::new(
                    &SessionPatch::protected(session, defaults.session),
                    |field, actor| match (field, actor) {
                        (SessionField::Status, Actor::Automatic) => {
                            FieldSchema::Value(ValueSchema::choice(Status::REPORT_VARIANTS))
                        }
                        _ => field.schema(),
                    },
                )
            }),
        }
    }
}

#[cfg(test)]
mod tests;
