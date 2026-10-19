"""Validate materialized Git history against the original domain operations."""

from dataclasses import dataclass, field, replace
from types import MappingProxyType
from typing import Mapping

from ..model import Catalog, Stream, record_key
from ..operations import Authority, Operation, reconcile
from ..yaml_store import YamlStore
from ..storage import StorageError
from .baseline import BASELINE, Baseline, decode_baseline
from .envelope import BOOTSTRAP, decode_fingerprints, decode_operation, operation_digest
from .git_io import Git
from .local import RepairRequired, authority_for


def records(catalog: Catalog) -> dict:
    return {record_key(record): record for record in catalog.records}


@dataclass(frozen=True)
class History:
    tip: str | None = None
    root: str | None = None
    catalog: Catalog = field(default_factory=Catalog)
    files: Mapping[str, bytes] = field(default_factory=dict)
    authorities: Mapping[str, Authority] = field(default_factory=dict)
    operations: Mapping[str, tuple[Operation, Mapping[str, str]]] = field(default_factory=dict)
    baseline: Baseline | None = None
    fingerprints: Mapping[str, str] = field(default_factory=dict)

    def __post_init__(self) -> None:
        fingerprints = dict(self.baseline.fingerprints) if self.baseline is not None else {}
        for id, digest in decode_fingerprints(dict(self.fingerprints)).items():
            if id in fingerprints and fingerprints[id] != digest:
                raise ValueError(f"inconsistent operation fingerprint: {id}")
            fingerprints[id] = digest
        for id, (operation, _) in self.operations.items():
            if id != operation.id:
                raise ValueError("history operation key must match its ID")
            digest = operation_digest(operation)
            if id in fingerprints and fingerprints[id] != digest:
                raise ValueError(f"inconsistent operation fingerprint: {id}")
            fingerprints[id] = digest
        object.__setattr__(self, "fingerprints", MappingProxyType(fingerprints))
        object.__setattr__(self, "files", MappingProxyType(dict(self.files)))
        object.__setattr__(self, "authorities", MappingProxyType(dict(self.authorities)))
        object.__setattr__(self, "operations", MappingProxyType({
            id: (operation, MappingProxyType(dict(origin)))
            for id, (operation, origin) in self.operations.items()
        }))


def read_history(git: Git, tip: str | None) -> History:
    try:
        return _read_history(git, tip)
    except (ValueError, StorageError) as error:
        raise RepairRequired(f"unsupported catalog history: {error}") from error


def _read_history(git: Git, tip: str | None) -> History:
    if tip in git.histories:
        return git.histories[tip]
    history = History()
    if tip is None:
        return history
    entries = git.run("log", "-z", "--reverse", "--topo-order", "--format=%H%x00%P%x00%B", tip).decode().split("\0")
    if entries[-1] == "":
        entries.pop()
    if len(entries) % 3:
        raise RepairRequired("invalid Git commit metadata")
    commits = list(zip(entries[::3], entries[1::3], entries[2::3]))
    start, previous = 0, None
    for index, (revision, _, _) in enumerate(commits):
        if revision in git.histories:
            history = git.histories[revision]
            start, previous = index + 1, revision
    for revision, parent_text, raw_message in commits[start:]:
        parents = parent_text.split()
        files = git.files(revision)
        message = raw_message.rstrip() + "\n"
        if previous is None:
            if parents:
                raise RepairRequired("catalog history root must have no parents")
            if message.startswith(BASELINE):
                baseline, catalog, authorities = decode_baseline(message, files)
                history = History(tip=revision, root=revision, files=files, baseline=baseline,
                                  catalog=catalog, authorities=authorities)
            elif message == BOOTSTRAP and not files:
                history = History(tip=revision, root=revision, files=files)
            else:
                raise RepairRequired("catalog history must start with an empty bootstrap or validated baseline")
        else:
            if parents != [previous]:
                raise RepairRequired("catalog history must be linear and non-merged")
            operation, origin = decode_operation(message)
            if operation.id in history.fingerprints:
                raise RepairRequired(f"duplicate operation ID in history: {operation.id}")
            result = reconcile(history.catalog, operation, authority=authority_for(operation, history.authorities))
            if result.outcome != "changes":
                raise RepairRequired(f"operation {operation.id} does not explain its commit: {result.outcome}")
            changes = {name for name in files.keys() | history.files.keys() if files.get(name) != history.files.get(name)}
            path = YamlStore.record_path(operation.target).as_posix()
            if changes != {path} or path not in files:
                raise RepairRequired(f"commit changes do not match operation {operation.id}")
            actual = history.catalog.replacing(YamlStore.decode_record(path, files[path]))
            expected = history.catalog.replacing(result.record)
            if records(actual) != records(expected):
                raise RepairRequired(f"commit changes do not match operation {operation.id}")
            authorities = dict(history.authorities)
            if isinstance(operation.after, Stream) and operation.before is None:
                authorities[operation.after.id] = Authority(operation.after.owner_host_id, operation.id)
            history = replace(history, tip=revision, catalog=actual, files=files, authorities=authorities,
                              operations={**history.operations, operation.id: (operation, origin)})
        previous = revision
    # Intermediate prefixes are validated but only explicitly requested tips
    # compete for cache space with HEAD and the confirmed publication tip.
    git.histories[tip] = history
    if len(git.histories) > 8:
        del git.histories[next(iter(git.histories))]
    return history
