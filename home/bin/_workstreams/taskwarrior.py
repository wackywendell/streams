"""Small Taskwarrior wrapper for workstream scripts."""

from __future__ import annotations

import json
import subprocess
from typing import Any

from .errors import WorkstreamError

STATE_TAGS = {"doing", "ready", "review", "blocked", "parked"}
WORKSTREAM_FIELDS = {"repo", "workspace", "bookmark", "pr", "aisession", "title", "slug"}


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


def export(*selectors: str) -> list[dict[str, Any]]:
    command = ["task", "rc.gc=off", *selectors, "export"]
    data = _json_from_stdout(command, run(command).stdout)
    if not isinstance(data, list):
        raise WorkstreamError("task export returned an unexpected shape")
    return data


def pending_tasks(*, include_waiting: bool = True) -> list[dict[str, Any]]:
    tasks: list[dict[str, Any]] = []
    seen: set[str] = set()
    selectors = ["status:pending"] + (["+WAITING"] if include_waiting else [])
    for selector in selectors:
        for task in export(selector):
            uuid = str(task.get("uuid") or "")
            if uuid and uuid in seen:
                continue
            if uuid:
                seen.add(uuid)
            tasks.append(task)
    return tasks


def is_workstream(task: dict[str, Any]) -> bool:
    tags = set(task.get("tags") or [])
    if tags & STATE_TAGS:
        return True
    return any(task.get(field) for field in WORKSTREAM_FIELDS)


def modify(selector: str, *args: str) -> None:
    run(["task", selector, "modify", *args])


def annotate(selector: str, text: str) -> None:
    run(["task", selector, "annotate", text])
