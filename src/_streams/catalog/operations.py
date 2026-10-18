"""Domain-owned intent and pure reconciliation, independent of persistence."""

from __future__ import annotations

from dataclasses import dataclass, fields, replace
from typing import Literal, Mapping

from .model import Catalog, Document, Host, Record, RecordKey, Session, Stream, ValidationError, choice, record_key, text

Actor = Literal["user", "automatic"]
Outcome = Literal["changes", "no-change", "superseded", "conflict"]


@dataclass(frozen=True)
class Authority:
    """Effective ownership supplied by the coordinator, never inferred from pending edits.

    Tokens identify an ownership period, including a return to the same host.
    The local-only coordinator also supplies one; no Git identity is required.
    """

    host_id: str
    token: str

    def __post_init__(self) -> None:
        text(self.host_id, "authority.host_id")
        text(self.token, "authority.token")


@dataclass(frozen=True)
class Operation:
    """One exact-target creation or edit, retaining its original base and intent on replay.

    before=None means creation. Retain the original ID, base and desired record
    on retries; do not reconstruct intent from the persisted result. Use update()
    to edit records so dependent compact descriptions are cleared.

    actor describes approved intent, not authentication of an untrusted caller.
    Integrations must establish user direction before setting actor='user'.
    Generic edits cannot hand over ownership, reattach or change native identity.
    """

    id: str
    before: Record | None
    after: Record
    actor: Actor
    ownership_token: str | None = None
    inputs: tuple[Record, ...] = ()

    def __post_init__(self) -> None:
        text(self.id, "operation.id")
        if not isinstance(self.inputs, tuple) or any(not isinstance(item, (Stream, Session, Host)) for item in self.inputs):
            raise ValidationError("operation.inputs must be a tuple of logical records")
        keys = [record_key(item) for item in self.inputs]
        if len(keys) != len(set(keys)):
            raise ValidationError("operation.inputs contains duplicate record identities")
        choice(self.actor, "operation.actor", ("user", "automatic"))
        key = record_key(self.after)
        if key in keys:
            raise ValidationError("operation.inputs must not repeat the target record")
        if self.before is not None:
            if record_key(self.before) != key:
                raise ValidationError("operations cannot change record identity")
            if isinstance(self.after, Stream) and self.after.owner_host_id != self.before.owner_host_id:
                raise ValidationError("handover requires a publication coordinator, not a generic edit")
            if isinstance(self.after, Session):
                for name in ("stream_id", "harness", "harness_session_id"):
                    if getattr(self.before, name) != getattr(self.after, name):
                        raise ValidationError(f"generic edits cannot change session.{name}")
        if isinstance(self.after, Session):
            text(self.ownership_token, "session operation ownership_token")
        elif self.ownership_token is not None:
            raise ValidationError("only session operations carry an ownership token")
        if self.actor == "automatic":
            if self.before is None:
                if isinstance(self.after, Stream) and (self.after.status != "active" or self.after.archived):
                    raise ValidationError("automatic enrollment cannot set personal lifecycle")
                for name in ("title_mode", "goal_mode"):
                    if hasattr(self.after, name) and getattr(self.after, name) != "automatic":
                        raise ValidationError("automatic creation cannot set manual modes")
            else:
                prohibited = {"title_mode", "goal_mode"}
                if isinstance(self.after, Stream):
                    prohibited |= {"status", "archived"}
                if self.changed_fields & prohibited:
                    raise ValidationError("automatic operations cannot change personal lifecycle or naming modes")

    @property
    def target(self) -> RecordKey:
        return record_key(self.after)

    @property
    def changed_fields(self) -> set[str]:
        return {field.name for field in fields(self.after)
                if self.before is None or getattr(self.before, field.name) != getattr(self.after, field.name)}


def update(id: str, before: Record, changes: Mapping[str, object], *, actor: Actor = "user",
           ownership_token: str | None = None, inputs: tuple[Record, ...] = ()) -> Operation:
    """Capture an edit's base, clearing display derivatives unless explicitly replaced.

    A changed goal invalidates goal_short. Changed state, remaining work or status
    invalidates state_short. Supply a new derivative in changes to retain one.
    """
    changes = dict(changes)
    if isinstance(before, Stream) and "goal" in changes and changes["goal"] != before.goal:
        changes.setdefault("goal_short", None)
    if isinstance(before, Session) and any(name in changes and changes[name] != getattr(before, name)
                                           for name in ("state", "remaining", "status")):
        changes.setdefault("state_short", None)
    try:
        after = replace(before, **changes)
    except TypeError as error:
        raise ValidationError(str(error)) from error
    return Operation(id=id, before=before, after=after, actor=actor, ownership_token=ownership_token, inputs=inputs)


@dataclass(frozen=True)
class Reconciliation:
    operation_id: str
    outcome: Outcome
    record: Record | None = None
    conflicts: tuple[str, ...] = ()
    reason: str = ""


def _guard_fields(operation: Operation) -> set[str]:
    changed = operation.changed_fields
    groups = [{"title", "title_mode"}]
    if isinstance(operation.after, Stream):
        groups.append({"goal", "goal_short", "goal_mode"})
    if isinstance(operation.after, Session):
        groups.append({"state", "state_short", "remaining", "status"})
    return changed | set().union(*(group for group in groups if changed & group))


def _merge_additions(before: tuple, desired: tuple, current: tuple) -> tuple | None:
    # Only additive intent is mergeable. Removal or editing must match its base.
    if any(item not in desired or item not in current for item in before):
        return None
    merged = list(current)
    for item in desired:
        identity = item.identity if isinstance(item, Document) else item.url
        match = next((old for old in merged
                      if (old.identity if isinstance(old, Document) else old.url) == identity), None)
        if match is not None and match != item:
            return None
        if match is None:
            merged.append(item)
    return tuple(merged)


def reconcile(catalog: Catalog, operation: Operation, *, authority: Authority | None = None) -> Reconciliation:
    """Return a proposed record or a structured refusal without changing catalog.

    Outcomes are changes (a validated replacement), no-change (already satisfied),
    superseded (automatic inputs or authority expired), or conflict (unsafe intent
    or relationships). No refusal silently reinterprets the original intent.

    User edits combine at independent fields; descriptions and naming modes are
    guarded together. Reference additions combine by identity, but concurrent edits
    and removals conflict. Automatic inference guards the entire captured target
    record and any supplied logical input records. Integrations must separately
    fence ephemeral conversation or document inputs; a fresh persistence snapshot
    does not verify evidence gathered before inference.

    The coordinator supplies effective session Authority, not a pending owner field,
    persists accepted changes against this snapshot and retains operation identity.
    """
    def refuse(reason: str, conflicts: tuple[str, ...] = (), *, superseded: bool = False) -> Reconciliation:
        outcome = "superseded" if superseded and operation.actor == "automatic" else "conflict"
        return Reconciliation(operation.id, outcome, conflicts=conflicts, reason=reason)

    for captured in operation.inputs:
        if catalog.get(record_key(captured)) != captured:
            return refuse("captured catalog input changed", ("inputs",), superseded=True)
    after, before = operation.after, operation.before
    current = catalog.get(operation.target)
    if isinstance(after, Session):
        if catalog.stream(after.stream_id) is None:
            return refuse("session stream does not exist", ("stream_id",))
        if authority is None or authority.host_id != after.host_id or authority.token != operation.ownership_token:
            return refuse("effective ownership changed or is unavailable", ("ownership",), superseded=True)
    if before is None:
        if current is not None:
            if current == after:
                return Reconciliation(operation.id, "no-change")
            return refuse("record already exists", ("identity",))
        proposed = after
    else:
        if current is None:
            return refuse("update target does not exist", ("identity",))
        if operation.actor == "automatic":
            for mode, names in (("title_mode", {"title"}), ("goal_mode", {"goal", "goal_short"})):
                if getattr(current, mode, None) == "manual" and operation.changed_fields & names:
                    return refuse("manual text is protected", (mode,), superseded=True)
            if current == after:
                return Reconciliation(operation.id, "no-change")
            if current != before:
                return refuse("generated description inputs changed", ("inputs",), superseded=True)
        changes = {}
        conflicts = []
        for name in sorted(_guard_fields(operation)):
            base_value, desired, actual = getattr(before, name), getattr(after, name), getattr(current, name)
            if actual == base_value or actual == desired:
                if name in operation.changed_fields:
                    changes[name] = desired
                continue
            if name in {"documents", "issues", "prs"} and operation.actor == "user":
                merged = _merge_additions(base_value, desired, actual)
                if merged is not None:
                    changes[name] = merged
                    continue
            conflicts.append(name)
        if conflicts:
            return refuse("concurrent intent conflicts with captured base", tuple(conflicts))
        try:
            proposed = replace(current, **changes)
        except ValidationError as error:
            return refuse(str(error), ("relationships",))
    try:
        catalog.replacing(proposed)
    except ValidationError as error:
        return refuse(str(error), ("relationships",))
    if proposed == current:
        return Reconciliation(operation.id, "no-change")
    return Reconciliation(operation.id, "changes", record=proposed)
