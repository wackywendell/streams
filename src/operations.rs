//! Domain-owned intent and pure reconciliation, independent of persistence.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use strum::{AsRefStr, Display, EnumString};

use crate::codec::{record_from_data, record_to_data};
use crate::model::{
    Catalog, Host, NamingMode, Record, RecordKey, Status, ValidationError, check_text,
};

/// Who supplied the intent. Automatic writes cannot change lifecycle or manual naming modes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, EnumString, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum Actor {
    User,
    Automatic,
}

/// Domain acceptance only; it says nothing about persistence or publication.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, EnumString, Display, AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum DomainOutcome {
    Changes,
    NoChange,
    Superseded,
    Conflict,
}

/// Effective ownership supplied from durable coordinator evidence, never from pending edits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Authority {
    pub host_id: String,
    pub token: String,
}

impl Authority {
    pub fn new(
        host_id: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, ValidationError> {
        let host_id = host_id.into();
        let token = token.into();
        check_text(&host_id, "authority.host_id", false)?;
        check_text(&token, "authority.token", false)?;
        Ok(Self { host_id, token })
    }
}

/// Drop edits to manual text at capture time; reconciliation still checks the live modes.
pub fn filter_automatic_changes(
    before: &Record,
    changes: &Map<String, Value>,
) -> Map<String, Value> {
    let mut protected = BTreeSet::new();
    match before {
        Record::Stream(s) => {
            if s.title_mode == NamingMode::Manual {
                protected.insert("title");
            }
            if s.goal_mode == NamingMode::Manual {
                protected.insert("goal");
                protected.insert("goal_short");
            }
        }
        Record::Session(s) => {
            if s.title_mode == NamingMode::Manual {
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
    let before_obj = record_to_data(before);
    if let Record::Stream(_) = before
        && let Some(new_goal) = out.get("goal")
        && before_obj.get("goal") != Some(new_goal)
    {
        out.entry("goal_short".to_string()).or_insert(Value::Null);
    }
    if let Record::Session(_) = before {
        let state_changed = ["state", "remaining", "status"].iter().any(|field| {
            out.get(*field)
                .is_some_and(|val| before_obj.get(*field) != Some(val))
        });
        if state_changed {
            out.entry("state_short".to_string()).or_insert(Value::Null);
        }
    }
    out
}

/// Captured intent against an original base and optional dependent records.
///
/// Construction validates records and actor permissions. Fields stay private so decoding
/// cannot bypass these checks; replay must not refresh the captured base or inputs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Operation {
    pub(crate) id: String,
    pub(crate) before: Option<Record>,
    pub(crate) after: Record,
    pub(crate) actor: Actor,
    pub(crate) ownership_token: Option<String>,
    pub(crate) inputs: Vec<Record>,
}

impl Operation {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn before(&self) -> Option<&Record> {
        self.before.as_ref()
    }
    pub fn after(&self) -> &Record {
        &self.after
    }
    pub fn actor(&self) -> Actor {
        self.actor
    }
    pub fn ownership_token(&self) -> Option<&str> {
        self.ownership_token.as_deref()
    }
    pub fn inputs(&self) -> &[Record] {
        &self.inputs
    }

    /// Reject identity/ownership moves and edits the actor is not allowed to make.
    /// A token records the captured ownership claim; reconciliation checks it against authority.
    pub fn new(
        id: impl Into<String>,
        before: Option<Record>,
        after: Record,
        actor: Actor,
        ownership_token: Option<String>,
        inputs: Vec<Record>,
    ) -> Result<Self, ValidationError> {
        let id = id.into();
        let after = after.validated()?;
        let before = before.map(Record::validated).transpose()?;
        let inputs = inputs
            .into_iter()
            .map(Record::validated)
            .collect::<Result<Vec<_>, _>>()?;
        check_text(&id, "operation.id", false)?;
        let mut keys = BTreeSet::new();
        for item in &inputs {
            if !keys.insert(item.key()) {
                return Err(ValidationError::new(
                    "operation.inputs contains duplicate record identities",
                ));
            }
        }
        let key = after.key();
        if keys.contains(&key) {
            return Err(ValidationError::new(
                "operation.inputs must not repeat the target record",
            ));
        }
        if let Some(ref b) = before {
            if b.key() != key {
                return Err(ValidationError::new(
                    "operations cannot change record identity",
                ));
            }
            if let (Record::Stream(bs), Record::Stream(as_)) = (b, &after)
                && as_.owner_host_id != bs.owner_host_id
            {
                return Err(ValidationError::new(
                    "handover requires a publication coordinator, not a generic edit",
                ));
            }
            if let (Record::Session(bs), Record::Session(as_)) = (b, &after) {
                if bs.stream_id != as_.stream_id {
                    return Err(ValidationError::new(
                        "generic edits cannot change session.stream_id",
                    ));
                }
                if bs.harness != as_.harness {
                    return Err(ValidationError::new(
                        "generic edits cannot change session.harness",
                    ));
                }
                if bs.harness_session_id != as_.harness_session_id {
                    return Err(ValidationError::new(
                        "generic edits cannot change session.harness_session_id",
                    ));
                }
            }
        }
        let needs_token = matches!(after, Record::Session(_))
            || (matches!(after, Record::Stream(_))
                && before.is_some()
                && actor == Actor::Automatic);
        if needs_token {
            let Some(ref tok) = ownership_token else {
                return Err(ValidationError::new(
                    "operation ownership_token must be a nonempty string",
                ));
            };
            check_text(tok, "operation ownership_token", false)?;
        } else if ownership_token.is_some() {
            return Err(ValidationError::new(
                "only session operations and stream inference carry an ownership token",
            ));
        }
        let op = Self {
            id,
            before,
            after,
            actor,
            ownership_token,
            inputs,
        };
        if op.actor == Actor::Automatic {
            if op.before.is_none() {
                if let Record::Stream(ref s) = op.after {
                    if s.status != Status::Active || s.archived {
                        return Err(ValidationError::new(
                            "automatic enrollment cannot set personal lifecycle",
                        ));
                    }
                    if s.title_mode != NamingMode::Automatic || s.goal_mode != NamingMode::Automatic
                    {
                        return Err(ValidationError::new(
                            "automatic creation cannot set manual modes",
                        ));
                    }
                }
                if let Record::Session(ref s) = op.after
                    && s.title_mode != NamingMode::Automatic
                {
                    return Err(ValidationError::new(
                        "automatic creation cannot set manual modes",
                    ));
                }
            } else {
                let changed = op.changed_fields();
                let mut prohibited: BTreeSet<&str> = ["title_mode", "goal_mode"].into();
                if matches!(op.after, Record::Stream(_)) {
                    prohibited.insert("status");
                    prohibited.insert("archived");
                }
                if changed.iter().any(|f| prohibited.contains(f)) {
                    return Err(ValidationError::new(
                        "automatic operations cannot change personal lifecycle or naming modes",
                    ));
                }
            }
        }
        Ok(op)
    }

    pub fn target(&self) -> RecordKey {
        self.after.key()
    }

    /// Logical domain field names, compared natively rather than through serialized values.
    pub fn changed_fields(&self) -> BTreeSet<&'static str> {
        let Some(ref before) = self.before else {
            return record_field_names(&self.after).iter().copied().collect();
        };
        record_field_names(&self.after)
            .iter()
            .copied()
            .filter(|name| field_eq(before, &self.after, name) == Some(false))
            .collect()
    }
}

/// Build validated intent from partial wire fields while retaining the caller's captured base.
pub fn update(
    id: impl Into<String>,
    before: &Record,
    changes: &Map<String, Value>,
    actor: Actor,
    ownership_token: Option<String>,
    inputs: Vec<Record>,
) -> Result<Operation, ValidationError> {
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
    let after = record_from_data(before.kind(), &Value::Object(wire))?;
    Operation::new(
        id,
        Some(before.clone()),
        after,
        actor,
        ownership_token,
        inputs,
    )
}

/// The domain decision, separate from whether a coordinator saved or published it.
/// `record` is present only for changes; refusals retain field names and a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reconciliation {
    pub operation_id: String,
    pub outcome: DomainOutcome,
    pub record: Option<Record>,
    pub conflicts: Vec<String>,
    pub reason: String,
}

impl Reconciliation {
    pub fn no_change(operation_id: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            outcome: DomainOutcome::NoChange,
            record: None,
            conflicts: Vec::new(),
            reason: reason.into(),
        }
    }
}

// Keep names, equality and copying together. Native equality preserves timestamp instant
// equality across offsets; logical owner_host_id is not the nested wire field owner.
macro_rules! record_fields {
    ($($variant:ident {
        immutable: [$($immutable:ident),*],
        editable: [$($editable:ident),* $(,)?]
    }),* $(,)?) => {
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
            title, goal, owner_host_id, title_mode, goal_short, goal_mode,
            status, archived, repositories, documents, issues, prs,
        ]
    },
    Session {
        immutable: [id, host_id, stream_id, harness, harness_session_id],
        editable: [title, title_mode, status, state, state_short, remaining, checkout]
    },
    Host {
        immutable: [id],
        editable: [label, checked_at]
    },
}

// A concurrent edit to any member invalidates intent captured from the whole text group.
fn guard_fields(operation: &Operation) -> BTreeSet<&'static str> {
    let mut fields = operation.changed_fields();
    let title_group = ["title", "title_mode"];
    if fields.iter().any(|f| title_group.contains(f)) {
        fields.extend(title_group);
    }
    match operation.after {
        Record::Stream(_) => {
            let goal_group = ["goal", "goal_short", "goal_mode"];
            if fields.iter().any(|f| goal_group.contains(f)) {
                fields.extend(goal_group);
            }
        }
        Record::Session(_) => {
            let state_group = ["state", "state_short", "remaining", "status"];
            if fields.iter().any(|f| state_group.contains(f)) {
                fields.extend(state_group);
            }
        }
        Record::Host(_) => {}
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

/// Reconcile captured intent against the current catalog without performing effects.
/// User edits may merge independent fields and additive collections. Stale automatic
/// descriptions are superseded instead of overwriting newer inputs or manual text.
pub fn reconcile(
    catalog: &Catalog,
    operation: &Operation,
    authority: Option<&Authority>,
) -> Reconciliation {
    let refuse = |reason: &str, conflicts: &[&str], superseded: bool| -> Reconciliation {
        let outcome = if superseded && operation.actor == Actor::Automatic {
            DomainOutcome::Superseded
        } else {
            DomainOutcome::Conflict
        };
        Reconciliation {
            operation_id: operation.id.clone(),
            outcome,
            record: None,
            conflicts: conflicts.iter().map(|s| (*s).to_string()).collect(),
            reason: reason.to_string(),
        }
    };

    for captured in &operation.inputs {
        if catalog.get(&captured.key()).as_ref() != Some(captured) {
            return refuse("captured catalog input changed", &["inputs"], true);
        }
    }
    let after = &operation.after;
    let current = catalog.get(&operation.target());
    if let Record::Session(s) = after
        && catalog.stream(&s.stream_id).is_none()
    {
        return refuse("session stream does not exist", &["stream_id"], false);
    }
    if let Some(ref required_token) = operation.ownership_token {
        let host_id = match after {
            Record::Session(s) => &s.host_id,
            Record::Stream(s) => &s.owner_host_id,
            Record::Host(h) => &h.id,
        };
        let valid_auth =
            authority.is_some_and(|auth| &auth.host_id == host_id && &auth.token == required_token);
        if !valid_auth {
            return refuse(
                "effective ownership changed or is unavailable",
                &["ownership"],
                true,
            );
        }
    }

    let proposed = match (&operation.before, current.as_ref()) {
        (None, Some(cur)) => {
            if cur == after {
                return Reconciliation::no_change(&operation.id, "");
            }
            return refuse("record already exists", &["identity"], false);
        }
        (None, None) => after.clone(),
        (Some(_), None) => {
            return refuse("update target does not exist", &["identity"], false);
        }
        (Some(before), Some(cur)) => {
            let changed = operation.changed_fields();
            if operation.actor == Actor::Automatic {
                match cur {
                    Record::Stream(cs) => {
                        if cs.title_mode == NamingMode::Manual && changed.contains("title") {
                            return refuse("manual text is protected", &["title_mode"], true);
                        }
                        if cs.goal_mode == NamingMode::Manual
                            && (changed.contains("goal") || changed.contains("goal_short"))
                        {
                            return refuse("manual text is protected", &["goal_mode"], true);
                        }
                    }
                    Record::Session(cs) => {
                        if cs.title_mode == NamingMode::Manual && changed.contains("title") {
                            return refuse("manual text is protected", &["title_mode"], true);
                        }
                    }
                    Record::Host(_) => {}
                }
                if cur == after {
                    return Reconciliation::no_change(&operation.id, "");
                }
                let inputs_changed = match (before, cur) {
                    (Record::Stream(before), Record::Stream(current)) => {
                        before.title != current.title
                            || before.title_mode != current.title_mode
                            || before.goal != current.goal
                            || before.goal_short != current.goal_short
                            || before.goal_mode != current.goal_mode
                    }
                    _ => cur != before,
                };
                if inputs_changed {
                    return refuse("generated description inputs changed", &["inputs"], true);
                }
            }

            // Concurrent mode change fences pending text even when normalization chose the same mode.
            match (before, cur, after) {
                (Record::Stream(bs), Record::Stream(cs), Record::Stream(as_)) => {
                    if cs.title_mode != bs.title_mode
                        && changed.contains("title")
                        && cs.title != as_.title
                    {
                        return refuse("text mode changed since capture", &["title_mode"], true);
                    }
                    if cs.goal_mode != bs.goal_mode
                        && ((changed.contains("goal") && cs.goal != as_.goal)
                            || (changed.contains("goal_short") && cs.goal_short != as_.goal_short))
                    {
                        return refuse("text mode changed since capture", &["goal_mode"], true);
                    }
                }
                (Record::Session(bs), Record::Session(cs), Record::Session(as_))
                    if cs.title_mode != bs.title_mode
                        && changed.contains("title")
                        && cs.title != as_.title =>
                {
                    return refuse("text mode changed since capture", &["title_mode"], true);
                }
                _ => {}
            }

            let mut candidate = cur.clone();
            let mut conflicts = Vec::new();
            for field in guard_fields(operation) {
                let actual_eq_base = field_eq(cur, before, field).unwrap_or(false);
                let actual_eq_desired = field_eq(cur, after, field).unwrap_or(false);
                if actual_eq_base || actual_eq_desired {
                    if changed.contains(field) {
                        copy_field(&mut candidate, after, field);
                    }
                    continue;
                }
                if operation.actor == Actor::User
                    && let (
                        Record::Stream(bs),
                        Record::Stream(as_),
                        Record::Stream(cs),
                        Record::Stream(cand),
                    ) = (before, after, cur, &mut candidate)
                {
                    match field {
                        "documents" => {
                            if let Some(merged) = merge_additive(
                                &bs.documents,
                                &as_.documents,
                                &cs.documents,
                                |a, b| a.identity() == b.identity(),
                            ) {
                                cand.documents = merged;
                                continue;
                            }
                        }
                        "issues" => {
                            if let Some(merged) =
                                merge_additive(&bs.issues, &as_.issues, &cs.issues, |a, b| {
                                    a.url == b.url
                                })
                            {
                                cand.issues = merged;
                                continue;
                            }
                        }
                        "prs" => {
                            if let Some(merged) =
                                merge_additive(&bs.prs, &as_.prs, &cs.prs, |a, b| a.url == b.url)
                            {
                                cand.prs = merged;
                                continue;
                            }
                        }
                        _ => {}
                    }
                }
                conflicts.push(field);
            }
            if !conflicts.is_empty() {
                return refuse(
                    "concurrent intent conflicts with captured base",
                    &conflicts,
                    false,
                );
            }
            let validated = match candidate {
                Record::Stream(s) => s.validated().map(Record::Stream),
                Record::Session(s) => s.validated().map(Record::Session),
                Record::Host(h) => Host::new(h.id, h.label, h.checked_at).map(Record::Host),
            };
            match validated {
                Ok(rec) => rec,
                Err(err) => return refuse(&err.to_string(), &["relationships"], false),
            }
        }
    };

    if let Err(err) = catalog.replacing(proposed.clone()) {
        return refuse(&err.to_string(), &["relationships"], false);
    }
    if current.as_ref() == Some(&proposed) {
        return Reconciliation::no_change(&operation.id, "");
    }
    Reconciliation {
        operation_id: operation.id.clone(),
        outcome: DomainOutcome::Changes,
        record: Some(proposed),
        conflicts: Vec::new(),
        reason: String::new(),
    }
}
