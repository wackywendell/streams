"""CMUX helpers for workstream scripts."""

from __future__ import annotations

import json
import re
import subprocess
from typing import Any

from .errors import WorkstreamError
from .resolve import UUID_RE

TASK_TOKEN_RE = re.compile(r"\btask:(" + UUID_RE.pattern + r"|[0-9a-fA-F]{8})\b")


def run(args: list[str]) -> subprocess.CompletedProcess[str]:
    try:
        return subprocess.run(args, text=True, capture_output=True, check=True)
    except FileNotFoundError as exc:
        raise WorkstreamError(f"missing executable: {args[0]}") from exc
    except subprocess.CalledProcessError as exc:
        detail = (exc.stderr or exc.stdout or "").strip()
        raise WorkstreamError(f"{' '.join(args)} failed: {detail}") from exc


def json_from_command(args: list[str]) -> Any:
    text = run(args).stdout.strip()
    start = min((idx for idx in (text.find("{"), text.find("[")) if idx != -1), default=-1)
    if start == -1:
        raise WorkstreamError(f"{' '.join(args)} did not return JSON")
    return json.loads(text[start:])


def workspaces() -> list[dict[str, Any]]:
    data = json_from_command(["cmux", "rpc", "workspace.list", "{}"])
    if not isinstance(data, dict) or not isinstance(data.get("workspaces"), list):
        raise WorkstreamError("cmux rpc workspace.list returned an unexpected shape")
    return data["workspaces"]


def task_token_from_description(workspace: dict[str, Any]) -> str | None:
    description = workspace.get("description") or ""
    match = TASK_TOKEN_RE.search(description)
    return match.group(1) if match else None
