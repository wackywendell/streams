"""Local integration boundary for harnesses; catalog persistence stays in domain operations.

A context is a pre-inference capture, not a projected read. Applying an update
keeps its supplied base, inputs and identity unchanged; it never fetches a newer
base to make generated intent applicable. Actor expresses approved intent, not
an authentication boundary. Callers must fence their own conversation lineage.
"""

from __future__ import annotations

from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
from uuid import NAMESPACE_URL, uuid5

from .catalog.codec import object_fields, record_from_data, record_to_data
from .catalog.model import Session, Stream, ValidationError, choice, text
from .catalog.operations import Operation, filter_automatic_changes, normalize_changes, update
from .catalog.storage import StorageError
from .catalog.sync.envelope import load_json
from .errors import StreamError


EDITABLE = {
    "stream": {"title", "title_mode", "goal", "goal_short", "goal_mode", "status", "archived", "documents", "issues", "prs"},
    "session": {"title", "title_mode", "status", "state", "state_short", "remaining"},
}
AUTOMATIC = {
    "stream": {"title", "goal", "goal_short"},
    "session": {"title", "status", "state", "state_short", "remaining"},
}


def _fields(value, allowed, required=()):
    data = object_fields(value, set(allowed))
    missing = set(required) - data.keys()
    if missing:
        raise ValidationError(f"missing fields: {', '.join(sorted(missing))}")
    return data


def _identity(harness, native_id):
    choice(harness, "harness", ("pi", "codex", "claude"))
    text(native_id, "harness_session_id")


def source_id(backend) -> str:
    """Fence host-local source selection, not the shared logical catalog identity."""
    root = backend.store.root.resolve()
    stat = root.stat()
    return hashlib.sha256(json.dumps([str(root), stat.st_dev, stat.st_ino, backend.host_id]).encode()).hexdigest()


def _check_source(data, expected):
    text(data["catalog_source"], "catalog_source")
    if data["catalog_source"] != expected:
        raise StreamError("catalog source changed since capture; inspect the current catalog before making a new request")


def session_id_for(host_id: str, harness: str, native_id: str) -> str:
    return str(uuid5(NAMESPACE_URL, "streams:session:" + json.dumps([host_id, harness, native_id])))


def context(backend, *, harness, native_id, stream_id=None, workspace_stream_id=None):
    """Read only actual local records; identifying a stream does not attach a session."""
    _identity(harness, native_id)
    explicit = os.environ.get("STREAMS_STREAM_ID")
    stream, session, sessions, authority = backend.context_records(
        harness=harness,
        native_id=native_id,
        stream_id=stream_id,
        explicit_stream_id=explicit,
        workspace_stream_id=workspace_stream_id,
        deterministic_id=session_id_for(backend.host_id, harness, native_id),
    )
    return {
        "catalog_source": source_id(backend),
        "host_id": backend.host_id,
        "stream": stream,
        "session": session,
        "sessions": sessions,
        "authority": authority,
    }


def parse_update(source: str, *, catalog_source: str) -> Operation:
    """Build one exact, allowlisted update from a captured record and dirty fields."""
    data = _fields(load_json(source), {"operation_id", "catalog_source", "kind", "actor", "before", "changes", "ownership_token", "inputs"},
                   {"operation_id", "catalog_source", "kind", "actor", "before", "changes"})
    _check_source(data, catalog_source)
    kind, actor = data["kind"], data["actor"]
    choice(kind, "kind", ("stream", "session"))
    choice(actor, "actor", ("automatic", "user"))
    before = record_from_data(kind, data["before"])
    changes = _fields(data["changes"], AUTOMATIC[kind] if actor == "automatic" else EDITABLE[kind])
    if actor == "automatic":
        changes = filter_automatic_changes(before, changes)
    changes = normalize_changes(before, changes, actor=actor)
    # Normalize before the codec constructs a record so clearing a goal cannot
    # leave an invalid intermediate goal_short. References stay codec-owned.
    wire = {**record_to_data(before), **changes}
    desired = record_from_data(kind, wire)
    changes = {name: getattr(desired, name) for name in changes}
    inputs = data.get("inputs", [])
    if not isinstance(inputs, list):
        raise ValidationError("inputs must be a list")
    captured = []
    for item in inputs:
        item = _fields(item, {"kind", "record"}, {"kind", "record"})
        captured.append(record_from_data(item["kind"], item["record"]))
    return update(data["operation_id"], before, changes, actor=actor,
                  ownership_token=data.get("ownership_token"), inputs=tuple(captured))


@contextmanager
def _conversation_lock(runtime, harness, native_id):
    key = hashlib.sha256(json.dumps([harness, native_id]).encode()).hexdigest()
    path = runtime.state_dir / "agent-locks" / (key + ".lock")
    path.parent.mkdir(parents=True, exist_ok=True)
    descriptor = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def parse_enrollment(source: str, *, catalog_source: str):
    """Construct validated enrollment card inputs without consulting runtime state."""
    data = _fields(load_json(source), {"operation_id", "catalog_source", "harness", "harness_session_id", "stream_id", "stream", "session", "actor", "cwd"},
                   {"operation_id", "catalog_source", "harness", "harness_session_id", "session", "actor", "cwd"})
    _check_source(data, catalog_source)
    _identity(data["harness"], data["harness_session_id"])
    text(data["operation_id"], "operation_id")
    choice(data["actor"], "actor", ("automatic", "user"))
    session_data = _fields(data["session"], {"title"}, {"title"})
    text(session_data["title"], "session.title")
    if data.get("stream_id") is not None:
        text(data["stream_id"], "stream_id")
    stream_data = None if data.get("stream") is None else _fields(data["stream"], {"title", "goal", "goal_short"}, {"title"})
    if stream_data is not None:
        stream_data = {"goal": None, **stream_data}
    text(data["cwd"], "cwd")
    return data, session_data, stream_data


def enrollment_stream(data, stream_data, *, host_id, stream_id):
    return Stream(id=stream_id, owner_host_id=host_id,
                  title_mode="manual" if data["actor"] == "user" else "automatic",
                  goal_mode="manual" if data["actor"] == "user" and stream_data.get("goal") is not None else "automatic", **stream_data)


def enrollment_session(data, session_data, *, host_id, stream_id):
    return Session(id=session_id_for(host_id, data["harness"], data["harness_session_id"]), host_id=host_id,
                   stream_id=stream_id, harness=data["harness"], harness_session_id=data["harness_session_id"],
                   title=session_data["title"], title_mode="manual" if data["actor"] == "user" else "automatic")


def enroll(backend, runtime, source: str):
    """Enroll the caller without launching native resources or rebinding a conversation.

    Related saves have distinct stable receipts and are not a cross-record
    transaction. A partial save retains its receipt; the caller must inspect
    context rather than reinterpret a failed request as fresh intent.
    """
    data, session_data, stream_data = parse_enrollment(source, catalog_source=source_id(backend))
    directory = Path(data["cwd"]).expanduser().resolve(strict=True)
    if not directory.is_dir():
        raise StreamError("enrollment cwd must be an existing directory")
    identity = json.dumps([backend.host_id, data["harness"], data["harness_session_id"]])
    effort_id = str(uuid5(NAMESPACE_URL, "streams:effort:" + identity))
    payload = {"catalog_source": source_id(backend), "saves": []}

    def new_stream(stream_id):
        return enrollment_stream(data, stream_data, host_id=backend.host_id, stream_id=stream_id)

    def persist(operation):
        saved = backend.save(operation, origin={"command": "streams-agent"})
        payload["saves"].append({"operation_id": operation.id, "save": saved})
        if not saved.accepted:
            raise StreamError("enrollment operation was not accepted; inspect its save receipt")

    if data["actor"] == "automatic":
        runtime.verify_automatic_enrollment(data["harness"], data["harness_session_id"])

    try:
        with _conversation_lock(runtime, data["harness"], data["harness_session_id"]), runtime.enrollment() as scope:
            current = context(backend, harness=data["harness"], native_id=data["harness_session_id"], stream_id=data.get("stream_id"))
            stream, session = current["stream"], current["session"]
            if stream is not None and scope.stream_id is not None and stream.id != scope.stream_id:
                raise StreamError("calling workspace has a different effort; moving requires an explicit move or fork operation")
            selected_id = stream.id if stream else scope.stream_id or effort_id
            if stream is None:
                stream = backend.snapshot().catalog.stream(selected_id)
            payload.update(stream=stream, session=session, authority=current["authority"])
            if data["actor"] == "user" and stream_data is not None and stream is not None:
                authority = backend.authority(stream.id)
                if authority is None or authority.host_id != backend.host_id or authority.token != data["operation_id"] + ":stream":
                    raise StreamError("effort already exists here; attach to it or create work in another workspace")
                # Replaying this exact creation obtains its original receipt. A
                # reused ID with changed intent is rejected by the coordinator.
                persist(Operation(data["operation_id"] + ":stream", None, new_stream(stream.id), "user"))
            if stream is None:
                if stream_data is None:
                    raise StreamError("new effort enrollment requires a stream title")
                stream = new_stream(selected_id)
                scope.claim(stream.id)
                persist(Operation(data["operation_id"] + ":stream", None, stream, data["actor"]))
                payload["stream"] = stream
            authority = backend.authority(stream.id)
            if authority is None or authority.host_id != backend.host_id:
                raise StreamError("effective ownership on this host is required to attach a conversation")
            scope.claim(stream.id)
            if session is None:
                session = enrollment_session(data, session_data, host_id=backend.host_id, stream_id=stream.id)
                persist(Operation(data["operation_id"] + ":session", None, session, data["actor"], ownership_token=authority.token))
            payload.update(stream=stream, session=session, authority=authority)
            runtime.remember_directory(stream.id, directory, if_missing=True)
            binding = runtime.remember_directory(stream.id, directory, session_id=session.id, if_missing=True)
            if binding.checkout_id is not None:
                runtime.checkouts.bind_inferred_repositories(binding.checkout_id)
            scope.bind(stream, authority=authority)
    except (StreamError, StorageError, OSError, ValueError) as error:
        if not payload["saves"] and payload.get("session") is None:
            raise
        payload["completion_error"] = {"message": str(error), "detail": "enrollment incomplete; inspect saved records and receipts before retrying"}
        return payload, 2
    return payload, 0
