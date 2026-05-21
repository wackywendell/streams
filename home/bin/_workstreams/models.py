"""Parsed workstream data models."""

from __future__ import annotations

from dataclasses import dataclass, field
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
    for fields not modeled yet, but normalize common workstream identity/state
    fields at the boundary so call sites are not repeatedly parsing dicts.
    """

    id: int | str | None = None
    uuid: str = ""
    status: str = ""
    description: str = ""
    tags: frozenset[str] = field(default_factory=frozenset)
    title: str | None = None
    slug: str | None = None
    repo: str | None = None
    scratch: str | None = None
    workspace: str | None = None
    bookmark: str | None = None
    pr: str | None = None
    aisession: str | None = None
    annotations: tuple[dict[str, Any], ...] = ()
    raw: dict[str, Any] = field(default_factory=dict, repr=False, compare=False)

    @classmethod
    def from_raw(cls, raw: dict[str, Any]) -> "Task":
        data = dict(raw)
        tags = data.get("tags") or []
        annotations = data.get("annotations") or []
        return cls(
            id=data.get("id"),
            uuid=str(data.get("uuid") or ""),
            status=str(data.get("status") or ""),
            description=str(data.get("description") or ""),
            tags=frozenset(str(tag) for tag in tags),
            title=_optional_str(data.get("title")),
            slug=_optional_str(data.get("slug")),
            repo=_optional_str(data.get("repo")),
            scratch=_optional_str(data.get("scratch")),
            workspace=_optional_str(data.get("workspace")),
            bookmark=_optional_str(data.get("bookmark")),
            pr=_optional_str(data.get("pr")),
            aisession=_optional_str(data.get("aisession")),
            annotations=tuple(annotation for annotation in annotations if isinstance(annotation, dict)),
            raw=data,
        )

    def get(self, key: str, default: Any = None) -> Any:
        """Compatibility escape hatch for fields not modeled yet."""
        return self.raw.get(key, default)


def _optional_str(value: Any) -> str | None:
    return str(value) if value else None
