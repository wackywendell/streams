"""Shared AI session command rendering for local workflow tools."""

from __future__ import annotations

import shlex
from dataclasses import dataclass
from enum import Enum

from .errors import StreamError
from .models import Stream


class Agent(str, Enum):
    CODEX = "codex"
    CLAUDE = "claude"
    PI = "pi"


@dataclass(frozen=True)
class AgentSessionCommand:
    agent: Agent
    session_id: str

    def __post_init__(self) -> None:
        if not isinstance(self.agent, Agent):
            raise ValueError(f"unsupported AI agent {self.agent!r}")
        if not self.session_id:
            raise ValueError("agent session id cannot be empty")

    @property
    def argv(self) -> tuple[str, ...]:
        if self.agent == Agent.CODEX:
            return ("codex", "--no-sandbox", "resume", self.session_id)
        if self.agent == Agent.CLAUDE:
            return ("claude", "--resume", self.session_id)
        if self.agent == Agent.PI:
            return ("pi", "--session", self.session_id)
        raise ValueError(f"unsupported AI agent {self.agent.value!r}")

    @property
    def command_text(self) -> str:
        return shlex.join(self.argv)


def command_from_session(session: str) -> AgentSessionCommand:
    agent, _, session_id = str(session).partition(":")
    if not session_id:
        raise StreamError(f"invalid session {session!r}; expected agent:id")
    try:
        return AgentSessionCommand(Agent(agent), session_id)
    except ValueError as exc:
        raise StreamError(str(exc)) from exc


def latest_session_command(stream: Stream) -> AgentSessionCommand | None:
    sessions = stream.frontmatter.get("sessions") or []
    if not sessions:
        return None
    return command_from_session(str(sessions[-1]))
