"""Frontend selection policy, independent of native clients and workspace targeting."""

from __future__ import annotations

from dataclasses import dataclass
from enum import StrEnum
from typing import Literal, Mapping

from .errors import StreamError


class Frontend(StrEnum):
    CMUX = "cmux"
    HERDR = "herdr"


class FrontendMode(StrEnum):
    AUTO = "auto"
    CMUX = "cmux"
    HERDR = "herdr"
    NONE = "none"


FrontendRequest = Frontend | Literal["auto"]


@dataclass(frozen=True)
class FrontendSelection:
    configured: FrontendMode
    requested: FrontendRequest | None
    resolved: Frontend | None

    def as_json(self) -> dict[str, str | None]:
        return {"configured": self.configured, "requested": self.requested, "resolved": self.resolved}


def resolve_frontend(configured: FrontendMode, requested: FrontendRequest | None, context: Mapping[str, str]) -> FrontendSelection:
    policy = configured if requested in (None, "auto") else FrontendMode(requested)
    resolved = None
    if policy == FrontendMode.AUTO:
        cmux = bool(context.get("CMUX_WORKSPACE_ID"))
        herdr = context.get("HERDR_ENV") == "1"
        pane = bool(context.get("HERDR_PANE_ID"))
        if herdr != pane:
            raise StreamError("incomplete Herdr caller context; expected HERDR_ENV=1 and HERDR_PANE_ID")
        if cmux and herdr:
            raise StreamError("both CMUX and Herdr caller contexts are present; choose --frontend cmux or --frontend herdr", kind="ambiguous", candidates=["cmux", "herdr"])
        resolved = Frontend.CMUX if cmux else Frontend.HERDR if herdr else None
    elif policy != FrontendMode.NONE:
        resolved = Frontend(policy)
    return FrontendSelection(configured, requested, resolved)
