"""Pure card-proposal simulation over a supplied local context, never a live save.

Redacted tokens are opaque, snapshot-local fake capabilities. This module does
not attest native identity, infer directories, or model ownership handover.
"""

from __future__ import annotations

import json
from uuid import NAMESPACE_URL, uuid5

from .catalog_agent import _fields, enrollment_session, enrollment_stream, parse_enrollment, parse_update
from .catalog.codec import record_from_data, record_to_data
from .catalog.model import Catalog, Session, ValidationError, record_key, text
from .catalog.operations import Authority, Operation, reconcile
from .catalog.sync.envelope import load_json
from .catalog.sync.local import check_producer


def _snapshot(value):
    if not isinstance(value, dict):
        raise ValidationError("catalog must be a local context object")
    required = {"catalog_source", "host_id", "stream", "session", "sessions", "authority"}
    if not required <= value.keys():
        raise ValidationError("catalog is missing local context fields")
    for name in ("catalog_source", "host_id"):
        text(value[name], name)
    stream = None if value["stream"] is None else record_from_data("stream", value["stream"])
    session = None if value["session"] is None else record_from_data("session", value["session"])
    if not isinstance(value["sessions"], list):
        raise ValidationError("catalog.sessions must be a list")
    sessions = tuple(record_from_data("session", item) for item in value["sessions"])
    catalog = Catalog(streams=() if stream is None else (stream,), sessions=sessions)
    if session is not None and (session.host_id != value["host_id"] or catalog.get(record_key(session)) != session):
        raise ValidationError("catalog.session must match a local-host session in catalog.sessions")
    authority = value["authority"]
    if authority is not None:
        authority = _fields(authority, {"host_id", "token"}, {"host_id", "token"})
        authority = Authority(**authority)
        if stream is None:
            raise ValidationError("catalog authority requires a stream")
    return catalog, stream, session, authority


def _result(operation, catalog, authority, host_id):
    check_producer(operation, host_id)
    result = reconcile(catalog, operation, authority=authority)
    if result.outcome not in ("changes", "no-change"):
        raise ValidationError(result.reason)
    if result.record is not None:
        catalog = catalog.replacing(result.record)
    return catalog, {"simulated": True, "local": "unchanged", "sharing": "local-only",
                     "domain": {"operation_id": operation.id, "outcome": result.outcome,
                                "conflicts": list(result.conflicts), "reason": result.reason},
                     "detail": "Card proposal simulation only; nothing persisted or published"}


def preview(source: str):
    """Project one exact captured apply/enroll request without opening local state."""
    data = _fields(load_json(source), {"catalog", "action", "request"}, {"catalog", "action", "request"})
    context = data["catalog"]
    catalog, stream, session, authority = _snapshot(context)
    captured_sessions = catalog.sessions
    request = json.dumps(data["request"])
    if data["action"] == "apply":
        operation = parse_update(request, catalog_source=context["catalog_source"])
        target = session if isinstance(operation.before, Session) else stream
        if target is None or operation.before != target:
            raise ValidationError("captured target does not match supplied context")
        for captured in operation.inputs:
            if catalog.get(record_key(captured)) != captured:
                raise ValidationError("captured input does not match supplied context")
        catalog, saved = _result(operation, catalog, authority, context["host_id"])
        if isinstance(operation.after, Session):
            session = catalog.get(operation.target)
        else:
            stream = catalog.get(operation.target)
        receipt = {"simulated": True, "operation_id": operation.id, "save": saved,
                   "session" if isinstance(operation.after, Session) else "stream": record_to_data(catalog.get(operation.target))}
    elif data["action"] == "enroll":
        request_data, session_data, stream_data = parse_enrollment(request, catalog_source=context["catalog_source"])
        if request_data.get("stream_id") is not None and (stream is None or request_data["stream_id"] != stream.id):
            raise ValidationError("enrollment stream_id must match supplied context")
        identity = (context["host_id"], request_data["harness"], request_data["harness_session_id"])
        if session is not None and session.native_identity != identity:
            raise ValidationError("enrollment native identity does not match supplied session")
        if session is None and any(item.native_identity == identity for item in catalog.sessions):
            raise ValidationError("enrollment identity already exists outside catalog.session")
        saves = []
        if stream is None:
            if stream_data is None:
                raise ValidationError("new effort enrollment requires a stream title")
            stream_id = str(uuid5(NAMESPACE_URL, "streams:preview:effort:" + json.dumps([request_data["operation_id"], *identity])))
            stream = enrollment_stream(request_data, stream_data, host_id=context["host_id"], stream_id=stream_id)
            operation = Operation(request_data["operation_id"] + ":stream", None, stream, request_data["actor"])
            catalog, saved = _result(operation, catalog, None, context["host_id"])
            saves.append({"operation_id": operation.id, "save": saved})
            authority = Authority(context["host_id"], "[redacted-preview-authority]")
        elif request_data["actor"] == "user" and stream_data is not None:
            # Live user creation can only replay its own creation operation.
            if authority is None or authority.host_id != context["host_id"] or authority.token != request_data["operation_id"] + ":stream":
                raise ValidationError("effort already exists here; attach without a stream proposal")
            proposed = enrollment_stream(request_data, stream_data, host_id=context["host_id"], stream_id=stream.id)
            operation = Operation(request_data["operation_id"] + ":stream", None, proposed, "user")
            catalog, saved = _result(operation, catalog, authority, context["host_id"])
            saves.append({"operation_id": operation.id, "save": saved})
        if authority is None or authority.host_id != context["host_id"]:
            raise ValidationError("effective ownership on this host is required to attach a conversation")
        if session is None:
            session = enrollment_session(request_data, session_data, host_id=context["host_id"], stream_id=stream.id)
            operation = Operation(request_data["operation_id"] + ":session", None, session, request_data["actor"], ownership_token=authority.token)
            catalog, saved = _result(operation, catalog, authority, context["host_id"])
            saves.append({"operation_id": operation.id, "save": saved})
        receipt = {"simulated": True, "catalog_source": context["catalog_source"], "saves": saves,
                   "stream": record_to_data(stream), "session": record_to_data(session),
                   "authority": {"host_id": authority.host_id, "token": authority.token}}
        if not saves:
            receipt["publication"] = {"simulated": True, "local": "unchanged", "sharing": "local-only", "domain": None,
                                      "detail": "Simulation only; no publication attempted"}
    else:
        raise ValidationError("action must be apply or enroll")
    # Keep captured ordering, including every other conversation.
    sessions = [catalog.get(record_key(item)) for item in captured_sessions]
    if session is not None and session not in sessions:
        sessions.append(session)
    projected = {**context, "stream": None if stream is None else record_to_data(stream),
                 "session": None if session is None else record_to_data(session),
                 "sessions": [record_to_data(item) for item in sessions],
                 "authority": None if authority is None else {"host_id": authority.host_id, "token": authority.token}}
    return {"catalog": projected, "receipt": receipt}
