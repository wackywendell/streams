"""Local file-store coordination and durable, fail-closed transport bookkeeping."""

from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import tempfile

from ..model import Host, Session, Stream, ValidationError
from ..operations import Authority, Operation, Reconciliation, reconcile
from ..storage import Snapshot, StorageError
from ..yaml_store import YamlStore
from .envelope import decode_operation, encode_operation, load_json


@dataclass(frozen=True)
class Result:
    sharing: str
    local: str = "unchanged"
    domain: Reconciliation | None = None
    local_revision: str | None = None
    remote_revision: str | None = None
    detail: str = ""
    conflicts: tuple[dict, ...] = ()


class RepairRequired(StorageError):
    pass


class State:
    """Private state is never staged. Atomic replacement also syncs the directory."""

    def __init__(self, root: Path):
        self.root = root / ".catalog-sync"

    def _directory(self):
        if self.root.is_symlink():
            raise RepairRequired("symlinked transport state")
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)

    def read(self, name: str):
        self._directory()
        path = self.root / name
        if path.is_symlink():
            raise RepairRequired(f"symlinked transport state: {path}")
        if not path.exists():
            return None
        try:
            data = load_json(path.read_text())
            if not isinstance(data, dict) or type(data.get("version")) is not int or data["version"] != 1:
                raise ValueError("unsupported transport state version")
            return data
        except (ValueError, OSError) as error:
            raise RepairRequired(f"cannot read {path}: {error}") from error

    def write(self, name: str, data: dict):
        import json
        self._directory()
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(dir=self.root, delete=False) as handle:
                temporary = Path(handle.name)
                handle.write(json.dumps({**data, "version": 1}, sort_keys=True, allow_nan=False).encode())
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, self.root / name)
            self._sync()
        finally:
            if temporary is not None and temporary.exists():
                temporary.unlink()

    def clear(self, name: str):
        (self.root / name).unlink(missing_ok=True)
        self._sync()

    def _sync(self):
        descriptor = os.open(self.root, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)

    def _receipts(self) -> dict:
        data = self.read("receipts.json") or {"operations": {}}
        operations = data.get("operations")
        if (not isinstance(operations, dict) or
                any(not isinstance(key, str) or not isinstance(value, str) or len(value) != 64
                    or any(c not in "0123456789abcdef" for c in value) for key, value in operations.items())):
            raise RepairRequired("invalid operation retry receipts")
        return operations

    @staticmethod
    def _digest(operation: Operation) -> str:
        return hashlib.sha256(encode_operation(operation).encode()).hexdigest()

    def seen(self, operation: Operation) -> bool:
        digest = self._receipts().get(operation.id)
        if digest is not None and digest != self._digest(operation):
            raise RepairRequired(f"operation ID reused with different intent: {operation.id}")
        return digest is not None

    def remember(self, operation: Operation):
        self.validate_operations((operation,), remember=True)

    def validate_operations(self, intents, *, remember: bool = False):
        """Compare a validated history in one pass before accepting or retiring it."""
        operations = self._receipts()
        changed = False
        for operation in intents:
            digest = self._digest(operation)
            previous = operations.get(operation.id)
            if previous is not None and previous != digest:
                raise RepairRequired(f"operation ID reused with different intent: {operation.id}")
            if previous is None:
                operations[operation.id] = digest
                changed = True
        if remember and changed:
            self.write("receipts.json", {"operations": operations})

    def check_save(self):
        marker = self.read("save.json")
        if marker is not None:
            raise RepairRequired(f"interrupted save; inspect {self.root / 'save.json'} before further writes")


def check_producer(operation: Operation, host_id: str):
    record = operation.after
    if isinstance(record, (Session, Host)):
        producer = record.host_id if isinstance(record, Session) else record.id
        if producer != host_id:
            raise ValidationError("cannot write another host's producer-owned record")
    if isinstance(record, Stream) and operation.before is None and record.owner_host_id != host_id:
        raise ValidationError("new streams must belong to the creating host")


def authority_for(operation: Operation, authorities: dict[str, Authority]) -> Authority | None:
    return authorities.get(operation.after.stream_id) if isinstance(operation.after, Session) else None


class LocalCatalog:
    """Local-only YAML composition. No Git executable or network is required."""

    def __init__(self, store: YamlStore, *, host_id: str):
        from ..model import text
        text(host_id, "host_id")
        self.store, self.host_id = store, host_id
        self.state = State(store.root)

    def _creations(self) -> dict[str, Authority]:
        state = self.state.read("local.json") or {"creations": {}}
        try:
            result = {}
            for stream_id, message in state["creations"].items():
                operation, _ = decode_operation(message)
                if operation.before is not None or not isinstance(operation.after, Stream) or operation.after.id != stream_id:
                    raise ValueError("invalid local creation receipt")
                result[stream_id] = Authority(operation.after.owner_host_id, operation.id)
            return result
        except (KeyError, AttributeError, ValueError) as error:
            raise RepairRequired(f"invalid local creation receipts: {error}") from error

    def snapshot(self) -> Snapshot:
        with self.store.transaction() as transaction:
            self.state.check_save()
            return transaction.read()

    def authority(self, stream_id: str) -> Authority | None:
        with self.store.transaction() as transaction:
            self.state.check_save()
            catalog = transaction.read().catalog
            authorities = self._creations()
            stream = catalog.stream(stream_id)
            return authorities.get(stream_id) if stream is not None else None

    def save(self, operation: Operation, *, origin: dict[str, str] | None = None) -> Result:
        check_producer(operation, self.host_id)
        try:
            with self.store.transaction() as transaction:
                self.state.check_save()
                snapshot = transaction.read()
                if self.state.seen(operation):
                    return Result("local-only", domain=Reconciliation(operation.id, "no-change", reason="operation already saved"))
                result = reconcile(snapshot.catalog, operation, authority=authority_for(operation, self._creations()))
                if result.outcome != "changes":
                    if result.outcome == "no-change":
                        self.state.remember(operation)
                    return Result("local-only", domain=result)
                self.state.write("save.json", {"operation": encode_operation(operation, origin=origin)})
                if operation.before is None and isinstance(operation.after, Stream):
                    receipts = self.state.read("local.json") or {"creations": {}}
                    receipts["creations"][operation.after.id] = encode_operation(operation, origin=origin)
                    self.state.write("local.json", receipts)
                transaction.save(result.record, expected=snapshot)
                self.state.remember(operation)
                self.state.clear("save.json")
                return Result("local-only", local="persisted", domain=result)
        except (OSError, StorageError) as error:
            return Result("local-only", local="repair-required", detail=str(error))

    def sync(self, **kwargs) -> Result:
        return Result("local-only")

    def handover(self, *args, **kwargs):
        raise NotImplementedError("handover requires a domain coordinator and a shared backend")
