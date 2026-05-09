"""Title and slug conventions for Taskwarrior workstreams."""

from __future__ import annotations

import re
import unicodedata
from pathlib import Path

from .models import Task

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


def suggested_title(task: Task) -> str | None:
    """Suggest a stable display title without mutating the task."""
    if task.title:
        return task.title
    if task.bookmark:
        return task.bookmark.split("/")[-1].replace("-", " ").title()
    if task.workspace:
        return Path(task.workspace).name.replace("-", " ").title()
    if task.pr:
        repo = (task.repo or "").split("/")[-1]
        return f"{repo} PR {task.pr}" if repo else f"PR {task.pr}"
    return None


def suggested_slug(task: Task) -> str | None:
    """Suggest a slug from explicit title or legacy workspace/bookmark metadata."""
    if task.slug:
        return task.slug
    if task.title:
        return slugify(task.title)
    if task.bookmark:
        return slugify_ref(task.bookmark)
    if task.workspace:
        return slugify_ref(Path(task.workspace).name)
    if task.pr:
        repo = (task.repo or "").split("/")[-1]
        return slugify(f"{repo}-{task.pr}" if repo else f"pr-{task.pr}")
    return None


def candidate_slugs(task: Task) -> set[str]:
    candidates: set[str] = set()
    for value in (
        task.slug,
        slugify(task.title or ""),
        suggested_slug(task),
    ):
        if value:
            candidates.add(value)
    if task.bookmark:
        candidates.add(slugify_ref(task.bookmark))
    if task.workspace:
        candidates.add(slugify_ref(Path(task.workspace).name))
    return {candidate for candidate in candidates if candidate}
