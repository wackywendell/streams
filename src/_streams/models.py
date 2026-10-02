"""Data models for scratch streams."""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import date, datetime
from pathlib import Path
from typing import Any, Literal, cast

from .errors import StreamError
from .location import StreamLocation
from .schema import error_messages, validate_frontmatter

Status = Literal["active", "waiting", "blocked", "parked", "done"]


@dataclass(frozen=True)
class Link:
    """An entry in `issues` or `prs`."""

    url: str
    description: str


@dataclass(frozen=True)
class Stream:
    """A scratch note whose frontmatter passed the schema. Build one with `Stream.parse`."""

    path: Path
    # As stored, for JSON output and rewrites; read the typed fields instead.
    frontmatter: dict[str, Any] = field(repr=False)
    body: str = field(repr=False, compare=False)
    title: str
    status: Status
    description: str
    state: str
    location: StreamLocation
    issues: tuple[Link, ...]
    prs: tuple[Link, ...]
    sessions: tuple[str, ...]

    @classmethod
    def parse(cls, path: Path, frontmatter: dict[str, Any], body: str) -> Stream:
        errors = error_messages(validate_frontmatter(path, frontmatter))
        if errors:
            raise StreamError(f"{path}: {'; '.join(errors)}", kind="invalid-stream")
        return cls(
            path=path,
            frontmatter=frontmatter,
            body=body,
            title=frontmatter["title"],
            status=cast(Status, frontmatter["status"]),
            description=frontmatter["description"],
            state=frontmatter["state"],
            location=StreamLocation.from_strings(vcs=frontmatter["vcs"], repo=frontmatter["repo"], cwd=frontmatter["cwd"]),
            issues=tuple(Link(item["url"], item["description"]) for item in frontmatter["issues"]),
            prs=tuple(Link(item["url"], item["description"]) for item in frontmatter["prs"]),
            sessions=tuple(frontmatter["sessions"]),
        )

    @property
    def slug(self) -> str:
        return self.path.stem

    def as_json(self) -> dict[str, Any]:
        payload = json_value(dict(self.frontmatter))
        payload["slug"] = self.slug
        payload["path"] = str(self.path)
        return payload


def json_value(value: Any) -> Any:
    if isinstance(value, (date, datetime)):
        return value.isoformat()
    if isinstance(value, dict):
        return {str(key): json_value(item) for key, item in value.items()}
    if isinstance(value, list):
        return [json_value(item) for item in value]
    if isinstance(value, tuple):
        return [json_value(item) for item in value]
    return value
