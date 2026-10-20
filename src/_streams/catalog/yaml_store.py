"""Versioned YAML storage with process serialization and atomic single-record saves.

No Git repository, network access or runtime integration is required. This adapter
requires Unix flock and atomic replacement. Records live in streams/<id>.yaml,
sessions/<host-id>/<id>.yaml and hosts/<id>.yaml, with escaped identity components.
Each file wraps the physical fields in a version/revision/kind/record envelope.

Snapshot revisions digest uniquely stamped bytes, so effective writes change the
revision even when logical values change back. Filesystem timestamps are not used.
The private catalog is tools-managed: malformed, unsupported, misplaced or symlinked
records and unexpected files in record directories fail closed.

Staging files live outside the record namespace in .catalog-staging; interruption
may leave an orphan without blocking reads or later saves. Replacement failure
preserves the old record; errors after replacement report an uncertain save.
"""

from __future__ import annotations

from contextlib import contextmanager
import fcntl
import hashlib
import os
from pathlib import Path
import tempfile
from uuid import uuid4

import yaml

from .codec import object_fields, record_from_data, record_to_data
from .model import Catalog, Host, Record, RecordKey, Session, Stream, ValidationError, record_key, text
from .storage import RevisionConflict, Snapshot, StorageError


class _StrictLoader(getattr(yaml, "CSafeLoader", yaml.SafeLoader)):
    pass


def _mapping(loader, node):
    result = {}
    for key_node, value_node in node.value:
        key = loader.construct_object(key_node, deep=True)
        if not isinstance(key, str) or key in result:
            raise ValidationError("YAML object keys must be unique strings")
        result[key] = loader.construct_object(value_node, deep=True)
    return result


_StrictLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _mapping)


def _component(id: str) -> str:
    # Encode uppercase and non-ASCII bytes too: logical IDs must remain distinct
    # on case-insensitive and Unicode-normalising filesystems.
    safe = b"abcdefghijklmnopqrstuvwxyz0123456789-_.~"
    encoded = "".join(chr(byte) if byte in safe else f"%{byte:02X}" for byte in id.encode("utf-8"))
    return encoded.replace(".", "%2E") if encoded in (".", "..") else encoded


def _path(root: Path, key: RecordKey) -> Path:
    name = f"{_component(key.id)}.yaml"
    if key.kind == "session":
        if key.stream_id is not None:
            return root / "sessions" / _component(key.stream_id) / _component(key.host_id) / name
        return root / "sessions" / _component(key.host_id) / name
    return root / ("streams" if key.kind == "stream" else "hosts") / name


def _directory(path: Path, *, create: bool = False) -> None:
    if path.is_symlink():
        raise StorageError(f"symlinked catalog directory: {path}")
    if create:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    if path.exists() and not path.is_dir():
        raise StorageError(f"expected catalog directory: {path}")


def _sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


class YamlStore:
    def __init__(self, root: Path):
        self.root = Path(root).absolute()

    @staticmethod
    def record_path(key: RecordKey) -> Path:
        """Canonical relative path for file-backed transport; identities stay logical."""
        return _path(Path(), key)

    @staticmethod
    def decode_record(path: str, contents: bytes) -> Record:
        """Decode one physical record, including its canonical identity location."""
        try:
            envelope = object_fields(yaml.load(contents, Loader=_StrictLoader), {"version", "revision", "kind", "record"})
            if type(envelope.get("version")) is not int or envelope["version"] != 1:
                raise ValidationError("unsupported catalog encoding version")
            text(envelope.get("revision"), "storage revision")
            record = record_from_data(envelope.get("kind"), envelope.get("record"))
            expected = _path(Path(), record_key(record)).as_posix()
            legacy = _path(Path(), RecordKey("session", record.id, record.host_id)).as_posix() if isinstance(record, Session) else expected
            if path not in (expected, legacy):
                raise ValidationError(f"record identity does not match its physical location: {path}")
            return record
        except (ValueError, yaml.YAMLError) as error:
            raise StorageError(f"cannot read catalog: {error}") from error

    @staticmethod
    def decode_snapshot(files: dict[str, bytes]) -> Snapshot:
        """Validate a Git tree or other file snapshot using this adapter's encoding."""
        records, revisions = [], {}
        try:
            for name, contents in sorted(files.items()):
                record = YamlStore.decode_record(name, contents)
                records.append(record)
                revisions[record_key(record)] = hashlib.sha256(contents).hexdigest()
            return Snapshot(catalog=Catalog(
                streams=tuple(item for item in records if isinstance(item, Stream)),
                sessions=tuple(item for item in records if isinstance(item, Session)),
                hosts=tuple(item for item in records if isinstance(item, Host))), revisions=revisions)
        except (ValueError, yaml.YAMLError) as error:
            raise StorageError(f"cannot read catalog: {error}") from error

    @contextmanager
    def transaction(self):
        """Hold the common process lock across read, reconciliation, save and composition.

        Do not nest scopes or pass a transaction to another thread. A Git adapter
        must use this same scope for all index and commit activity and exclude
        .catalog.lock and .catalog-staging/ from commits. Never unlink the lock
        while the store is in use; replacing its inode would split coordination.
        """
        try:
            _directory(self.root, create=True)
            lock = self.root / ".catalog.lock"
            descriptor = os.open(lock, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        except OSError as error:
            raise StorageError(f"cannot open catalog lock: {error}") from error
        transaction = _YamlTransaction(self.root)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            yield transaction
        finally:
            transaction.active = False
            os.close(descriptor)


class _YamlTransaction:
    def __init__(self, root: Path):
        self.root = root
        self.active = True

    def _check_active(self) -> None:
        if not self.active:
            raise RuntimeError("catalog transaction has ended")

    def _files(self):
        for name in ("streams", "sessions", "hosts"):
            directory = self.root / name
            _directory(directory)
            if not directory.exists():
                continue
            for path in sorted(directory.iterdir()):
                if name == "sessions":
                    _directory(path)
                    for child in sorted(path.iterdir()):
                        if child.is_dir() and not child.is_symlink():
                            _directory(child)
                            yield from self._record_files(child)
                        else:
                            yield self._record_file(child)
                else:
                    yield self._record_file(path)

    def _record_file(self, path: Path) -> Path:
        if path.is_symlink() or not path.is_file() or path.suffix != ".yaml":
            raise StorageError(f"unexpected catalog file: {path}")
        return path

    def _record_files(self, directory: Path):
        for path in sorted(directory.iterdir()):
            yield self._record_file(path)

    def _read_single_file(self, path: Path) -> Record | None:
        self._check_active()
        if path.is_symlink():
            raise StorageError(f"unexpected catalog file: {path}")
        if not path.exists():
            return None
        for parent in path.relative_to(self.root).parents:
            if str(parent) != ".":
                _directory(self.root / parent)
        self._record_file(path)
        try:
            rel = path.relative_to(self.root).as_posix()
            return YamlStore.decode_record(rel, path.read_bytes())
        except (OSError, ValueError, yaml.YAMLError) as error:
            raise StorageError(f"cannot read catalog: {error}") from error

    def read_stream(self, stream_id: str) -> Stream | None:
        record = self._read_single_file(_path(self.root, RecordKey("stream", stream_id)))
        return record if isinstance(record, Stream) else None

    def read_stream_sessions(self, stream_id: str) -> tuple[Session, ...]:
        self._check_active()
        sessions: dict[tuple[str, str], Session] = {}
        sessions_root = self.root / "sessions"
        _directory(sessions_root)
        if not sessions_root.exists():
            return ()
        stream_dir = sessions_root / _component(stream_id)
        _directory(stream_dir)
        if stream_dir.exists():
            for host_dir in sorted(stream_dir.iterdir()):
                if host_dir.is_dir() and not host_dir.is_symlink():
                    _directory(host_dir)
                    for path in self._record_files(host_dir):
                        rec = self._read_single_file(path)
                        if isinstance(rec, Session) and rec.stream_id == stream_id:
                            sessions[(rec.host_id, rec.id)] = rec
        # Also support any unmigrated 2-level legacy session files under sessions/<host>/<id>.yaml.
        for entry in sorted(sessions_root.iterdir()):
            _directory(entry)
            for child in sorted(entry.iterdir()):
                if child.suffix == ".yaml" and (child.is_file() or child.is_symlink()):
                    rec = self._read_single_file(child)
                    if isinstance(rec, Session) and rec.stream_id == stream_id:
                        sessions.setdefault((rec.host_id, rec.id), rec)
        return tuple(sessions.values())

    def read_native_session(
        self,
        host_id: str,
        harness: str,
        native_id: str,
        *,
        stream_id: str | None = None,
        deterministic_id: str | None = None,
    ) -> Session | None:
        self._check_active()
        sessions_root = self.root / "sessions"
        _directory(sessions_root)
        if not sessions_root.exists():
            return None
        host_comp = _component(host_id)
        if deterministic_id is not None:
            fname = f"{_component(deterministic_id)}.yaml"
            if stream_id is not None:
                direct = sessions_root / _component(stream_id) / host_comp / fname
                rec = self._read_single_file(direct)
                if isinstance(rec, Session) and rec.native_identity == (host_id, harness, native_id):
                    return rec
            for s_dir in sorted(sessions_root.iterdir()):
                _directory(s_dir)
                cand = s_dir / host_comp / fname
                if cand.exists() or cand.is_symlink():
                    rec = self._read_single_file(cand)
                    if isinstance(rec, Session) and rec.native_identity == (host_id, harness, native_id):
                        return rec
            legacy = sessions_root / host_comp / fname
            if legacy.exists() or legacy.is_symlink():
                rec = self._read_single_file(legacy)
                if isinstance(rec, Session) and rec.native_identity == (host_id, harness, native_id):
                    return rec
        # Fallback for unmigrated 2-level legacy session files or when deterministic_id is not supplied:
        legacy_host_dir = sessions_root / host_comp
        if legacy_host_dir.exists() or legacy_host_dir.is_symlink():
            _directory(legacy_host_dir)
            for child in sorted(legacy_host_dir.iterdir()):
                if child.suffix == ".yaml" and (child.is_file() or child.is_symlink()):
                    rec = self._read_single_file(child)
                    if isinstance(rec, Session) and rec.native_identity == (host_id, harness, native_id):
                        return rec
        if deterministic_id is None:
            for s_dir in sorted(sessions_root.iterdir()):
                _directory(s_dir)
                host_dir = s_dir / host_comp
                if host_dir.exists() or host_dir.is_symlink():
                    _directory(host_dir)
                    for path in self._record_files(host_dir):
                        rec = self._read_single_file(path)
                        if isinstance(rec, Session) and rec.native_identity == (host_id, harness, native_id):
                            return rec
        return None

    def read(self) -> Snapshot:
        self._check_active()
        try:
            files = {path.relative_to(self.root).as_posix(): path.read_bytes() for path in self._files()}
            return YamlStore.decode_snapshot(files)
        except (OSError, ValueError, yaml.YAMLError) as error:
            raise StorageError(f"cannot read catalog: {error}") from error

    def save(self, record: Record, *, expected: Snapshot) -> Snapshot:
        self._check_active()
        current = self.read()
        if current.revisions != expected.revisions:
            raise RevisionConflict("catalog changed since the captured snapshot")
        updated_catalog = current.catalog.replacing(record)
        key = record_key(record)
        if current.catalog.get(key) == record:
            return current
        envelope = {"version": 1, "revision": str(uuid4()), "kind": key.kind, "record": record_to_data(record)}
        contents = yaml.safe_dump(envelope, sort_keys=False, allow_unicode=True).encode("utf-8")
        path = _path(self.root, key)
        legacy_path = _path(self.root, RecordKey("session", record.id, record.host_id)) if isinstance(record, Session) else None
        temporary = None
        replaced = False
        try:
            # Check every internal directory before creating any missing descendants.
            if key.kind == "session" and key.stream_id is not None:
                _directory(path.parent.parent.parent, create=True)
                _directory(path.parent.parent, create=True)
            elif key.kind == "session":
                _directory(path.parent.parent, create=True)
            _directory(path.parent, create=True)
            staging = self.root / ".catalog-staging"
            _directory(staging, create=True)
            # A killed process may leave an orphan here. Staging is outside the
            # record namespace, so it cannot hide or corrupt a logical record.
            with tempfile.NamedTemporaryFile(dir=staging, prefix="record-", delete=False) as handle:
                temporary = Path(handle.name)
                handle.write(contents)
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, path)
            replaced = True
            if legacy_path is not None and legacy_path != path and legacy_path.exists():
                legacy_path.unlink(missing_ok=True)
            _sync_directory(path.parent)
            written = path.read_bytes()
            decoded = YamlStore.decode_record(path.relative_to(self.root).as_posix(), written)
            if decoded != record:
                raise StorageError("post-replace record mismatch")
            return Snapshot(
                catalog=updated_catalog,
                revisions={**current.revisions, key: hashlib.sha256(written).hexdigest()},
            )
        except (OSError, StorageError) as error:
            raise StorageError(f"cannot save catalog record: {error}", uncertain=replaced) from error
        finally:
            if temporary is not None and temporary.exists():
                temporary.unlink()
