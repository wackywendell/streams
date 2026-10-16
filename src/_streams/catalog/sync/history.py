"""Validate materialized Git history against the original domain operations."""

from dataclasses import dataclass, field

from ..model import Catalog, Stream, record_key
from ..operations import Authority, Operation, reconcile
from ..yaml_store import YamlStore
from ..storage import StorageError
from .envelope import BOOTSTRAP, decode_operation
from .git_io import Git
from .local import RepairRequired, authority_for


def records(catalog: Catalog) -> dict:
    return {record_key(record): record for record in catalog.records}


@dataclass
class History:
    tip: str | None = None
    root: str | None = None
    catalog: Catalog = field(default_factory=Catalog)
    files: dict[str, bytes] = field(default_factory=dict)
    authorities: dict[str, Authority] = field(default_factory=dict)
    operations: dict[str, tuple[Operation, dict[str, str]]] = field(default_factory=dict)


def read_history(git: Git, tip: str | None) -> History:
    try:
        return _read_history(git, tip)
    except (ValueError, StorageError) as error:
        raise RepairRequired(f"unsupported catalog history: {error}") from error


def _read_history(git: Git, tip: str | None) -> History:
    history = History(tip=tip)
    if tip is None:
        return history
    previous = None
    for line in git.text("rev-list", "--reverse", "--topo-order", "--parents", tip).splitlines():
        revision, *parents = line.split()
        files = git.files(revision)
        message = git.run("show", "-s", "--format=%B", revision).decode().rstrip() + "\n"
        if previous is None:
            if parents or files or message != BOOTSTRAP:
                raise RepairRequired("catalog history must start with an empty Streams catalog v1 commit")
            history.root = revision
        else:
            if parents != [previous]:
                raise RepairRequired("catalog history must be linear and non-merged")
            operation, origin = decode_operation(message)
            if operation.id in history.operations:
                raise RepairRequired(f"duplicate operation ID in history: {operation.id}")
            result = reconcile(history.catalog, operation, authority=authority_for(operation, history.authorities))
            if result.outcome != "changes":
                raise RepairRequired(f"operation {operation.id} does not explain its commit: {result.outcome}")
            actual = YamlStore.decode_snapshot(files).catalog
            expected = history.catalog.replacing(result.record)
            changes = {name for name in files.keys() | history.files.keys() if files.get(name) != history.files.get(name)}
            path = YamlStore.record_path(operation.target).as_posix()
            if records(actual) != records(expected) or changes != {path}:
                raise RepairRequired(f"commit changes do not match operation {operation.id}")
            if isinstance(operation.after, Stream) and operation.before is None:
                history.authorities[operation.after.id] = Authority(operation.after.owner_host_id, operation.id)
            history.catalog = actual
            history.operations[operation.id] = (operation, origin)
        history.files = files
        previous = revision
    return history
