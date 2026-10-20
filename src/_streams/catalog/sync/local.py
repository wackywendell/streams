"""Local file-store coordination with no Git or network dependency."""

from ..model import Host, Session, Stream, ValidationError
from ..operations import Authority, Operation, Reconciliation, reconcile
from ..storage import Snapshot, StorageError
from ..yaml_store import YamlStore
from .envelope import decode_operation, encode_operation
from .outcomes import ReadResult, Result
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


def read_context_records(
    transaction,
    *,
    host_id: str,
    authorities: dict[str, Authority],
    harness: str,
    native_id: str,
    stream_id: str | None = None,
    explicit_stream_id: str | None = None,
    workspace_stream_id: str | None = None,
    deterministic_id: str | None = None,
) -> tuple[Stream | None, Session | None, tuple[Session, ...], Authority | None]:
    """Select validated local context without scanning unrelated record contents."""
    from ..model import Catalog, text
    for identity in (stream_id, explicit_stream_id, workspace_stream_id):
        if identity is not None:
            text(identity, "stream_id")
    hint_stream = stream_id or explicit_stream_id or workspace_stream_id
    session = transaction.read_native_session(
        host_id, harness, native_id, stream_id=hint_stream, deterministic_id=deterministic_id
    )
    candidates = [v for v in (stream_id, explicit_stream_id, workspace_stream_id, session.stream_id if session else None) if v is not None]
    identities = set(candidates)
    if len(identities) > 1:
        from ...errors import StreamError
        raise StreamError("conversation already has a different effort; moving it requires an explicit move or fork operation")
    selected = next(iter(identities), None)
    if selected is None:
        return None, session, (), None
    stream = transaction.read_stream(selected)
    if stream is None:
        from ...errors import StreamError
        raise StreamError(f"stream {selected!r} is not in the local catalog; run streams sync to adopt remote-only records")
    sessions = transaction.read_stream_sessions(stream.id)
    if session is not None and all((s.host_id, s.id) != (session.host_id, session.id) for s in sessions):
        sessions = sessions + (session,)
    Catalog(streams=(stream,), sessions=sessions)
    return stream, session, sessions, authorities.get(stream.id)


class LocalCatalog:
    """Local-only YAML composition. No Git executable or network is required."""

    def __init__(self, store: YamlStore, *, host_id: str):
        from ..model import text
        text(host_id, "host_id")
        self.store, self.host_id = store, host_id
        self.state = State(store.root)
        self._cached_creations: tuple[tuple[int, int] | None, dict[str, Authority]] | None = None

    def _ready(self) -> dict[str, Authority]:
        self.state.check_save()
        allowed = {".catalog.lock", ".catalog-sync", ".catalog-staging", "streams", "sessions", "hosts"}
        if any(path.name not in allowed for path in self.store.root.iterdir()):
            raise RepairRequired("unexpected contents in local catalog; inspect before continuing")
        self.state.check_local()
        return self._creations()

    def _creations(self) -> dict[str, Authority]:
        local_path = self.store.root / ".catalog-sync" / "local.json"
        try:
            st = local_path.stat()
            stamp: tuple[int, int] | None = (st.st_mtime_ns, st.st_size)
        except FileNotFoundError:
            stamp = None
        if self._cached_creations is not None and self._cached_creations[0] == stamp:
            return dict(self._cached_creations[1])
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
            self._cached_creations = (stamp, dict(result))
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
            stream = transaction.read_stream(stream_id)
            return authorities.get(stream_id) if stream is not None else None

    def context_records(
        self,
        *,
        harness: str,
        native_id: str,
        stream_id: str | None = None,
        explicit_stream_id: str | None = None,
        workspace_stream_id: str | None = None,
        deterministic_id: str | None = None,
    ) -> tuple[Stream | None, Session | None, tuple[Session, ...], Authority | None]:
        with self.store.transaction() as transaction:
            authorities = self._ready()
            return read_context_records(
                transaction, host_id=self.host_id, authorities=authorities, harness=harness, native_id=native_id,
                stream_id=stream_id, explicit_stream_id=explicit_stream_id, workspace_stream_id=workspace_stream_id,
                deterministic_id=deterministic_id,
            )

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
        snapshot = self.snapshot()
        return ReadResult(snapshot.catalog, snapshot, None, "local-only")

    def sync(self, **kwargs) -> Result:
        try:
            self.snapshot()
            return Result("local-only")
        except (OSError, StorageError) as error:
            return Result("local-only", local="repair-required", detail=str(error))
