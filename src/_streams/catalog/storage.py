"""Storage contracts and composition helpers; no YAML, paths or Git requirements."""

from __future__ import annotations

from dataclasses import dataclass
from types import MappingProxyType
from typing import ContextManager, Mapping, Protocol

from .model import Catalog, Record, RecordKey
from .operations import Authority, Operation, Reconciliation, reconcile


class StorageError(Exception):
    """Persistence failure. uncertain=True means the write may have taken effect.

    Reread before retrying; a confirmation failure is not proof of an unsaved write.
    """

    def __init__(self, message: str, *, uncertain: bool = False):
        super().__init__(message)
        self.uncertain = uncertain


class RevisionConflict(StorageError):
    """The snapshot used to prepare a save no longer matches storage."""


@dataclass(frozen=True)
class Snapshot:
    """Immutable records and opaque storage revisions, not progress-verification times."""

    catalog: Catalog
    revisions: Mapping[RecordKey, str]

    def __post_init__(self) -> None:
        object.__setattr__(self, "revisions", MappingProxyType(dict(self.revisions)))


class Transaction(Protocol):
    """Serialized scope, not an implicit rollback or multi-record transaction.

    Read, reconcile and save within this scope. save() atomically replaces one
    record, checks the complete snapshot, validates relationships and returns a
    new snapshot. A stale snapshot fails even if its logical values changed back.
    Even an unrelated record change invalidates the captured snapshot: reread and
    reconcile rather than force a stale save. No-op saves retain revisions. Adapters
    must refuse use after scope exit. Low-level save is a trusted persistence
    primitive; callers must apply domain operations and approval checks first.

    A Git coordinator can hold this scope through persistence AND local commit.
    It must supply its own journal/recovery for failures between those steps;
    file replacement alone must not be advertised as a Git-backed successful save.
    """

    def read(self) -> Snapshot: ...

    def save(self, record: Record, *, expected: Snapshot) -> Snapshot: ...


class Store(Protocol):
    def transaction(self) -> ContextManager[Transaction]: ...


def apply_operation(transaction: Transaction, operation: Operation, *, authority: Authority | None = None
                    ) -> tuple[Reconciliation, Snapshot]:
    """Compose domain reconciliation and local persistence, without publication claims.

    The caller supplies confirmed authority and owns operation journaling. Git
    transport must retain the original Operation for replay, not derive it from
    the resulting record or create a new operation ID on retry.
    """
    snapshot = transaction.read()
    result = reconcile(snapshot.catalog, operation, authority=authority)
    if result.outcome == "changes":
        snapshot = transaction.save(result.record, expected=snapshot)
    return result, snapshot
