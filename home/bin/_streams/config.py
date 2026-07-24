"""Configuration and constants for scratch streams."""

from __future__ import annotations

from pathlib import Path

SCRATCH_ROOT = Path("~/worknotes/scratch").expanduser()
WORKNOTES_ROOT = SCRATCH_ROOT.parent
OBSIDIAN_VAULT_NAME = "worknotes"
ARCHIVE_DIRNAME = "archive"
VALID_STATUSES = frozenset({"active", "waiting", "blocked", "parked", "done"})
DEFAULT_STATUS = "active"
DEFAULT_STATE = "New stream ready for scoping."
DEFAULT_VCS = "jj"
