"""Configuration and constants for scratch streams."""

from __future__ import annotations

import json
import os
from pathlib import Path
import tempfile

from .errors import StreamError

CONFIG_KEYS = ("notes.root", "obsidian.vault", "cmux.enabled")
ARCHIVE_DIRNAME = "archive"
VALID_STATUSES = frozenset({"active", "waiting", "blocked", "parked", "done"})
DEFAULT_STATUS = "active"
DEFAULT_STATE = "New stream ready for scoping."
VALID_VCS = frozenset({"jj", "git"})


def config_path() -> Path:
    return Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "streams/config.json"


def validate_value(key: str, value: object) -> str | bool:
    if key not in CONFIG_KEYS:
        raise StreamError(f"unknown streams config key: {key}")
    if key == "cmux.enabled":
        if not isinstance(value, bool):
            raise StreamError(f"{key} must be a boolean (true or false)")
    elif not isinstance(value, str) or not value.strip():
        raise StreamError(f"{key} must be a nonempty string")
    elif key == "notes.root":
        try:
            path = Path(value).expanduser()
        except RuntimeError as exc:
            raise StreamError(f"{key}: {exc}") from exc
        if not path.is_absolute():
            raise StreamError(f"{key} must be an absolute path (or start with ~/)")
        value = str(path)
    return value


def read_overrides() -> dict:
    path = config_path()
    try:
        data = json.loads(path.read_text())
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as exc:
        raise StreamError(f"could not read {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise StreamError(f"{path} must contain a JSON object")
    for section, values in data.items():
        if section not in {key.split('.')[0] for key in CONFIG_KEYS} or not isinstance(values, dict):
            raise StreamError(f"{path}: invalid config section {section!r}")
        for name, value in values.items():
            values[name] = validate_value(f"{section}.{name}", value)
    return data


def load_config() -> dict:
    config = {"notes": {"root": str(Path.home() / "worknotes")}, "obsidian": {"vault": "worknotes"}, "cmux": {"enabled": False}}
    for section, values in read_overrides().items():
        config[section].update(values)
    return config


def set_config(key: str, text: str) -> None:
    value = validate_value(key, {"true": True, "false": False}.get(text, text) if key == "cmux.enabled" else text)
    data = read_overrides()
    section, name = key.split(".")
    data.setdefault(section, {})[name] = value
    # Resolve Stow-managed symlinks so replacing the file preserves the link.
    path = config_path().resolve()
    temporary = None
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        fd, temporary = tempfile.mkstemp(prefix=".config-", suffix=".tmp", dir=path.parent)
        with os.fdopen(fd, "w") as handle:
            json.dump(data, handle, indent=2, sort_keys=True)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except OSError as exc:
        raise StreamError(f"could not write {path}: {exc}") from exc
    finally:
        if temporary is not None:
            Path(temporary).unlink(missing_ok=True)


def notes_root() -> Path:
    return Path(load_config()["notes"]["root"])


def scratch_root() -> Path:
    return notes_root() / "scratch"
