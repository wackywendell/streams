"""CMUX projection support for scratch streams."""

from __future__ import annotations

import json
import os
import subprocess
from dataclasses import dataclass
from typing import Any

from .errors import StreamError
from .agent_sessions import Agent, AgentSessionCommand
from .models import Stream
from .slug import normalized_slug, slugify
from .store import show_stream

STATUS_COLORS = {
    "active": "Green",
    "waiting": "Amber",
    "review": "Amber",
    "blocked": "Orange",
    "parked": "Red",
    "done": "Green",
}


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
    def display_ref(self) -> str:
        return self.ref or self.id or "<unknown>"


class CmuxClient:
    def run(self, args: list[str]) -> subprocess.CompletedProcess[str]:
        try:
            return subprocess.run(args, text=True, capture_output=True, check=True)
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
        return self.run(["cmux", "current-workspace"]).stdout.strip()

    def resolve_workspace(self, selector: str | None) -> CmuxWorkspace:
        selector = selector or self.current_workspace_ref()
        matches = [
            workspace
            for workspace in self.workspaces()
            if selector in {workspace.ref, workspace.id, workspace.title}
        ]
        if not matches:
            raise StreamError(f"could not find CMUX workspace {selector!r}")
        if len(matches) > 1:
            choices = ", ".join(f"{item.display_ref} {item.title}" for item in matches)
            raise StreamError(f"CMUX workspace {selector!r} is ambiguous: {choices}")
        return matches[0]


def color_for_status(status: str) -> str:
    return STATUS_COLORS.get(status, "Amber")


def stream_for_current_workspace(cmux: CmuxClient) -> tuple[Stream, CmuxWorkspace]:
    """Resolve both stream and workspace from the current CMUX title."""
    workspace = cmux.resolve_workspace(None)
    slug = slugify(workspace.title)
    if not slug:
        raise StreamError(f"current CMUX workspace title {workspace.title!r} does not produce a stream slug")
    return show_stream(slug), workspace


def resume_command(stream: Stream) -> str | None:
    sessions = stream.frontmatter.get("sessions") or []
    if not sessions:
        return None
    session = str(sessions[0])
    agent, _, session_id = session.partition(":")
    if not session_id:
        raise StreamError(f"invalid session {session!r}; expected agent:id")
    try:
        return AgentSessionCommand(Agent(agent), session_id).command_text
    except ValueError as exc:
        raise StreamError(str(exc)) from exc


def open_workspace(stream: Stream, cmux: CmuxClient | None = None) -> None:
    cmux = cmux or CmuxClient()
    command = [
        "cmux",
        "new-workspace",
        "--name",
        stream.title,
        "--description",
        stream.state,
        "--cwd",
        str(stream.frontmatter.get("cwd") or "."),
    ]
    if session_command := resume_command(stream):
        command.extend(["--command", session_command])
    cmux.run(command)


def update_workspace(stream: Stream, workspace: CmuxWorkspace, cmux: CmuxClient | None = None) -> None:
    cmux = cmux or CmuxClient()
    ref = workspace.display_ref
    cmux.run(["cmux", "workspace-action", "--workspace", ref, "--action", "rename", "--title", stream.title])
    cmux.run(
        [
            "cmux",
            "workspace-action",
            "--workspace",
            ref,
            "--action",
            "set-description",
            "--description",
            stream.state,
        ]
    )
    cmux.run(
        [
            "cmux",
            "workspace-action",
            "--workspace",
            ref,
            "--action",
            "set-color",
            "--color",
            color_for_status(stream.status),
        ]
    )


def load_stream_for_update(slug: str | None, workspace_selector: str | None, current: bool, cmux: CmuxClient) -> tuple[Stream, CmuxWorkspace]:
    """Resolve the stream/workspace pair for `streams cmux update`.

    An explicit slug always selects the stream. `--current` and `--workspace`
    select only the target CMUX workspace, so users can update a workspace whose
    current title does not yet match the desired stream.
    """
    if current and workspace_selector:
        raise StreamError("use either --current or --workspace, not both")
    if current:
        workspace = cmux.resolve_workspace(None)
        if slug is not None:
            return show_stream(normalized_slug(slug)), workspace
        stream, workspace = stream_for_current_workspace(cmux)
        return stream, workspace
    if slug is None and workspace_selector is None:
        stream, workspace = stream_for_current_workspace(cmux)
        return stream, workspace
    if workspace_selector is None:
        raise StreamError("update requires --current or --workspace")
    workspace = cmux.resolve_workspace(workspace_selector)
    return show_stream(normalized_slug(slug or workspace.title)), workspace
