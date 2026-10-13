"""Terminal resume support for scratch streams."""

from __future__ import annotations

import os
from pathlib import Path

from .agent_sessions import AgentSessionCommand, latest_session_command
from .errors import StreamError
from .models import Stream
from .projection import stream_cwd
from .resolve import resolve_stream_ref


def resolve_stream_for_resume(ref: str, *, root: Path | None = None) -> Stream:
    """Resolve an explicit reference without consulting a frontend."""
    return resolve_stream_ref(ref, **({"root": root} if root is not None else {}))


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
