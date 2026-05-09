"""Deterministic task and CMUX workspace resolution."""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path

from .errors import WorkstreamError
from .models import CmuxWorkspace, Task
from .slug import candidate_slugs, slugify
from .taskwarrior import export, pending_tasks

UUID_RE = re.compile(
    r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}"
)
TASK_TOKEN_RE = re.compile(r"\btask:(" + UUID_RE.pattern + r"|[0-9a-fA-F]{8})\b")
PR_RE = re.compile(r"github\.com/([^/\s]+/[^/\s]+)/pull/(\d+)")


@dataclass(frozen=True)
class TaskResolution:
    task: Task
    reason: str

    @property
    def legacy(self) -> bool:
        return self.reason != "slug"


def describe_task(task: Task) -> str:
    return f"{task.id}:{task.uuid} {task.title or '-'} — {task.description}"


def one_task(matches: list[Task], selector: str) -> Task:
    if not matches:
        raise WorkstreamError(f"no pending workstream task matches {selector!r}")
    if len(matches) > 1:
        choices = "\n".join(f"  {describe_task(task)}" for task in matches)
        raise WorkstreamError(f"selector {selector!r} is ambiguous:\n{choices}")
    return matches[0]


def task_matches_uuid_token(task: Task, token: str) -> bool:
    if UUID_RE.fullmatch(token):
        return task.uuid.lower() == token.lower()
    return task.uuid.lower().startswith(token.lower())


def task_token_from_description(workspace: CmuxWorkspace) -> str | None:
    match = TASK_TOKEN_RE.search(workspace.description)
    return match.group(1) if match else None


def resolve_task(selector: str) -> Task:
    """Resolve uuid/id/slug/title-ish selector to exactly one pending task.

    Slug resolution is exact after normalizing the selector with slugify. This
    intentionally fails closed rather than fuzzy-matching automation inputs.
    """
    selector = selector.strip()
    if not selector:
        raise WorkstreamError("empty task selector")

    tasks = pending_tasks(include_waiting=True)

    if UUID_RE.fullmatch(selector):
        return one_task([task for task in tasks if task.uuid.lower() == selector.lower()], selector)

    if re.fullmatch(r"[0-9a-fA-F]{8,}", selector):
        return one_task(
            [task for task in tasks if task.uuid.lower().startswith(selector.lower())],
            selector,
        )

    if selector.isdigit():
        matches = [task for task in export(selector) if task.status == "pending"]
        return one_task(matches, selector)

    if selector.startswith("pr:") and selector.removeprefix("pr:").isdigit():
        pr = selector.removeprefix("pr:")
        return one_task([task for task in tasks if task.pr == pr], selector)

    selector_slug = slugify(selector)
    if selector_slug:
        slug_matches = [task for task in tasks if task.slug == selector_slug]
        if slug_matches:
            return one_task(slug_matches, selector)

        title_slug_matches = [
            task for task in tasks if task.title and slugify(task.title) == selector_slug
        ]
        if title_slug_matches:
            return one_task(title_slug_matches, selector)

    title_matches = [task for task in tasks if task.title == selector]
    if title_matches:
        return one_task(title_matches, selector)

    raise WorkstreamError(f"no pending workstream task matches {selector!r}")


def candidate_titles(task: Task) -> set[str]:
    titles: set[str] = set()
    if task.title:
        titles.add(task.title)
    if task.slug:
        titles.add(task.slug)
    titles.update(candidate_slugs(task))
    if task.workspace:
        titles.add(Path(task.workspace).name)
    if task.bookmark:
        titles.add(task.bookmark.split("/")[-1])
    for annotation in task.annotations:
        description = annotation.get("description") or ""
        titles.update(title.strip() for title in re.findall(r"CMUX .*?\(([^)]+)\)", description))
    return {title for title in titles if title}


def task_matches_pr_link(task: Task, workspace: CmuxWorkspace) -> bool:
    for repo, pr in PR_RE.findall(workspace.description):
        if task.repo == repo and task.pr == pr:
            return True
    return False


def comparable_path(path: str) -> str:
    return os.path.normpath(os.path.expanduser(path))


def task_matches_workspace_cwd(task: Task, workspace: CmuxWorkspace) -> bool:
    return bool(
        workspace.current_directory
        and task.workspace
        and comparable_path(task.workspace) == comparable_path(workspace.current_directory)
    )


def resolve_cmux_workspace(
    workspace: CmuxWorkspace,
    *,
    tasks: list[Task] | None = None,
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

    if allow_legacy_token and (token := task_token_from_description(workspace)):
        matches = [task for task in tasks if task_matches_uuid_token(task, token)]
        return TaskResolution(one_task(matches, f"task:{token}"), "legacy-task-token")

    title_slug = slugify(workspace.title)
    if title_slug:
        matches = [task for task in tasks if task.slug == title_slug]
        if matches:
            return TaskResolution(one_task(matches, f"CMUX title slug {title_slug!r}"), "slug")

    if allow_fallbacks:
        exact_title_matches = [task for task in tasks if workspace.title and task.title == workspace.title]
        if exact_title_matches:
            return TaskResolution(one_task(exact_title_matches, f"CMUX title {workspace.title!r}"), "title")

        candidate_title_matches = [task for task in tasks if workspace.title and workspace.title in candidate_titles(task)]
        if candidate_title_matches:
            return TaskResolution(one_task(candidate_title_matches, f"CMUX title candidate {workspace.title!r}"), "candidate-title")

        pr_matches = [task for task in tasks if task_matches_pr_link(task, workspace)]
        if pr_matches:
            return TaskResolution(one_task(pr_matches, f"CMUX PR link {workspace.title!r}"), "pr-link")

        if not workspace.title:
            cwd_matches = [task for task in tasks if task_matches_workspace_cwd(task, workspace)]
            if cwd_matches:
                return TaskResolution(one_task(cwd_matches, f"CMUX cwd {workspace.current_directory!r}"), "cwd")

    raise WorkstreamError(
        f"could not find a pending Taskwarrior task for {workspace.display_ref} {workspace.title!r}; "
        "set a unique Taskwarrior slug matching the CMUX title, add legacy task:<uuid-prefix> "
        "to the CMUX description, or add a CMUX annotation to the task"
    )
