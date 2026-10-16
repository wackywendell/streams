"""Strict physical field mapping shared by JSON-compatible encoders, initially YAML.

Unknown fields are rejected. Logical validity remains in model constructors;
this mapping does not require other storage adapters to use the same field shape.
"""

from __future__ import annotations

from dataclasses import fields, is_dataclass
from datetime import datetime
from typing import Mapping

from .model import Checkout, Document, Host, Link, Record, Repository, Session, Stream, ValidationError


def _value(value: object) -> object:
    if isinstance(value, datetime):
        return value.isoformat()
    if is_dataclass(value):
        return {field.name: _value(getattr(value, field.name)) for field in fields(value)}
    if isinstance(value, Mapping):
        return {key: _value(item) for key, item in value.items()}
    if isinstance(value, tuple):
        return [_value(item) for item in value]
    return value


def record_to_data(record: Record) -> dict:
    data = _value(record)
    if not isinstance(data, dict):
        raise ValidationError("expected a record")
    if isinstance(record, Stream):
        data["owner"] = {"host_id": data.pop("owner_host_id")}
    return data


def object_fields(value: object, allowed: set[str]) -> dict:
    if not isinstance(value, dict) or any(not isinstance(key, str) for key in value):
        raise ValidationError("expected an object with string keys")
    unknown = set(value) - allowed
    if unknown:
        raise ValidationError(f"unknown fields: {', '.join(sorted(unknown))}")
    return dict(value)


def _construct(cls: type, value: object):
    data = object_fields(value, {field.name for field in fields(cls)})
    try:
        return cls(**data)
    except TypeError as error:
        raise ValidationError(f"invalid {cls.__name__}: {error}") from error


def record_from_data(kind: str, value: object) -> Record:
    if kind == "stream":
        allowed = {field.name for field in fields(Stream)} - {"owner_host_id"} | {"owner"}
        data = object_fields(value, allowed)
        owner = object_fields(data.pop("owner", None), {"host_id"})
        if "host_id" not in owner:
            raise ValidationError("missing owner.host_id")
        data["owner_host_id"] = owner["host_id"]
        if "repositories" in data:
            if not isinstance(data["repositories"], dict):
                raise ValidationError("repositories must be an object")
            data["repositories"] = {alias: _construct(Repository, repo) for alias, repo in data["repositories"].items()}
        for name, cls in (("documents", Document), ("issues", Link), ("prs", Link)):
            if name in data:
                if not isinstance(data[name], list):
                    raise ValidationError(f"{name} must be a list")
                data[name] = tuple(_construct(cls, item) for item in data[name])
        return _construct(Stream, data)
    if kind == "session":
        data = object_fields(value, {field.name for field in fields(Session)})
        if data.get("checkout") is not None:
            data["checkout"] = _construct(Checkout, data["checkout"])
        return _construct(Session, data)
    if kind == "host":
        data = object_fields(value, {field.name for field in fields(Host)})
        try:
            data["checked_at"] = datetime.fromisoformat(data["checked_at"])
        except (KeyError, TypeError, ValueError) as error:
            raise ValidationError("host.checked_at must be an ISO datetime") from error
        return _construct(Host, data)
    raise ValidationError(f"unknown record kind: {kind}")
