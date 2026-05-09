"""Parsed workstream data models."""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any


@dataclass(frozen=True)
class CmuxWorkspace:
    """A parsed CMUX workspace with raw RPC data preserved."""

    raw: dict[str, Any]

    @classmethod
    def from_raw(cls, raw: dict[str, Any]) -> "CmuxWorkspace":
        return cls(raw=dict(raw))

    def get(self, key: str, default: Any = None) -> Any:
        """Compatibility escape hatch for fields not modeled yet."""
        return self.raw.get(key, default)

    @property
    def id(self) -> str:
        return str(self.raw.get("id") or "")

    @property
    def ref(self) -> str:
        return str(self.raw.get("ref") or "")

    @property
    def title(self) -> str:
        return str(self.raw.get("title") or "")

    @property
    def description(self) -> str:
        return str(self.raw.get("description") or "")

    @property
    def current_directory(self) -> str | None:
        cwd = self.raw.get("current_directory")
        return str(cwd) if cwd else None

    @property
    def display_ref(self) -> str:
        return self.ref or self.id or "<unknown>"


@dataclass(frozen=True)
class Task:
    """A parsed Taskwarrior task with raw export data preserved.

    Taskwarrior's JSON shape is intentionally flexible. Keep ``raw`` available
    for fields not modeled yet, but route common workstream identity/state access
    through typed properties so resolution code does not repeatedly parse dicts.
    """

    raw: dict[str, Any]

    @classmethod
    def from_raw(cls, raw: dict[str, Any]) -> "Task":
        return cls(raw=dict(raw))

    def get(self, key: str, default: Any = None) -> Any:
        """Compatibility escape hatch for fields not modeled yet."""
        return self.raw.get(key, default)

    @property
    def id(self) -> int | str | None:
        return self.raw.get("id")

    @property
    def uuid(self) -> str:
        return str(self.raw.get("uuid") or "")

    @property
    def status(self) -> str:
        return str(self.raw.get("status") or "")

    @property
    def description(self) -> str:
        return str(self.raw.get("description") or "")

    @property
    def tags(self) -> frozenset[str]:
        return frozenset(str(tag) for tag in (self.raw.get("tags") or []))

    @property
    def title(self) -> str | None:
        title = self.raw.get("title")
        return str(title) if title else None

    @property
    def slug(self) -> str | None:
        slug = self.raw.get("slug")
        return str(slug) if slug else None

    @property
    def repo(self) -> str | None:
        repo = self.raw.get("repo")
        return str(repo) if repo else None

    @property
    def scratch(self) -> str | None:
        scratch = self.raw.get("scratch")
        return str(scratch) if scratch else None

    @property
    def workspace(self) -> str | None:
        workspace = self.raw.get("workspace")
        return str(workspace) if workspace else None

    @property
    def bookmark(self) -> str | None:
        bookmark = self.raw.get("bookmark")
        return str(bookmark) if bookmark else None

    @property
    def pr(self) -> str | None:
        pr = self.raw.get("pr")
        return str(pr) if pr else None

    @property
    def aisession(self) -> str | None:
        session = self.raw.get("aisession")
        return str(session) if session else None

    @property
    def annotations(self) -> tuple[dict[str, Any], ...]:
        annotations = self.raw.get("annotations") or []
        return tuple(annotation for annotation in annotations if isinstance(annotation, dict))
