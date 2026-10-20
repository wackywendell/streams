"""Shared stream command errors."""

from __future__ import annotations

from typing import Literal, Sequence

ErrorKind = Literal["not-found", "ambiguous", "invalid-ref", "error"]


class StreamError(Exception):
    """Raised for expected user-facing stream failures.

    `kind` lets `--json` callers tell failures apart without parsing messages:
    a caller may offer to create a stream on `not-found`, but not when the
    lookup itself failed.
    """

    def __init__(self, message: str, *, kind: ErrorKind = "error", candidates: Sequence[str] = ()) -> None:
        super().__init__(message)
        self.kind: ErrorKind = kind
        self.candidates = list(candidates)

    def as_json(self) -> dict[str, object]:
        payload: dict[str, object] = {"kind": self.kind, "message": str(self)}
        if self.candidates:
            payload["candidates"] = self.candidates
        return payload
