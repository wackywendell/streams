"""CMUX workspace transport."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from dataclasses import dataclass
from typing import Any

from .errors import StreamError


@dataclass(frozen=True)
class CmuxWorkspace:
    raw: dict[str, Any]

    @property
    def id(self) -> str:
        return str(self.raw.get("id") or "")

    @property
    def ref(self) -> str:
        return str(self.raw.get("ref") or "")

    @property
    def title(self) -> str:
        return str(self.raw.get("title") or "")

    @property
    def index(self) -> int | None:
        value = self.raw.get("index")
        return value if isinstance(value, int) else None

    @property
    def current_directory(self) -> str:
        return str(self.raw.get("current_directory") or "")

    @property
    def display_ref(self) -> str:
        return self.ref or self.id or "<unknown>"


class CmuxClient:
    def run(self, args: list[str]) -> subprocess.CompletedProcess[str]:
        try:
            result = subprocess.run(args, text=True, capture_output=True, check=True)
            if result.stderr:
                print(result.stderr, end="", file=sys.stderr)
            return result
        except FileNotFoundError as exc:
            raise StreamError(f"missing executable: {args[0]}") from exc
        except subprocess.CalledProcessError as exc:
            detail = (exc.stderr or exc.stdout or "").strip()
            raise StreamError(f"{' '.join(args)} failed: {detail}") from exc

    def json_from_command(self, args: list[str]) -> Any:
        text = self.run(args).stdout.strip()
        start = min((idx for idx in (text.find("{"), text.find("[")) if idx != -1), default=-1)
        if start == -1:
            raise StreamError(f"{' '.join(args)} did not return JSON")
        return json.loads(text[start:])

    def workspaces(self) -> list[CmuxWorkspace]:
        data = self.json_from_command(["cmux", "rpc", "workspace.list", "{}"])
        if not isinstance(data, dict) or not isinstance(data.get("workspaces"), list):
            raise StreamError("cmux rpc workspace.list returned an unexpected shape")
        return [CmuxWorkspace(dict(workspace)) for workspace in data["workspaces"] if isinstance(workspace, dict)]

    def current_workspace_ref(self) -> str:
        if workspace_id := os.environ.get("CMUX_WORKSPACE_ID"):
            return workspace_id
        raise StreamError("current CMUX targeting requires caller context; pass --workspace outside CMUX")

    def resolve_workspace(self, selector: str | None) -> CmuxWorkspace:
        current = selector is None
        if current:
            selector = self.current_workspace_ref()
        elif not selector:
            raise StreamError("CMUX workspace selector must be nonempty")
        matches = [
            workspace
            for workspace in self.workspaces()
            if selector in {workspace.ref, workspace.id} or (not current and selector == workspace.title)
        ]
        if not matches:
            raise StreamError(f"could not find CMUX workspace {selector!r}")
        if len(matches) > 1:
            choices = ", ".join(f"{item.display_ref} {item.title}" for item in matches)
            raise StreamError(f"CMUX workspace {selector!r} is ambiguous: {choices}")
        return matches[0]
