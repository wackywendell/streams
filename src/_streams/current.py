"""Fixed, caller-only dispatch for generic current-workspace actions."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from .cmux import CmuxClient, CmuxWorkspace, stream_for_workspace_hint as infer_cmux_stream, update_workspace as update_cmux_workspace
from .frontend import Frontend
from .herdr import HerdrClient, HerdrWorkspace, stream_for_workspace as infer_herdr_stream, update_workspace as update_herdr_workspace
from .models import Stream


@dataclass(frozen=True)
class CurrentCmux:
    client: CmuxClient
    workspace: CmuxWorkspace

    @property
    def ref(self) -> str:
        return self.workspace.display_ref

    def infer_stream(self, *, root: Path | None = None) -> Stream:
        return infer_cmux_stream(self.workspace, root=root)

    def update(self, stream: Stream) -> None:
        update_cmux_workspace(stream, self.workspace, self.client)


@dataclass(frozen=True)
class CurrentHerdr:
    client: HerdrClient
    workspace: HerdrWorkspace

    @property
    def ref(self) -> str:
        return self.workspace.id

    def infer_stream(self, *, root: Path | None = None) -> Stream:
        return infer_herdr_stream(self.workspace, self.client, root=root)

    def update(self, stream: Stream) -> None:
        update_herdr_workspace(stream, self.workspace, self.client)


CurrentTarget = CurrentCmux | CurrentHerdr


def current_target(frontend: Frontend) -> CurrentTarget:
    if frontend == Frontend.CMUX:
        cmux = CmuxClient()
        return CurrentCmux(cmux, cmux.resolve_workspace(None))
    herdr = HerdrClient()
    return CurrentHerdr(herdr, herdr.resolve_workspace(None))
