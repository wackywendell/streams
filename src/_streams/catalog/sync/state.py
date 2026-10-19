"""Durable transport state: parse supported records before making effects."""

from dataclasses import asdict, dataclass
import math
import os
from pathlib import Path
import tempfile
from typing import ClassVar

from ..model import text
from ..operations import Operation
from ..storage import StorageError
from .envelope import decode_fingerprints, load_json, operation_digest


class RepairRequired(StorageError):
    pass


def _oid(value, *, nullable=False):
    if value is None and nullable:
        return
    if not isinstance(value, str) or len(value) not in (40, 64) or any(c not in "0123456789abcdef" for c in value):
        raise ValueError("expected a Git object ID")


def _timestamp(value, *, nullable=False):
    if value is None and nullable:
        return
    if type(value) not in (int, float) or not math.isfinite(value):
        raise ValueError("expected a finite timestamp")


@dataclass(frozen=True)
class SyncMetadata:
    confirmed: str | None = None
    last_sync: float | None = None
    last_push: float | None = None
    retry_after: float = 0
    fetched_at: float | None = None
    last_confirmed_attempt: str | None = None
    refresh_retry_after: float = 0
    refresh_error: str = ""

    def __post_init__(self):
        _oid(self.confirmed, nullable=True)
        _oid(self.last_confirmed_attempt, nullable=True)
        for value in (self.last_sync, self.last_push, self.fetched_at):
            _timestamp(value, nullable=True)
        for value in (self.retry_after, self.refresh_retry_after):
            _timestamp(value)
        if not isinstance(self.refresh_error, str):
            raise ValueError("expected a refresh error string")
        if self.last_confirmed_attempt is not None and self.last_push is None:
            raise ValueError("confirmation identity requires its scheduling baseline")


@dataclass(frozen=True)
class GitConfig:
    remote: str | None
    branch: str
    host: str

    def __post_init__(self):
        if self.remote is not None:
            text(self.remote, "remote")
        for name in ("branch", "host"):
            text(getattr(self, name), name)


@dataclass(frozen=True, kw_only=True)
class PublicationAttempt:
    base: str | None
    candidate: str
    local_tip: str
    branch: str
    remote: str
    operations: tuple[str, ...]
    confirmed_at: float | None = None
    phase: ClassVar[str] = "attempting"

    def __post_init__(self):
        _oid(self.base, nullable=True)
        _oid(self.candidate)
        _oid(self.local_tip)
        text(self.branch, "branch")
        text(self.remote, "remote")
        _timestamp(self.confirmed_at, nullable=True)
        if not isinstance(self.operations, tuple):
            raise ValueError("operation identities must be a tuple")
        for identity in self.operations:
            text(identity, "operation identity")
        if len(set(self.operations)) != len(self.operations):
            raise ValueError("duplicate publication operation identities")

    def adopting(self, old: str, new: str) -> "AdoptingAttempt":
        return AdoptingAttempt(**asdict(self), adoption_from=old, adoption_to=new)


@dataclass(frozen=True, kw_only=True)
class AdoptingAttempt(PublicationAttempt):
    adoption_from: str
    adoption_to: str
    phase: ClassVar[str] = "adopting"

    def __post_init__(self):
        super().__post_init__()
        _oid(self.adoption_from)
        _oid(self.adoption_to)


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

    def _parse(self, name, model, data, *, required=()):
        try:
            values = {key: value for key, value in data.items() if key != "version"}
            if set(required) - values.keys():
                raise ValueError("missing transport state fields")
            if "operations" in values:
                if not isinstance(values["operations"], list):
                    raise ValueError("publication operations must be a list")
                values["operations"] = tuple(values["operations"])
            return model(**values)
        except (TypeError, ValueError, OverflowError) as error:
            raise RepairRequired(f"invalid {self.root / name}: {error}") from error

    def metadata(self) -> SyncMetadata:
        data = self.read("sync.json")
        if data is None:
            return SyncMetadata()
        return self._parse("sync.json", SyncMetadata, data,
                           required=("confirmed", "last_sync", "last_push", "retry_after", "fetched_at"))

    def write_metadata(self, metadata: SyncMetadata):
        self.write("sync.json", asdict(metadata))

    def config(self) -> GitConfig | None:
        data = self.read("config.json")
        return None if data is None else self._parse("config.json", GitConfig, data)

    def write_config(self, config: GitConfig):
        self.write("config.json", asdict(config))

    def publication(self) -> PublicationAttempt | AdoptingAttempt | None:
        data = self.read("publication.json")
        if data is None:
            return None
        phase = data.pop("phase", None)
        if phase not in ("attempting", "adopting"):
            raise RepairRequired("invalid publication phase; inspect publication.json")
        model = PublicationAttempt if phase == "attempting" else AdoptingAttempt
        return self._parse("publication.json", model, data)

    def write_publication(self, attempt: PublicationAttempt):
        self.write("publication.json", {**asdict(attempt), "phase": attempt.phase})

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

    def fingerprints(self) -> dict[str, str]:
        data = self.read("receipts.json") or {"operations": {}}
        try:
            if set(data) - {"version", "operations"}:
                raise ValueError("unexpected retry receipt fields")
            return decode_fingerprints(data.get("operations"))
        except ValueError as error:
            raise RepairRequired(f"invalid operation retry receipts: {error}") from error

    @staticmethod
    def _digest(operation: Operation) -> str:
        return operation_digest(operation)

    def seen(self, operation: Operation) -> bool:
        digest = self.fingerprints().get(operation.id)
        if digest is not None and digest != self._digest(operation):
            raise RepairRequired(f"operation ID reused with different intent: {operation.id}")
        return digest is not None

    def remember(self, operation: Operation):
        self.validate_fingerprints({operation.id: self._digest(operation)}, remember=True)

    def validate_fingerprints(self, fingerprints, *, remember: bool = False):
        """Compare all history claims before persisting any missing retry evidence."""
        incoming = decode_fingerprints(dict(fingerprints))
        operations = self.fingerprints()
        for identity, digest in incoming.items():
            if identity in operations and operations[identity] != digest:
                raise RepairRequired(f"operation ID reused with different intent: {identity}")
        if remember and incoming.keys() - operations.keys():
            self.write("receipts.json", {"operations": {**operations, **incoming}})

    def check_save(self):
        marker = self.read("save.json")
        if marker is not None:
            raise RepairRequired(f"interrupted save; inspect {self.root / 'save.json'} before further writes")

    def check_local(self):
        self.check_save()
        if any(path.name not in {"cli.json", "local.json", "receipts.json", "save.json"} for path in self.root.iterdir()):
            raise RepairRequired("unexpected local catalog bookkeeping; strategy cannot be switched")
        self.fingerprints()
