"""Optional catalog sharing; integrations call save and then event-driven sync.

LocalCatalog has no Git dependency. GitCatalog is the file-backed sharing strategy;
no module here imports a harness, frontend or the legacy scratch-note CLI.
"""

from .envelope import decode_operation, encode_operation
from .local import LocalCatalog, RepairRequired, Result
from .policy import Policy, Timing

__all__ = ["LocalCatalog", "Policy", "Timing", "RepairRequired", "Result", "decode_operation", "encode_operation"]
