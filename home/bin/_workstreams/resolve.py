"""Deterministic task and CMUX workspace resolution."""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .errors import WorkstreamError
from .slug import candidate_slugs, explicit_slug, explicit_title, slugify
from .taskwarrior import export, pending_tasks

UUID_RE = re.compile(
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
)
TASK_TOKEN_RE = re.compile(r"\btask:(" + UUID_RE.pattern + r"|[0-9a-fA-F]{8})\b")
PR_RE = re.compile(r"github\.com/([^/\s]+/[^/\s]+)/pull/(\d+)")


@dataclass(frozen=True)
class TaskResolution:
    task: dict[str, Any]
    reason: str

    @property
    def legacy(self) -> bool:
        return self.reason != "slug"


def describe_task(task: dict[str, Any]) -> str:
    return f"{task.get('id')}:{task.get('uuid')} {task.get('title') or '-'} — {task.get('description')}"


def one_task(matches: list[dict[str, Any]], selector: str) -> dict[str, Any]:
    if not matches:
        raise WorkstreamError(f"no pending workstream task matches {selector!r}")
    if len(matches) > 1:
        choices = "\n".join(f"  {describe_task(task)}" for task in matches)
        raise WorkstreamError(f"selector {selector!r} is ambiguous:\n{choices}")
    return matches[0]


def task_matches_uuid_token(task: dict[str, Any], token: str) -> bool:
    uuid = str(task.get("uuid") or "")
    if UUID_RE.fullmatch(token):
        return uuid.lower() == token.lower()
    return uuid.lower().startswith(token.lower())


def task_token_from_description(workspace: dict[str, Any]) -> str | None:
    description = workspace.get("description") or ""
    match = TASK_TOKEN_RE.search(description)
    return match.group(1) if match else None


def resolve_task(selector: str) -> dict[str, Any]:
    """Resolve uuid/id/slug/title-ish selector to exactly one pending task.

    Slug resolution is exact after normalizing the selector with slugify. This
    intentionally fails closed rather than fuzzy-matching automation inputs.
    """
    selector = selector.strip()
    if not selector:
        raise WorkstreamError("empty task selector")

    tasks = pending_tasks(include_waiting=True)

    if UUID_RE.fullmatch(selector):
        return one_task(
            [task for task in tasks if str(task.get("uuid", "")).lower() == selector.lower()],
            selector,
        )

    if re.fullmatch(r"[0-9a-fA-F]{8,}", selector):
        return one_task(
            [task for task in tasks if str(task.get("uuid", "")).lower().startswith(selector.lower())],
            selector,
        )

    if selector.isdigit():
        matches = [task for task in export(selector) if task.get("status") == "pending"]
        return one_task(matches, selector)

    if selector.startswith("pr:") and selector.removeprefix("pr:").isdigit():
        pr = selector.removeprefix("pr:")
        return one_task([task for task in tasks if str(task.get("pr") or "") == pr], selector)

    selector_slug = slugify(selector)
    if selector_slug:
        slug_matches = [task for task in tasks if explicit_slug(task) == selector_slug]
        if slug_matches:
            return one_task(slug_matches, selector)

        title_slug_matches = [
            task for task in tasks if explicit_title(task) and slugify(str(explicit_title(task))) == selector_slug
        ]
        if title_slug_matches:
            return one_task(title_slug_matches, selector)

    title_matches = [task for task in tasks if explicit_title(task) == selector]
    if title_matches:
        return one_task(title_matches, selector)

    raise WorkstreamError(f"no pending workstream task matches {selector!r}")


def candidate_titles(task: dict[str, Any]) -> set[str]:
    titles: set[str] = set()
    if title := explicit_title(task):
        titles.add(title)
    if slug := explicit_slug(task):
        titles.add(slug)
    titles.update(candidate_slugs(task))
    if workspace := task.get("workspace"):
        titles.add(Path(str(workspace)).name)
    if bookmark := task.get("bookmark"):
        titles.add(str(bookmark).split("/")[-1])
    for annotation in task.get("annotations") or []:
        description = annotation.get("description") or ""
        titles.update(title.strip() for title in re.findall(r"CMUX .*?\(([^)]+)\)", description))
    return {title for title in titles if title}


def task_matches_pr_link(task: dict[str, Any], workspace: dict[str, Any]) -> bool:
    description = workspace.get("description") or ""
    for repo, pr in PR_RE.findall(description):
        if task.get("repo") == repo and str(task.get("pr")) == pr:
            return True
    return False


def task_matches_workspace_cwd(task: dict[str, Any], workspace: dict[str, Any]) -> bool:
    cwd = workspace.get("current_directory") or ""
    return bool(cwd and task.get("workspace") == cwd)


def resolve_cmux_workspace(
    workspace: dict[str, Any],
    *,
    tasks: list[dict[str, Any]] | None = None,
    allow_legacy_token: bool = True,
    allow_fallbacks: bool = True,
) -> TaskResolution:
    """Resolve a CMUX workspace to a Taskwarrior task.

    Steady-state resolution is ``slugify(CMUX title) -> task.slug``. Legacy
    visible ``task:<uuid-prefix>`` links and older cwd/title/PR heuristics can
    remain enabled for migration, but callers can inspect ``reason`` to warn
    about those non-steady-state paths.
    """
    tasks = tasks if tasks is not None else pending_tasks(include_waiting=True)
    title = str(workspace.get("title") or "")

    if allow_legacy_token and (token := task_token_from_description(workspace)):
        matches = [task for task in tasks if task_matches_uuid_token(task, token)]
        return TaskResolution(one_task(matches, f"task:{token}"), "legacy-task-token")

    title_slug = slugify(title)
    if title_slug:
        matches = [task for task in tasks if explicit_slug(task) == title_slug]
        if matches:
            return TaskResolution(one_task(matches, f"CMUX title slug {title_slug!r}"), "slug")

    if allow_fallbacks:
        exact_title_matches = [task for task in tasks if title and explicit_title(task) == title]
        if exact_title_matches:
            return TaskResolution(one_task(exact_title_matches, f"CMUX title {title!r}"), "title")

        candidate_title_matches = [task for task in tasks if title and title in candidate_titles(task)]
        if candidate_title_matches:
            return TaskResolution(one_task(candidate_title_matches, f"CMUX title candidate {title!r}"), "candidate-title")

        pr_matches = [task for task in tasks if task_matches_pr_link(task, workspace)]
        if pr_matches:
            return TaskResolution(one_task(pr_matches, f"CMUX PR link {title!r}"), "pr-link")

        if not title:
            cwd_matches = [task for task in tasks if task_matches_workspace_cwd(task, workspace)]
            if cwd_matches:
                return TaskResolution(one_task(cwd_matches, f"CMUX cwd {workspace.get('current_directory')!r}"), "cwd")

    ref = workspace.get("ref") or workspace.get("id") or "<unknown>"
    raise WorkstreamError(
        f"could not find a pending Taskwarrior task for {ref} {title!r}; "
        "set a unique Taskwarrior slug matching the CMUX title, add legacy task:<uuid-prefix> "
        "to the CMUX description, or add a CMUX annotation to the task"
    )
