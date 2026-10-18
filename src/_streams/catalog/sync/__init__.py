"""Optional catalog sharing; integrations call save and then event-driven sync.

LocalCatalog has no Git dependency. GitCatalog is the file-backed sharing strategy;
no module here imports a harness, frontend or the legacy scratch-note CLI.
"""

from .coordinator import ReadResult, SyncCoordinator
from .envelope import decode_operation, encode_operation
from .local import LocalCatalog
from .outcomes import Result
from .state import RepairRequired
from .git import GitCatalog
from .policy import Policy, Timing

__all__ = ["ReadResult", "SyncCoordinator", "GitCatalog", "LocalCatalog", "Policy", "Timing", "RepairRequired", "Result", "decode_operation", "encode_operation"]
