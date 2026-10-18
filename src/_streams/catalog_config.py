"""Strict catalog settings and explicit, host-bound backend initialization."""

from __future__ import annotations

from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import tempfile
from uuid import UUID, uuid4

from .catalog.storage import StorageError
from .catalog.sync import GitCatalog, LocalCatalog
from .catalog.sync.envelope import load_json
from .catalog.sync.local import State
from .catalog.sync.policy import Policy
from .catalog.yaml_store import YamlStore
from .errors import StreamError
from .frontend import FrontendMode

CONFIG_KEYS = ("catalog.root", "frontend.mode", "sync.stream_seconds", "sync.session_seconds")


def config_path() -> Path:
    return Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "streams/config.json"


def _value(key: str, value: object) -> str | int | float:
    if key not in CONFIG_KEYS:
        raise StreamError(f"unknown streams config key: {key}")
    if key.startswith("sync."):
        try:
            Policy(**{key.split(".")[1]: value})
        except (ValueError, OverflowError) as error:
            raise StreamError(f"{key} must be a finite nonnegative number") from error
        return value
    if not isinstance(value, str) or not value.strip() or "\x00" in value:
        raise StreamError(f"{key} must be a nonempty string without NUL characters")
    if key == "frontend.mode":
        if value not in {mode.value for mode in FrontendMode}:
            raise StreamError("frontend.mode must be one of auto, cmux, herdr, none")
        return value
    try:
        path = Path(value).expanduser()
    except (RuntimeError, ValueError) as error:
        raise StreamError(f"invalid catalog.root: {error}") from error
    if not path.is_absolute():
        raise StreamError("catalog.root must be an absolute path (or start with ~/)")
    return str(path)


def read_overrides() -> dict:
    path = config_path()
    try:
        data = load_json(path.read_text())
    except FileNotFoundError:
        if path.is_symlink():
            raise StreamError(f"broken config symlink: {path}")
        return {}
    except (OSError, ValueError) as error:
        raise StreamError(f"could not read {path}: {error}") from error
    if not isinstance(data, dict):
        raise StreamError(f"{path} must contain a JSON object")
    for section, values in data.items():
        if section not in {"catalog", "frontend", "sync"} or not isinstance(values, dict):
            raise StreamError(f"{path}: invalid config section {section!r}")
        for name, value in values.items():
            values[name] = _value(f"{section}.{name}", value)
    return data


def load_config() -> dict:
    data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    policy = Policy()
    result = {"catalog": {"root": str(data_home / "streams/catalog")}, "frontend": {"mode": "auto"},
              "sync": {"stream_seconds": policy.stream_seconds, "session_seconds": policy.session_seconds}}
    for section, values in read_overrides().items():
        result[section].update(values)
    # Defaults cross the same validation boundary as explicit settings.
    result["catalog"]["root"] = _value("catalog.root", result["catalog"]["root"])
    return result


@contextmanager
def _config_lock():
    directory = config_path().parent
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(directory / ".catalog-config.lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        os.close(descriptor)


def _atomic_json(path: Path, data: dict) -> None:
    temporary = None
    try:
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        descriptor, temporary = tempfile.mkstemp(prefix=".catalog-", suffix=".tmp", dir=path.parent)
        with os.fdopen(descriptor, "w") as handle:
            json.dump(data, handle, indent=2, sort_keys=True, allow_nan=False)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
        descriptor = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    finally:
        if temporary is not None:
            Path(temporary).unlink(missing_ok=True)


def set_config(key: str, text: str) -> None:
    value = text
    if key in CONFIG_KEYS and key.startswith("sync."):
        try:
            value = load_json(text)
        except ValueError as error:
            raise StreamError(f"{key} must be a finite nonnegative number") from error
    value = _value(key, value)
    try:
        with _config_lock():
            data = read_overrides()
            section, name = key.split(".")
            data.setdefault(section, {})[name] = value
            # Resolve the destination, not the link, for Stow-managed settings.
            _atomic_json(config_path().resolve(), data)
    except (OSError, RuntimeError) as error:
        raise StreamError(f"could not write {config_path()}: {error}") from error


def get_frontend_mode() -> FrontendMode:
    return FrontendMode(load_config()["frontend"]["mode"])


def catalog_root() -> Path:
    return Path(load_config()["catalog"]["root"])


def _uuid(value: object) -> str:
    if not isinstance(value, str):
        raise ValueError("host identity must be a UUID")
    if str(UUID(value)) != value:
        raise ValueError("host identity must be a canonical UUID")
    return value


def _host(*, create: bool) -> str:
    path = config_path().parent / "host.json"
    if path.is_symlink():
        raise StreamError(f"symlinked host identity: {path}; repair host state before running streams")
    try:
        data = load_json(path.read_text())
    except FileNotFoundError:
        if not create:
            raise StreamError(f"host identity is missing at {path}; run streams init for a new catalog; restore host.json for an existing catalog")
        identity = str(uuid4())
        _atomic_json(path, {"version": 1, "id": identity})
        return identity
    except (OSError, ValueError) as error:
        raise StreamError(f"invalid host identity at {path}: {error}; restore host.json, do not regenerate it") from error
    try:
        if not isinstance(data, dict) or set(data) != {"version", "id"} or type(data["version"]) is not int or data["version"] != 1:
            raise ValueError("unsupported host identity state")
        return _uuid(data["id"])
    except ValueError as error:
        raise StreamError(f"invalid host identity at {path}: {error}; restore host.json, do not regenerate it") from error


def host_id(*, create: bool = False) -> str:
    if not create:
        return _host(create=False)
    try:
        with _config_lock():
            # Never repair an existing catalog by issuing it a new producer ID.
            marker = catalog_root() / ".catalog-sync/cli.json"
            return _host(create=not (marker.exists() or marker.is_symlink()))
    except OSError as error:
        raise StreamError(f"could not create host identity: {error}") from error


def runtime_state_dir() -> Path:
    digest = hashlib.sha256(str(catalog_root().resolve()).encode()).hexdigest()
    state_home = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state")
    return state_home / "streams/catalogs" / digest


def _check_root(root: Path) -> None:
    if root.is_symlink() or (root.exists() and not root.is_dir()):
        raise StreamError(f"catalog root must be a real directory: {root}")
    for name in (".catalog-sync", ".catalog.lock", ".catalog-staging"):
        if (root / name).is_symlink():
            raise StreamError(f"symlinked catalog machinery: {root / name}")
    state = root / ".catalog-sync"
    if state.exists() and not state.is_dir():
        raise StreamError(f"invalid catalog state directory: {state}")


def _marker(root: Path) -> dict | None:
    _check_root(root)
    path = root / ".catalog-sync/cli.json"
    if not path.exists() and not path.is_symlink():
        return None
    data = State(root).read("cli.json")
    if (not isinstance(data, dict) or set(data) != {"version", "strategy", "host"}
            or data.get("strategy") not in ("none", "git")):
        raise StreamError("invalid catalog initialization marker; inspect .catalog-sync/cli.json")
    try:
        _uuid(data["host"])
    except ValueError as error:
        raise StreamError(f"invalid catalog initialization host: {error}") from error
    return data


def _check_host(marker: dict, identity: str) -> None:
    if marker["host"] != identity:
        raise StreamError("catalog belongs to a different host; restore the matching private host identity")


def _empty_root(root: Path) -> None:
    if any(path.name not in {".catalog.lock", ".catalog-sync"} for path in root.iterdir()):
        raise StreamError("initialization requires a new empty catalog directory")
    state = root / ".catalog-sync"
    if state.exists() and any(state.iterdir()):
        raise StreamError("initialization requires empty catalog bookkeeping; inspect existing state")


def open_catalog() -> LocalCatalog | GitCatalog:
    settings = load_config()
    root = Path(settings["catalog"]["root"])
    try:
        marker = _marker(root)
        if marker is None:
            raise StreamError("catalog is not initialized; run streams init")
        identity = host_id()
        _check_host(marker, identity)
        if marker["strategy"] == "none":
            backend = LocalCatalog(YamlStore(root), host_id=identity)
            backend.snapshot()
            return backend
        backend = GitCatalog(root, host_id=identity, policy=Policy(**settings["sync"]))
        backend.snapshot()
        return backend
    except (OSError, StorageError, ValueError) as error:
        raise StreamError(f"cannot open catalog: {error}; inspect catalog state or run streams init for a new catalog") from error


def initialize(strategy: str, *, remote: str | None = None, branch: str = "main") -> LocalCatalog | GitCatalog:
    if strategy not in ("none", "git"):
        raise StreamError("sync strategy must be none or git")
    if strategy == "none" and remote is not None:
        raise StreamError("--remote requires --sync git")
    if strategy == "git" and (not isinstance(remote, str) or not remote.strip() or "\x00" in remote):
        raise StreamError("Git initialization requires --remote")
    if not isinstance(branch, str) or not branch.strip() or "\x00" in branch:
        raise StreamError("branch must be a nonempty string without NUL characters")
    settings = load_config()
    root = Path(settings["catalog"]["root"])
    try:
        with _config_lock():
            marker = _marker(root)
            identity = _host(create=marker is None)
            if marker is not None:
                _check_host(marker, identity)
                if marker["strategy"] != strategy:
                    raise StreamError("catalog sync strategy does not match; existing catalogs cannot switch strategy")
            if strategy == "git":
                if marker is None:
                    with YamlStore(root).transaction():
                        _empty_root(root)
                backend = GitCatalog.initialize(root, remote=remote, host_id=identity, branch=branch,
                                                policy=Policy(**settings["sync"]))
                backend.snapshot()
                if marker is None:
                    with backend.store.transaction():
                        State(root).write("cli.json", {"strategy": strategy, "host": identity})
                return backend
            backend = LocalCatalog(YamlStore(root), host_id=identity)
            with backend.store.transaction() as transaction:
                # Read again while holding the same lock as catalog writers.
                marker = _marker(root)
                if marker is None:
                    _empty_root(root)
                else:
                    _check_host(marker, identity)
                    if marker["strategy"] != strategy:
                        raise StreamError("catalog sync strategy does not match")
                transaction.read()
                if marker is None:
                    State(root).write("cli.json", {"strategy": strategy, "host": identity})
            backend.snapshot()
            return backend
    except (OSError, StorageError, ValueError) as error:
        raise StreamError(f"cannot initialize catalog: {error}") from error
