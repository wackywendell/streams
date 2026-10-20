"""Native Herdr transport and endpoint inspection."""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Any

from .errors import StreamError


@dataclass(frozen=True)
class HerdrEndpoint:
    socket: str
    session: str | None = None


@dataclass(frozen=True)
class HerdrWorkspace:
    id: str
    label: str
    cwd: str = ""

    @classmethod
    def parse(cls, value: object) -> HerdrWorkspace:
        data = _object(value, "workspace")
        label = data.get("label")
        if not isinstance(label, str):
            raise StreamError("Herdr returned an invalid workspace label")
        return cls(_id(data, "workspace_id"), label)


@dataclass(frozen=True)
class HerdrPane:
    id: str
    tab_id: str
    workspace_id: str
    cwd: str

    @classmethod
    def parse(cls, data: object) -> HerdrPane:
        pane = _object(data, "pane")
        foreground_cwd, shell_cwd = pane.get("foreground_cwd"), pane.get("cwd")
        if any(value is not None and not isinstance(value, str) for value in (foreground_cwd, shell_cwd)):
            raise StreamError("Herdr returned an invalid pane cwd")
        return cls(_id(pane, "pane_id"), _id(pane, "tab_id"), _id(pane, "workspace_id"), foreground_cwd or shell_cwd or "")


def _object(value: object, name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise StreamError(f"Herdr returned an unexpected {name} shape")
    return value


def _id(data: dict[str, Any], key: str) -> str:
    value = data.get(key)
    if not isinstance(value, str) or not value.strip():
        raise StreamError(f"Herdr returned an invalid {key}")
    return value


class HerdrClient:
    def __init__(self) -> None:
        self.context = dict(os.environ)
        self.binary = self.context.get("HERDR_BIN_PATH") or "herdr"

    def run(self, args: list[str]) -> subprocess.CompletedProcess[str]:
        try:
            result = subprocess.run(
                [self.binary, *args], env=self.context,
                text=True, capture_output=True, check=True, timeout=30,
            )
        except FileNotFoundError as exc:
            raise StreamError(f"missing executable: {self.binary}") from exc
        except subprocess.TimeoutExpired as exc:
            raise StreamError("Herdr command timed out; it may have completed, so inspect native state before retrying") from exc
        except subprocess.CalledProcessError as exc:
            detail = (exc.stderr or exc.stdout or "").strip()[-2000:]
            raise StreamError(f"Herdr {' '.join(args[:2])} failed: {detail}") from exc
        except (OSError, ValueError) as exc:
            raise StreamError(f"could not execute Herdr: {exc}") from exc
        if result.stderr:
            print(result.stderr, end="", file=sys.stderr)
        return result

    def json_from_command(self, args: list[str]) -> dict[str, Any]:
        try:
            data = json.loads(self.run(args).stdout)
        except ValueError as exc:
            raise StreamError("Herdr command did not return valid JSON") from exc
        data = _object(data, "response")
        if "error" in data:
            error = _object(data["error"], "error")
            raise StreamError(f"Herdr command failed: {str(error.get('message') or error.get('code') or 'unknown error')[-2000:]}")
        return data

    def command_result(self, args: list[str], expected: str) -> dict[str, Any]:
        result = _object(self.json_from_command(args).get("result"), "result")
        if result.get("type") != expected:
            raise StreamError(f"Herdr returned an unexpected result; expected {expected}")
        return result

    def endpoint(self) -> HerdrEndpoint:
        server = _object(self.json_from_command(["status", "--json"]).get("server"), "server status")
        if server.get("running") is not True:
            raise StreamError("Herdr server is not running")
        if server.get("compatible") is not True or server.get("endpoint_compatible") is not True:
            raise StreamError("Herdr server is not API-compatible; no automatic update or restart will be attempted")
        version = server.get("version")
        parsed = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(?:[-+][\w.-]+)?", version) if isinstance(version, str) else None
        if parsed is None or tuple(map(int, parsed.groups())) < (0, 9, 3):
            raise StreamError("streams Herdr projection requires server version 0.9.3 or newer")
        socket = _id(server, "socket")
        if not Path(socket).is_absolute():
            raise StreamError("Herdr returned an invalid server socket path")
        session = server.get("session")
        if session is not None and not isinstance(session, str):
            raise StreamError("Herdr returned an invalid server session")
        return HerdrEndpoint(socket, session)

    def current_pane(self) -> HerdrPane:
        if self.context.get("HERDR_ENV") != "1" or not self.context.get("HERDR_PANE_ID"):
            raise StreamError("current Herdr targeting requires caller context; pass --workspace outside Herdr")
        return HerdrPane.parse(self.command_result(["pane", "current", "--current"], "pane_current").get("pane"))

    def resolve_workspace(self, selector: str | None) -> HerdrWorkspace:
        if selector is None:
            pane = self.current_pane()
            workspace_id = pane.workspace_id
        else:
            if not selector.strip():
                raise StreamError("Herdr --workspace requires a nonempty workspace ID", kind="invalid-ref")
            pane = None
            workspace_id = selector
        workspace = HerdrWorkspace.parse(self.command_result(["workspace", "get", workspace_id], "workspace_info").get("workspace"))
        if workspace.id != workspace_id:
            raise StreamError("Herdr workspace ID mismatch in native response")
        return replace(workspace, cwd=pane.cwd) if pane is not None else workspace
