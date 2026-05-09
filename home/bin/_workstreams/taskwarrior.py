"""Small Taskwarrior wrapper for workstream scripts."""

from __future__ import annotations

import json
import subprocess
from typing import Any

from .errors import WorkstreamError
from .models import Task

STATE_TAGS = {"doing", "ready", "review", "blocked", "parked"}


def run(args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(args, text=True, capture_output=True, check=check)
    except FileNotFoundError as exc:
        raise WorkstreamError(f"missing executable: {args[0]}") from exc
    except subprocess.CalledProcessError as exc:
        detail = (exc.stderr or exc.stdout or "").strip()
        raise WorkstreamError(f"{' '.join(args)} failed: {detail}") from exc


def _json_from_stdout(command: list[str], stdout: str) -> Any:
    text = stdout.strip()
    start = min((idx for idx in (text.find("{"), text.find("[")) if idx != -1), default=-1)
    if start == -1:
        raise WorkstreamError(f"{' '.join(command)} did not return JSON")
    return json.loads(text[start:])


def export(*selectors: str) -> list[Task]:
    command = ["task", "rc.gc=off", *selectors, "export"]
    data = _json_from_stdout(command, run(command).stdout)
    if not isinstance(data, list):
        raise WorkstreamError("task export returned an unexpected shape")
    return [Task.from_raw(task) for task in data if isinstance(task, dict)]


def pending_tasks(*, include_waiting: bool = True) -> list[Task]:
    tasks: list[Task] = []
    seen: set[str] = set()
    selectors = ["status:pending"] + (["+WAITING"] if include_waiting else [])
    for selector in selectors:
        for task in export(selector):
            if task.uuid and task.uuid in seen:
                continue
            if task.uuid:
                seen.add(task.uuid)
            tasks.append(task)
    return tasks


def is_workstream(task: Task) -> bool:
    if task.tags & STATE_TAGS:
        return True
    return bool(task.repo or task.workspace or task.bookmark or task.pr or task.aisession or task.title or task.slug)


def modify(selector: str, *args: str) -> None:
    run(["task", selector, "modify", *args])


def annotate(selector: str, text: str) -> None:
    run(["task", selector, "annotate", text])
