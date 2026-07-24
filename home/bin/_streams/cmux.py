"""CMUX projection support for scratch streams."""

from __future__ import annotations

import json
import os
import re
import subprocess
from dataclasses import dataclass
from urllib.parse import urlencode
from pathlib import Path
from typing import Any

from .config import OBSIDIAN_VAULT_NAME, WORKNOTES_ROOT
from .errors import StreamError
from .agent_sessions import latest_session_command
from .models import Stream
from .resolve import resolve_stream_ref
from .store import list_streams

STATUS_COLORS = {
    "active": "Green",
    "waiting": "Amber",
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


def stream_for_workspace_hint(workspace: CmuxWorkspace, *, root: Path | None = None) -> Stream:
    """Infer a stream from weak CMUX workspace hints."""
    root_args = {"root": root} if root is not None else {}
    if workspace.title:
        try:
            return resolve_stream_ref(workspace.title, **root_args)
        except StreamError:
            pass

    cwd_matches = [
        stream
        for stream in list_streams(**root_args)
        if str(stream.frontmatter.get("cwd") or "") == workspace.current_directory
    ]
    if len(cwd_matches) == 1:
        return cwd_matches[0]
    if len(cwd_matches) > 1:
        choices = ", ".join(sorted(stream.slug for stream in cwd_matches))
        raise StreamError(
            f"current CMUX workspace matches multiple streams by cwd; pass a slug: {choices}"
        )
    raise StreamError(
        f"current CMUX workspace {workspace.display_ref} title {workspace.title!r} "
        "does not identify one stream; pass a slug"
    )


def resume_command(stream: Stream) -> str | None:
    command = latest_session_command(stream)
    return command.command_text if command else None


GITHUB_ITEM_RE = re.compile(r"^https://github\.com/([^/]+)/([^/]+)/(issues|pull)/([0-9]+)(?:[/?#].*)?$")


def markdown_link_for_item(item: dict[str, str], label: str) -> str:
    url = item["url"]
    match = GITHUB_ITEM_RE.fullmatch(url)
    if not match:
        return url
    number = match.group(4)
    return f"[{label}#{number}]({url}): {item['description']}"


def obsidian_link_for_stream(stream: Stream) -> str:
    try:
        file = stream.path.relative_to(WORKNOTES_ROOT).as_posix()
    except ValueError as exc:
        raise StreamError(f"stream {stream.slug!r} is outside worknotes root {WORKNOTES_ROOT}") from exc
    query = urlencode({"vault": OBSIDIAN_VAULT_NAME, "file": file})
    return f"[🔗](obsidian://open?{query})"


def description_for_stream(stream: Stream) -> str:
    lines = [f"{obsidian_link_for_stream(stream)} _{stream.state}_"]
    for field, label in (("issues", "I"), ("prs", "PR")):
        for item in stream.frontmatter.get(field) or []:
            lines.append(markdown_link_for_item(item, label))
    return "\n".join(lines)


def stream_cwd(stream: Stream) -> Path:
    path = Path(str(stream.frontmatter.get("cwd") or "")).expanduser()
    if not path.exists():
        raise StreamError(f"stream {stream.slug!r} cwd does not exist: {path}")
    if not path.is_dir():
        raise StreamError(f"stream {stream.slug!r} cwd is not a directory: {path}")
    return path


def open_workspace(stream: Stream, cmux: CmuxClient | None = None) -> None:
    cmux = cmux or CmuxClient()
    cwd = stream_cwd(stream)
    command = [
        "cmux",
        "new-workspace",
        "--name",
        stream.title,
        "--description",
        description_for_stream(stream),
        "--cwd",
        str(cwd),
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
            description_for_stream(stream),
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


def update_current_workspace(stream: Stream, cmux: CmuxClient | None = None) -> CmuxWorkspace:
    cmux = cmux or CmuxClient()
    workspace = cmux.resolve_workspace(None)
    update_workspace(stream, workspace, cmux)
    return workspace


def update_all_workspaces(
    cmux: CmuxClient,
    *,
    root: Path | None = None,
) -> tuple[list[tuple[CmuxWorkspace, Stream]], list[str]]:
    updated: list[tuple[CmuxWorkspace, Stream]] = []
    warnings: list[str] = []
    for workspace in cmux.workspaces():
        if workspace.index == 0 and workspace.title == "Tasks":
            continue
        try:
            stream = stream_for_workspace_hint(workspace, root=root)
        except StreamError:
            warnings.append(f"Warning: {workspace.display_ref} {workspace.title!r} has no matching scratch stream")
            continue
        update_workspace(stream, workspace, cmux)
        updated.append((workspace, stream))
    return updated, warnings


def load_stream_for_update(
    slug: str | None,
    workspace_selector: str | None,
    current: bool,
    cmux: CmuxClient,
    *,
    root: Path | None = None,
) -> tuple[Stream, CmuxWorkspace]:
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
            return resolve_stream_ref(slug, **({"root": root} if root is not None else {})), workspace
        return stream_for_workspace_hint(workspace, root=root), workspace
    if slug is None and workspace_selector is None:
        workspace = cmux.resolve_workspace(None)
        return stream_for_workspace_hint(workspace, root=root), workspace
    if workspace_selector is None:
        raise StreamError("update requires --current or --workspace")
    workspace = cmux.resolve_workspace(workspace_selector)
    if slug is not None:
        return resolve_stream_ref(slug, **({"root": root} if root is not None else {})), workspace
    return stream_for_workspace_hint(workspace, root=root), workspace
