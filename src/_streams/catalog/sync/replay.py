"""One domain replay rule for publication candidates and non-publishing read views."""

from dataclasses import dataclass

from ..model import Catalog, Record, Stream
from ..operations import Authority, Operation, Reconciliation, reconcile
from .history import History
from .local import RepairRequired, authority_for


@dataclass(frozen=True)
class Replay:
    catalog: Catalog
    changes: tuple[tuple[Operation, dict[str, str], Record], ...] = ()
    conflicts: tuple[dict, ...] = ()


def replay(local: History, remote: History) -> Replay:
    catalog = remote.catalog
    authorities = dict(remote.authorities)
    changes = []
    for operation, origin in local.operations.values():
        published = remote.operations.get(operation.id)
        if published is not None:
            if published[0] != operation:
                raise RepairRequired(f"remote operation ID has different intent: {operation.id}")
            continue
        # Field equality cannot turn another creation into local ownership.
        if operation.before is None and isinstance(operation.after, Stream) and remote.catalog.stream(operation.after.id):
            result = Reconciliation(operation.id, "conflict", conflicts=("identity",), reason="remote stream has a different creation")
        else:
            result = reconcile(catalog, operation, authority=authority_for(operation, authorities))
        if result.outcome == "conflict":
            conflict = {"operation": operation.id, "target": operation.target, "origin": origin,
                        "fields": result.conflicts, "reason": result.reason}
            return Replay(catalog, tuple(changes), (conflict,))
        if result.outcome != "changes":
            continue
        catalog = catalog.replacing(result.record)
        changes.append((operation, origin, result.record))
        if operation.before is None and isinstance(operation.after, Stream):
            authorities[operation.after.id] = Authority(operation.after.owner_host_id, operation.id)
    return Replay(catalog, tuple(changes))
