"""Local file-store coordination with no Git or network dependency."""

from ..model import Host, Session, Stream, ValidationError
from ..operations import Authority, Operation, Reconciliation, reconcile
from ..storage import Snapshot, StorageError
from ..yaml_store import YamlStore
from .envelope import decode_operation, encode_operation
from .outcomes import Result
from .state import RepairRequired, State


def check_producer(operation: Operation, host_id: str):
    record = operation.after
    if isinstance(record, (Session, Host)):
        producer = record.host_id if isinstance(record, Session) else record.id
        if producer != host_id:
            raise ValidationError("cannot write another host's producer-owned record")
    if isinstance(record, Stream) and (operation.before is None or operation.actor == "automatic") and record.owner_host_id != host_id:
        raise ValidationError("stream creation and inference require the owning host")


def authority_for(operation: Operation, authorities: dict[str, Authority]) -> Authority | None:
    if isinstance(operation.after, Session):
        return authorities.get(operation.after.stream_id)
    if isinstance(operation.after, Stream):
        return authorities.get(operation.after.id)
    return None


class LocalCatalog:
    """Local-only YAML composition. No Git executable or network is required."""

    def __init__(self, store: YamlStore, *, host_id: str):
        from ..model import text
        text(host_id, "host_id")
        self.store, self.host_id = store, host_id
        self.state = State(store.root)

    def _ready(self) -> dict[str, Authority]:
        self.state.check_save()
        allowed = {".catalog.lock", ".catalog-sync", ".catalog-staging", "streams", "sessions", "hosts"}
        if any(path.name not in allowed for path in self.store.root.iterdir()):
            raise RepairRequired("unexpected contents in local catalog; inspect before continuing")
        self.state.check_local()
        return self._creations()

    def _creations(self) -> dict[str, Authority]:
        state = self.state.read("local.json") or {"creations": {}}
        try:
            if set(state) - {"version", "creations"}:
                raise ValueError("unexpected creation receipt fields")
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
            self._ready()
            return transaction.read()

    def authority(self, stream_id: str) -> Authority | None:
        with self.store.transaction() as transaction:
            authorities = self._ready()
            catalog = transaction.read().catalog
            stream = catalog.stream(stream_id)
            return authorities.get(stream_id) if stream is not None else None

    def save(self, operation: Operation, *, origin: dict[str, str] | None = None) -> Result:
        check_producer(operation, self.host_id)
        try:
            with self.store.transaction() as transaction:
                authorities = self._ready()
                snapshot = transaction.read()
                if self.state.seen(operation):
                    return Result("local-only", domain=Reconciliation(operation.id, "no-change", reason="operation already saved"))
                result = reconcile(snapshot.catalog, operation, authority=authority_for(operation, authorities))
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

    def read(self, *, refresh: bool = False):
        from .coordinator import ReadResult
        snapshot = self.snapshot()
        return ReadResult(snapshot.catalog, snapshot, None, "local-only")

    def sync(self, **kwargs) -> Result:
        try:
            self.snapshot()
            return Result("local-only")
        except (OSError, StorageError) as error:
            return Result("local-only", local="repair-required", detail=str(error))

    def handover(self, *args, **kwargs):
        raise NotImplementedError("handover requires a domain coordinator and a shared backend")
