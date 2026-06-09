"""Shared AI session command rendering for local workflow tools."""

from __future__ import annotations

import shlex
from dataclasses import dataclass
from enum import Enum


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
