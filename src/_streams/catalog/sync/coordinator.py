"""CLI/application entry points for opportunistic publication and blocking reads.

Complete related local saves before calling maybe_sync once. Read views never
publish, and their projected catalog is not an opaque persistence/CAS snapshot.
"""

from dataclasses import dataclass
from typing import Literal, TYPE_CHECKING

from ..model import Catalog
from ..storage import Snapshot
from .local import Result

if TYPE_CHECKING:
    from .git import GitCatalog
    from .local import LocalCatalog


@dataclass(frozen=True)
class ReadResult:
    catalog: Catalog
    local: Snapshot
    confirmed: Snapshot | None
    freshness: Literal["local-only", "cached", "refreshed", "failed"]
    fetched_at: float | None = None
    conflicts: tuple[dict, ...] = ()
    detail: str = ""


class SyncCoordinator:
    def __init__(self, backend: "GitCatalog | LocalCatalog"):
        self.backend = backend

    def maybe_sync(self, event: str) -> Result:
        """Check shared M/N gates; explicit/force/shutdown bypass timing only."""
        return self.backend.sync(event=event)

    def read(self, *, refresh: bool = False) -> ReadResult:
        """Refresh under N or explicit override, then project remote + local intent.

        A conflict keeps the complete local catalog as the primary view and
        exposes the confirmed alternative and diagnostics separately. No mixed,
        potentially invalid record graph is presented as a saved snapshot.
        """
        return self.backend.read(refresh=refresh)
