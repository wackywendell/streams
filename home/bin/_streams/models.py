"""Data models for scratch streams."""

from __future__ import annotations

from dataclasses import dataclass, field
from datetime import date, datetime
from pathlib import Path
from typing import Any

from .errors import StreamError
from .schema import error_messages, validate_frontmatter


@dataclass(frozen=True)
class Stream:
    path: Path
    frontmatter: dict[str, Any]
    body: str = field(repr=False, compare=False)

    @property
    def slug(self) -> str:
        return self.path.stem

    @property
    def title(self) -> str:
        return str(self.frontmatter.get("title") or self.slug)

    @property
    def status(self) -> str:
        return str(self.frontmatter.get("status") or "")

    @property
    def state(self) -> str:
        return str(self.frontmatter.get("state") or "")

    def validate(self) -> None:
        errors = error_messages(validate_frontmatter(self.path, self.frontmatter))
        if errors:
            raise StreamError(f"{self.path}: {'; '.join(errors)}")

    def as_json(self) -> dict[str, Any]:
        payload = _json_value(dict(self.frontmatter))
        payload["slug"] = self.slug
        payload["path"] = str(self.path)
        return payload


def _json_value(value: Any) -> Any:
    if isinstance(value, (date, datetime)):
        return value.isoformat()
    if isinstance(value, dict):
        return {str(key): _json_value(item) for key, item in value.items()}
    if isinstance(value, list):
        return [_json_value(item) for item in value]
    if isinstance(value, tuple):
        return [_json_value(item) for item in value]
    return value
