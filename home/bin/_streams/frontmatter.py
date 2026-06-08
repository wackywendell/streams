"""Markdown frontmatter parsing for scratch streams."""

from __future__ import annotations

from typing import Any

import yaml

from .errors import StreamError


def split_frontmatter(text: str) -> tuple[dict[str, Any], str]:
    if not text.startswith("---\n"):
        raise StreamError("missing YAML frontmatter")
    end = text.find("\n---", 4)
    if end == -1:
        raise StreamError("unterminated YAML frontmatter")
    raw = text[4:end]
    body_start = end + len("\n---")
    if text[body_start : body_start + 1] == "\n":
        body_start += 1
    data = yaml.safe_load(raw) or {}
    if not isinstance(data, dict):
        raise StreamError("frontmatter must be a mapping")
    return data, text[body_start:]


def render_frontmatter(data: dict[str, Any], body: str) -> str:
    raw = yaml.safe_dump(data, sort_keys=False, allow_unicode=True, default_flow_style=False).strip()
    body_text = body[1:] if body.startswith("\n") else body
    return f"---\n{raw}\n---\n{body_text}"
