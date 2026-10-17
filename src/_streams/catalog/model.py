"""Immutable logical records and relationships; no serialization or runtime bindings.

Constructors validate logical fields and own their immutable collections. Storage
adapters must construct these records rather than replace their validation rules.
"""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import datetime
from pathlib import PurePosixPath, PureWindowsPath
from types import MappingProxyType
from typing import Literal, Mapping

Status = Literal["active", "waiting", "blocked", "parked", "done"]
NamingMode = Literal["automatic", "manual"]
STATUSES = ("active", "waiting", "blocked", "parked", "done")
NAMING_MODES = ("automatic", "manual")


class ValidationError(ValueError):
    """Invalid logical record or operation."""


def text(value: object, name: str, *, empty: bool = False) -> None:
    if not isinstance(value, str) or "\x00" in value or (not empty and not value.strip()):
        raise ValidationError(f"{name} must be {'a string' if empty else 'a nonempty string'}")


def choice(value: object, name: str, options: tuple[str, ...]) -> None:
    if value not in options:
        raise ValidationError(f"{name} must be one of {', '.join(options)}")


def relative_path(value: str, name: str) -> None:
    text(value, name)
    path = PurePosixPath(value)
    if path.is_absolute() or PureWindowsPath(value).drive or "\\" in value or ".." in path.parts:
        raise ValidationError(f"{name} must be a portable relative path")


def _items(value: object, item_type: type, name: str) -> tuple:
    if not isinstance(value, (tuple, list)) or any(not isinstance(item, item_type) for item in value):
        raise ValidationError(f"{name} must contain {item_type.__name__} records")
    return tuple(value)


def _unique(values: list[object], name: str) -> None:
    if len(set(values)) != len(values):
        raise ValidationError(f"duplicate {name}")


@dataclass(frozen=True)
class Repository:
    url: str

    def __post_init__(self) -> None:
        text(self.url, "repository.url")


@dataclass(frozen=True)
class Link:
    url: str
    description: str

    def __post_init__(self) -> None:
        text(self.url, "link.url")
        text(self.description, "link.description", empty=True)


@dataclass(frozen=True)
class Document:
    repository: str
    ref: str
    path: str
    description: str

    def __post_init__(self) -> None:
        text(self.repository, "document.repository")
        text(self.ref, "document.ref")
        relative_path(self.path, "document.path")
        text(self.description, "document.description", empty=True)

    @property
    def identity(self) -> tuple[str, str, str]:
        return self.repository, self.ref, self.path


@dataclass(frozen=True)
class Checkout:
    repository: str
    ref: str
    directory_within_checkout: str = "."

    def __post_init__(self) -> None:
        text(self.repository, "checkout.repository")
        text(self.ref, "checkout.ref")
        relative_path(self.directory_within_checkout, "checkout.directory_within_checkout")


@dataclass(frozen=True)
class Stream:
    id: str
    title: str
    goal: str | None
    owner_host_id: str
    title_mode: NamingMode = "automatic"
    goal_short: str | None = None
    goal_mode: NamingMode = "automatic"
    status: Status = "active"
    archived: bool = False
    repositories: Mapping[str, Repository] = field(default_factory=dict)
    documents: tuple[Document, ...] = ()
    issues: tuple[Link, ...] = ()
    prs: tuple[Link, ...] = ()

    def __post_init__(self) -> None:
        for name in ("id", "title", "owner_host_id"):
            text(getattr(self, name), f"stream.{name}")
        if self.goal is not None:
            text(self.goal, "stream.goal")
        elif self.goal_short is not None:
            raise ValidationError("stream.goal_short requires a goal")
        choice(self.title_mode, "stream.title_mode", NAMING_MODES)
        choice(self.goal_mode, "stream.goal_mode", NAMING_MODES)
        choice(self.status, "stream.status", STATUSES)
        if type(self.archived) is not bool:
            raise ValidationError("stream.archived must be a boolean")
        if self.goal_short is not None:
            text(self.goal_short, "stream.goal_short")
        if not isinstance(self.repositories, Mapping):
            raise ValidationError("stream.repositories must be a mapping")
        repositories = dict(self.repositories)
        for alias, repository in repositories.items():
            text(alias, "repository alias")
            if not isinstance(repository, Repository):
                raise ValidationError("stream.repositories must contain Repository records")
        object.__setattr__(self, "repositories", MappingProxyType(repositories))
        for name, item_type in (("documents", Document), ("issues", Link), ("prs", Link)):
            values = _items(getattr(self, name), item_type, name)
            _unique([item.identity if isinstance(item, Document) else item.url for item in values], name)
            object.__setattr__(self, name, values)
        for document in self.documents:
            if document.repository not in repositories:
                raise ValidationError(f"unknown document repository: {document.repository}")


@dataclass(frozen=True)
class Session:
    id: str
    host_id: str
    stream_id: str
    harness: str
    harness_session_id: str
    title: str
    title_mode: NamingMode = "automatic"
    status: Status = "active"
    state: str = ""
    state_short: str | None = None
    remaining: str = ""
    checkout: Checkout | None = None

    def __post_init__(self) -> None:
        for name in ("id", "host_id", "stream_id", "harness", "harness_session_id", "title"):
            text(getattr(self, name), f"session.{name}")
        choice(self.title_mode, "session.title_mode", NAMING_MODES)
        choice(self.status, "session.status", STATUSES)
        for name in ("state", "remaining"):
            text(getattr(self, name), f"session.{name}", empty=True)
        if self.state_short is not None:
            text(self.state_short, "session.state_short")
        if self.checkout is not None and not isinstance(self.checkout, Checkout):
            raise ValidationError("session.checkout must be a Checkout record")

    @property
    def native_identity(self) -> tuple[str, str, str]:
        return self.host_id, self.harness, self.harness_session_id


@dataclass(frozen=True)
class Host:
    id: str
    label: str
    checked_at: datetime

    def __post_init__(self) -> None:
        text(self.id, "host.id")
        text(self.label, "host.label")
        if not isinstance(self.checked_at, datetime) or self.checked_at.utcoffset() is None:
            raise ValidationError("host.checked_at must be a timezone-aware datetime")


Record = Stream | Session | Host


@dataclass(frozen=True)
class RecordKey:
    kind: Literal["stream", "session", "host"]
    id: str
    host_id: str | None = None

    def __post_init__(self) -> None:
        choice(self.kind, "record kind", ("stream", "session", "host"))
        text(self.id, "record id")
        if self.kind == "session":
            text(self.host_id, "session host id")
        elif self.host_id is not None:
            raise ValidationError("only session keys have a host id")


def record_key(record: Record) -> RecordKey:
    if isinstance(record, Stream):
        return RecordKey("stream", record.id)
    if isinstance(record, Session):
        return RecordKey("session", record.id, record.host_id)
    if isinstance(record, Host):
        return RecordKey("host", record.id)
    raise ValidationError("expected a stream, session or host record")


@dataclass(frozen=True)
class Catalog:
    """A closed record set with unique identities and resolvable references.

    Sessions name their stream, not vice versa. Historical sessions may belong
    to a different host from the stream owner; membership is not report authority.
    """

    streams: tuple[Stream, ...] = ()
    sessions: tuple[Session, ...] = ()
    hosts: tuple[Host, ...] = ()

    def __post_init__(self) -> None:
        for name, item_type in (("streams", Stream), ("sessions", Session), ("hosts", Host)):
            values = _items(getattr(self, name), item_type, name)
            _unique([record_key(item) for item in values], f"{name} record identity")
            object.__setattr__(self, name, values)
        _unique([session.native_identity for session in self.sessions], "native session identity")
        streams = {stream.id: stream for stream in self.streams}
        for session in self.sessions:
            if session.stream_id not in streams:
                raise ValidationError(f"unknown session stream: {session.stream_id}")
            if session.checkout and session.checkout.repository not in streams[session.stream_id].repositories:
                raise ValidationError(f"unknown checkout repository: {session.checkout.repository}")

    def stream(self, id: str) -> Stream | None:
        return next((stream for stream in self.streams if stream.id == id), None)

    def session(self, host_id: str, id: str) -> Session | None:
        return next((session for session in self.sessions if (session.host_id, session.id) == (host_id, id)), None)

    @property
    def records(self) -> tuple[Record, ...]:
        return self.streams + self.sessions + self.hosts

    def get(self, key: RecordKey) -> Record | None:
        return next((record for record in self.records if record_key(record) == key), None)

    def replacing(self, record: Record) -> Catalog:
        key = record_key(record)
        records = tuple(item for item in self.records if record_key(item) != key) + (record,)
        return Catalog(
            streams=tuple(item for item in records if isinstance(item, Stream)),
            sessions=tuple(item for item in records if isinstance(item, Session)),
            hosts=tuple(item for item in records if isinstance(item, Host)),
        )
