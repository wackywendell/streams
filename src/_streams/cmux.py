"""CMUX workspace transport."""

from __future__ import annotations

import json
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

