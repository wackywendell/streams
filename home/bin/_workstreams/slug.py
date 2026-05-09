"""Title and slug conventions for Taskwarrior workstreams."""

from __future__ import annotations

import re
import unicodedata
from pathlib import Path
from typing import Any

SLUG_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$")
OWNER_PREFIX_RE = re.compile(r"^[a-z0-9_.-]+/")


class SlugError(ValueError):
    pass


def _slugify_core(text: str) -> str:
    value = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode("ascii")
    value = value.strip().lower()
    value = re.sub(r"[^a-z0-9]+", "-", value)
    value = re.sub(r"-+", "-", value).strip("-")
    return value


def slugify(text: str) -> str:
    """Normalize a human workstream title to a slug.

    Human titles do not strip slash-separated prefixes: "API / HTTP" should
    become "api-http", not "http". Use ``slugify_ref`` for branch/bookmark
    names where an owner prefix such as ``user/foo`` should be ignored.
    """
    return _slugify_core(text)


def slugify_ref(text: str) -> str:
    """Normalize a branch/bookmark/workspace ref to a slug.

    This strips a leading owner prefix such as ``user/`` before applying the
    normal title slug rules.
    """
    return _slugify_core(OWNER_PREFIX_RE.sub("", text.strip().lower()))


def validate_slug(slug: str) -> None:
    if not SLUG_RE.fullmatch(slug):
        raise SlugError(
            f"invalid slug {slug!r}; use lowercase kebab-case matching {SLUG_RE.pattern}"
        )


def normalized_slug(text: str) -> str:
    slug = slugify(text)
    validate_slug(slug)
    return slug


def explicit_slug(task: dict[str, Any]) -> str | None:
    slug = task.get("slug")
    return str(slug) if slug else None


def explicit_title(task: dict[str, Any]) -> str | None:
    title = task.get("title")
    return str(title) if title else None


def suggested_title(task: dict[str, Any]) -> str | None:
    """Suggest a stable display title without mutating the task."""
    if title := explicit_title(task):
        return title
    if bookmark := task.get("bookmark"):
        return str(bookmark).split("/")[-1].replace("-", " ").title()
    if workspace := task.get("workspace"):
        return Path(str(workspace)).name.replace("-", " ").title()
    if pr := task.get("pr"):
        repo = str(task.get("repo") or "").split("/")[-1]
        return f"{repo} PR {pr}" if repo else f"PR {pr}"
    return None


def suggested_slug(task: dict[str, Any]) -> str | None:
    """Suggest a slug from explicit title or legacy workspace/bookmark metadata."""
    if slug := explicit_slug(task):
        return slug
    if title := explicit_title(task):
        return slugify(title)
    if bookmark := task.get("bookmark"):
        return slugify_ref(str(bookmark))
    if workspace := task.get("workspace"):
        return slugify_ref(Path(str(workspace)).name)
    if pr := task.get("pr"):
        repo = str(task.get("repo") or "").split("/")[-1]
        return slugify(f"{repo}-{pr}" if repo else f"pr-{pr}")
    return None


def candidate_slugs(task: dict[str, Any]) -> set[str]:
    candidates: set[str] = set()
    for value in (
        explicit_slug(task),
        slugify(explicit_title(task) or ""),
        suggested_slug(task),
    ):
        if value:
            candidates.add(value)
    if bookmark := task.get("bookmark"):
        candidates.add(slugify_ref(str(bookmark)))
    if workspace := task.get("workspace"):
        candidates.add(slugify_ref(Path(str(workspace)).name))
    return {candidate for candidate in candidates if candidate}
