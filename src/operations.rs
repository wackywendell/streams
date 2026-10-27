//! Domain-owned intent and pure reconciliation, independent of persistence.

pub mod edits;
pub mod membership;

use std::collections::BTreeSet;

use serde::Serialize;
use serde_json::{Map, Value};
use strum::{AsRefStr, Display, EnumString, VariantNames};

use crate::codec::{record_from_data, record_to_data};
use crate::model::{NamingMode, Record, RecordKind, ValidationError};

/// Who supplied the intent. Automatic writes cannot change lifecycle or manual naming modes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Actor {
    User,
    Automatic,
    /// Library-only verified native membership transitions, never model-generated intent.
    Native,
}

/// Domain acceptance only; it says nothing about persistence or publication.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, EnumString, VariantNames, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum DomainOutcome {
    Changes,
    NoChange,
    Superseded,
    Conflict,
}

crate::codec::deserialize_string_enum!(Actor, DomainOutcome);

/// Drop edits to manual text at capture time; reconciliation still checks the live modes.
pub fn filter_automatic_changes(
    before: &Record,
    changes: &Map<String, Value>,
    title_default: NamingMode,
) -> Map<String, Value> {
    let mut protected = BTreeSet::new();
    match before {
        Record::Stream(s) => {
            if s.title_mode.resolve(title_default) == NamingMode::Manual {
                protected.insert("title");
            }
            if s.goal_mode == NamingMode::Manual {
                protected.insert("goal");
                protected.insert("goal_short");
            }
        }
        Record::Session(s) => {
            if s.title_mode.resolve(title_default) == NamingMode::Manual {
                protected.insert("title");
            }
        }
        Record::Host(_) => {}
    }
    changes
        .iter()
        .filter(|(k, _)| !protected.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Treat user text edits as manual by default and invalidate dependent short summaries.
/// An explicitly supplied mode or summary is preserved rather than silently replaced.
pub fn normalize_changes(
    before: &Record,
    changes: &Map<String, Value>,
    actor: Actor,
) -> Map<String, Value> {
    let mut out = changes.clone();
    if actor == Actor::User {
        match before {
            Record::Stream(_) => {
                if out.contains_key("title") {
                    out.entry("title_mode".to_string())
                        .or_insert_with(|| Value::from(NamingMode::Manual.as_ref()));
                }
                if out.contains_key("goal") {
                    out.entry("goal_mode".to_string())
                        .or_insert_with(|| Value::from(NamingMode::Manual.as_ref()));
                }
            }
            Record::Session(_) => {
                if out.contains_key("title") {
                    out.entry("title_mode".to_string())
                        .or_insert_with(|| Value::from(NamingMode::Manual.as_ref()));
                }
            }
            Record::Host(_) => {}
        }
    }
    if let Record::Stream(stream) = before
        && let Some(new_goal) = out.get("goal")
        && match (new_goal, stream.goal.as_deref()) {
            (Value::Null, None) => false,
            (Value::String(goal), Some(before)) => goal != before,
            _ => true,
        }
    {
        out.entry("goal_short".to_string()).or_insert(Value::Null);
    }
    if let Record::Session(session) = before {
        for (field, current, short) in [
            ("recap", session.recap.as_str(), "recap_short"),
            ("state", session.state.as_str(), "state_short"),
            ("remaining", session.remaining.as_str(), "remaining_short"),
        ] {
            if out
                .get(field)
                .is_some_and(|value| value.as_str() != Some(current))
            {
                out.entry(short.to_string()).or_insert(Value::Null);
            }
        }
    }
    out
}

/// Normalize partial fields and validate the resulting record, without capturing intent.
pub fn apply_changes(
    before: &Record,
    changes: &Map<String, Value>,
    actor: Actor,
) -> Result<Record, ValidationError> {
    let normalized = normalize_changes(before, changes, actor);
    let mut wire = record_to_data(before)
        .as_object()
        .cloned()
        .unwrap_or_default();
    for (k, v) in normalized {
        if k == "owner_host_id" && matches!(before, Record::Stream(_)) {
            let mut owner = Map::new();
            owner.insert("host_id".to_string(), v);
            wire.insert("owner".to_string(), Value::Object(owner));
        } else {
            wire.insert(k, v);
        }
    }
    record_from_data(before.kind(), &Value::Object(wire))
}

/// The domain decision, separate from whether a coordinator saved or published it.
/// Only changes carry a record; refusals retain field names and a reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciliation {
    Changes {
        operation_id: String,
        record: Box<Record>,
    },
    NoChange {
        operation_id: String,
        reason: String,
    },
    Superseded {
        operation_id: String,
        conflicts: Vec<String>,
        reason: String,
    },
    Conflict {
        operation_id: String,
        conflicts: Vec<String>,
        reason: String,
    },
}

impl Reconciliation {
    pub fn no_change(operation_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::NoChange {
            operation_id: operation_id.into(),
            reason: reason.into(),
        }
    }

    pub fn operation_id(&self) -> &str {
        match self {
            Self::Changes { operation_id, .. }
            | Self::NoChange { operation_id, .. }
            | Self::Superseded { operation_id, .. }
            | Self::Conflict { operation_id, .. } => operation_id,
        }
    }

    pub fn outcome(&self) -> DomainOutcome {
        match self {
            Self::Changes { .. } => DomainOutcome::Changes,
            Self::NoChange { .. } => DomainOutcome::NoChange,
            Self::Superseded { .. } => DomainOutcome::Superseded,
            Self::Conflict { .. } => DomainOutcome::Conflict,
        }
    }

    pub fn conflicts(&self) -> &[String] {
        match self {
            Self::Superseded { conflicts, .. } | Self::Conflict { conflicts, .. } => conflicts,
            Self::Changes { .. } | Self::NoChange { .. } => &[],
        }
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::NoChange { reason, .. }
            | Self::Superseded { reason, .. }
            | Self::Conflict { reason, .. } => reason,
            Self::Changes { .. } => "",
        }
    }
}

impl Serialize for Reconciliation {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Receipt<'a> {
            operation_id: &'a str,
            outcome: DomainOutcome,
            record: Option<&'a Record>,
            conflicts: &'a [String],
            reason: &'a str,
        }
        Receipt {
            operation_id: self.operation_id(),
            outcome: self.outcome(),
            record: match self {
                Self::Changes { record, .. } => Some(record),
                _ => None,
            },
            conflicts: self.conflicts(),
            reason: self.reason(),
        }
        .serialize(serializer)
    }
}

enum AgentAccess {
    None,
    User,
    Automatic,
}

struct FieldPolicy {
    automatic: bool,
    agent: AgentAccess,
}

pub(crate) fn agent_field_allowed(kind: RecordKind, actor: Actor, name: &str) -> bool {
    if actor == Actor::Native {
        return false;
    }
    field_policy(kind, name).is_some_and(|policy| match policy.agent {
        AgentAccess::None => false,
        AgentAccess::User => actor == Actor::User,
        AgentAccess::Automatic => true,
    })
}

// Keep names, permissions, equality and copying together. Native equality preserves timestamp
// instant equality across offsets; logical owner_host_id is not the nested wire field owner.
macro_rules! record_fields {
    ($($variant:ident {
        immutable: [$($immutable:ident),*],
        editable: [$($editable:ident: ($automatic:literal, $agent:ident)),* $(,)?]
    }),* $(,)?) => {
        fn field_policy(kind: RecordKind, name: &str) -> Option<FieldPolicy> {
            match kind {
                $(RecordKind::$variant => match name {
                    $(stringify!($editable) => Some(FieldPolicy {
                        automatic: $automatic,
                        agent: AgentAccess::$agent,
                    }),)*
                    _ => None,
                },)*
            }
        }

        fn record_field_names(record: &Record) -> &'static [&'static str] {
            match record {
                $(Record::$variant(_) => &[
                    $(stringify!($immutable),)*
                    $(stringify!($editable),)*
                ],)*
            }
        }

        fn field_eq(a: &Record, b: &Record, field: &str) -> Option<bool> {
            match (a, b) {
                $((Record::$variant(a), Record::$variant(b)) => match field {
                    $(stringify!($immutable) => Some(a.$immutable == b.$immutable),)*
                    $(stringify!($editable) => Some(a.$editable == b.$editable),)*
                    _ => None,
                },)*
                _ => None,
            }
        }

        fn copy_field(target: &mut Record, source: &Record, field: &str) {
            match (target, source) {
                $((Record::$variant(t), Record::$variant(s)) => match field {
                    $(stringify!($editable) => t.$editable = s.$editable.clone(),)*
                    _ => {}
                },)*
                _ => {}
            }
        }
    };
}

record_fields! {
    Stream {
        immutable: [id],
        editable: [
            title: (true, Automatic),
            goal: (true, Automatic),
            owner_host_id: (true, None),
            title_mode: (false, User),
            goal_short: (true, Automatic),
            goal_mode: (false, User),
            status: (false, User),
            archived: (false, User),
            repositories: (true, None),
            documents: (true, User),
            issues: (true, User),
            prs: (true, User),
        ]
    },
    Session {
        immutable: [id, host_id, harness, harness_session_id],
        editable: [
            stream_id: (false, None),
            previous_stream_ids: (false, None),
            title: (true, Automatic),
            title_mode: (false, User),
            status: (true, Automatic),
            recap: (true, Automatic),
            recap_short: (true, Automatic),
            state: (true, Automatic),
            state_short: (true, Automatic),
            remaining: (true, Automatic),
            remaining_short: (true, Automatic),
            checkout: (true, None),
        ]
    },
    Host {
        immutable: [id],
        editable: [label: (true, None), checked_at: (true, None)]
    },
}

const TITLE_GROUP: [&str; 2] = ["title", "title_mode"];
const STREAM_GOAL_GROUP: [&str; 3] = ["goal", "goal_short", "goal_mode"];

// A concurrent edit to any member invalidates intent captured from the whole text group.
fn text_group_guards(
    kind: RecordKind,
    mut fields: BTreeSet<&'static str>,
) -> BTreeSet<&'static str> {
    if fields.iter().any(|f| TITLE_GROUP.contains(f)) {
        fields.extend(TITLE_GROUP);
    }
    match kind {
        RecordKind::Stream => {
            if fields.iter().any(|f| STREAM_GOAL_GROUP.contains(f)) {
                fields.extend(STREAM_GOAL_GROUP);
            }
        }
        RecordKind::Session => {
            let state_group = [
                "recap",
                "recap_short",
                "state",
                "state_short",
                "remaining",
                "remaining_short",
                "status",
            ];
            if fields.iter().any(|f| state_group.contains(f)) {
                fields.extend(state_group);
            }
        }
        RecordKind::Host => {}
    }
    fields
}

fn merge_additive<T: Clone + PartialEq>(
    before: &[T],
    desired: &[T],
    current: &[T],
    same_identity: impl Fn(&T, &T) -> bool,
) -> Option<Vec<T>> {
    if before
        .iter()
        .any(|item| !desired.contains(item) || !current.contains(item))
    {
        return None;
    }
    let mut merged = current.to_vec();
    for item in desired {
        if let Some(old) = merged.iter().find(|old| same_identity(old, item)) {
            if old != item {
                return None;
            }
        } else {
            merged.push(item.clone());
        }
    }
    Some(merged)
}

#[cfg(test)]
mod field_policy_tests {
    use super::*;

    #[test]
    fn backend_and_agent_capabilities_remain_distinct() {
        for (kind, names, automatic, user_agent, automatic_agent) in [
            (
                RecordKind::Stream,
                "title goal goal_short",
                true,
                true,
                true,
            ),
            (
                RecordKind::Stream,
                "title_mode goal_mode status archived",
                false,
                true,
                false,
            ),
            (
                RecordKind::Stream,
                "documents issues prs",
                true,
                true,
                false,
            ),
            (
                RecordKind::Stream,
                "owner_host_id repositories",
                true,
                false,
                false,
            ),
            (
                RecordKind::Session,
                "title status recap recap_short state state_short remaining remaining_short",
                true,
                true,
                true,
            ),
            (RecordKind::Session, "title_mode", false, true, false),
            (RecordKind::Session, "checkout", true, false, false),
            (
                RecordKind::Session,
                "stream_id previous_stream_ids",
                false,
                false,
                false,
            ),
            (RecordKind::Host, "label checked_at", true, false, false),
        ] {
            for name in names.split_whitespace() {
                assert_eq!(field_policy(kind, name).unwrap().automatic, automatic);
                assert!(!agent_field_allowed(kind, Actor::Native, name));
                assert_eq!(agent_field_allowed(kind, Actor::User, name), user_agent);
                assert_eq!(
                    agent_field_allowed(kind, Actor::Automatic, name),
                    automatic_agent,
                );
            }
        }
        for (kind, names) in [
            (RecordKind::Stream, "id unknown owner"),
            (
                RecordKind::Session,
                "id host_id harness harness_session_id unknown",
            ),
            (RecordKind::Host, "id unknown"),
        ] {
            for name in names.split_whitespace() {
                assert!(field_policy(kind, name).is_none());
                for actor in [Actor::User, Actor::Automatic] {
                    assert!(!agent_field_allowed(kind, actor, name));
                }
            }
        }
    }
}
