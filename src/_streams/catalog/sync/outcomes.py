"""Independent domain, local-save and publication facts for every caller."""

from dataclasses import dataclass
from typing import Literal

from ..operations import Reconciliation

LocalStatus = Literal["unchanged", "persisted", "committed", "repair-required"]
SharingStatus = Literal["local-only", "pending", "confirmed", "blocked", "failed", "uncertain"]


@dataclass(frozen=True)
class Result:
    sharing: SharingStatus
    local: LocalStatus = "unchanged"
    domain: Reconciliation | None = None
    local_revision: str | None = None
    remote_revision: str | None = None
    detail: str = ""
    conflicts: tuple[dict, ...] = ()

    def __post_init__(self):
        if self.local not in ("unchanged", "persisted", "committed", "repair-required"):
            raise ValueError("unsupported local-save outcome")
        if self.sharing not in ("local-only", "pending", "confirmed", "blocked", "failed", "uncertain"):
            raise ValueError("unsupported publication outcome")

    @property
    def accepted(self) -> bool:
        """A save was accepted; a publication result alone cannot prove that."""
        return not self.requires_repair and self.domain is not None and self.domain.outcome in ("changes", "no-change")

    @property
    def requires_repair(self) -> bool:
        return self.local == "repair-required"

    @property
    def needs_attention(self) -> bool:
        return self.requires_repair or self.sharing in ("blocked", "failed", "uncertain")
