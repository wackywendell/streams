"""Deterministic task resolution for workstream selectors."""

from __future__ import annotations

import re
from typing import Any

from .errors import WorkstreamError
from .slug import explicit_slug, explicit_title, slugify
from .taskwarrior import export, pending_tasks

UUID_RE = re.compile(
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
)


def _describe(task: dict[str, Any]) -> str:
    return f"{task.get('id')}:{task.get('uuid')} {task.get('title') or '-'} — {task.get('description')}"


def _one(matches: list[dict[str, Any]], selector: str) -> dict[str, Any]:
    if not matches:
        raise WorkstreamError(f"no pending workstream task matches {selector!r}")
    if len(matches) > 1:
        choices = "\n".join(f"  {_describe(task)}" for task in matches)
        raise WorkstreamError(f"selector {selector!r} is ambiguous:\n{choices}")
    return matches[0]


def resolve_task(selector: str) -> dict[str, Any]:
    """Resolve uuid/id/slug/title-ish selector to exactly one pending task.

    Slug resolution is exact after normalizing the selector with slugify. This
    intentionally fails closed rather than fuzzy-matching automation inputs.
    """
    selector = selector.strip()
    if not selector:
        raise WorkstreamError("empty task selector")

    if UUID_RE.fullmatch(selector):
        return _one(export(selector), selector)

    tasks = pending_tasks(include_waiting=True)

    if re.fullmatch(r"[0-9a-fA-F]{8,}", selector):
        return _one(
            [task for task in tasks if str(task.get("uuid", "")).lower().startswith(selector.lower())],
            selector,
        )

    if selector.isdigit():
        matches = export(selector)
        matches = [task for task in matches if task.get("status") == "pending"]
        return _one(matches, selector)

    if selector.startswith("pr:") and selector.removeprefix("pr:").isdigit():
        pr = selector.removeprefix("pr:")
        return _one([task for task in tasks if str(task.get("pr") or "") == pr], selector)

    selector_slug = slugify(selector)
    if selector_slug:
        slug_matches = [task for task in tasks if explicit_slug(task) == selector_slug]
        if slug_matches:
            return _one(slug_matches, selector)

        title_slug_matches = [
            task for task in tasks if explicit_title(task) and slugify(str(explicit_title(task))) == selector_slug
        ]
        if title_slug_matches:
            return _one(title_slug_matches, selector)

    title_matches = [task for task in tasks if explicit_title(task) == selector]
    if title_matches:
        return _one(title_matches, selector)

    raise WorkstreamError(f"no pending workstream task matches {selector!r}")
