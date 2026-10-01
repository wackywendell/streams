"""Terminal resume support for scratch streams."""

from __future__ import annotations

import os
from pathlib import Path

from .agent_sessions import AgentSessionCommand, latest_session_command
from .cmux import CmuxClient, stream_cwd, stream_for_workspace_hint
from .errors import StreamError
from .models import Stream
from .resolve import resolve_stream_ref


def resolve_stream_for_resume(
    ref: str | None,
    *,
    cmux: CmuxClient | None = None,
    root: Path | None = None,
) -> Stream:
    root_args = {"root": root} if root is not None else {}
    if ref is not None:
        return resolve_stream_ref(ref, **root_args)

    cmux = cmux or CmuxClient()
    workspace = cmux.resolve_workspace(None)
    try:
        return stream_for_workspace_hint(workspace, root=root)
    except StreamError as exc:
        raise StreamError(f"could not infer stream to resume from current CMUX workspace: {exc}") from exc


def resume_command_for_stream(stream: Stream) -> AgentSessionCommand:
    command = latest_session_command(stream)
    if command is None:
        raise StreamError(f"stream {stream.slug!r} has no recorded AI sessions")
    return command


def exec_resume(stream: Stream) -> None:
    cwd = stream_cwd(stream)
    command = resume_command_for_stream(stream)
    os.chdir(cwd)
    os.execvp(command.argv[0], list(command.argv))
