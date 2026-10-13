"""Configuration and constants for scratch streams."""

from __future__ import annotations

import json
import os
from pathlib import Path
import tempfile

from .errors import StreamError
from .frontend import FrontendMode

CONFIG_KEYS = ("notes.root", "obsidian.vault", "frontend.mode")
LEGACY_CONFIG_KEYS = ("cmux.enabled",)
ARCHIVE_DIRNAME = "archive"
VALID_STATUSES = frozenset({"active", "waiting", "blocked", "parked", "done"})
DEFAULT_STATUS = "active"
DEFAULT_STATE = "New stream ready for scoping."
VALID_VCS = frozenset({"jj", "git"})


def config_path() -> Path:
    return Path(os.environ.get("XDG_CONFIG_HOME") or Path.home() / ".config") / "streams/config.json"


def validate_value(key: str, value: object) -> str | bool:
    if key not in (*CONFIG_KEYS, *LEGACY_CONFIG_KEYS):
        raise StreamError(f"unknown streams config key: {key}")
    if key == "cmux.enabled":
        if not isinstance(value, bool):
            raise StreamError(f"{key} must be a boolean (true or false)")
    elif key == "frontend.mode":
        if not isinstance(value, str) or value not in {mode.value for mode in FrontendMode}:
            raise StreamError(f"{key} must be one of auto, cmux, herdr, none")
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


def _read_overrides() -> dict:
    """Decode the document; consumers validate the settings they depend on."""
    path = config_path()
    try:
        data = json.loads(path.read_text())
    except FileNotFoundError:
        return {}
    except (OSError, ValueError) as exc:
        raise StreamError(f"could not read {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise StreamError(f"{path} must contain a JSON object")
    return data


def read_overrides() -> dict:
    data = _read_overrides()
    path = config_path()
    for section, values in data.items():
        if section not in {key.split('.')[0] for key in (*CONFIG_KEYS, *LEGACY_CONFIG_KEYS)} or not isinstance(values, dict):
            raise StreamError(f"{path}: invalid config section {section!r}")
        for name, value in values.items():
            values[name] = validate_value(f"{section}.{name}", value)
    _frontend_mode(data)  # Reject conflicting selectors at the configuration boundary.
    return data


def _frontend_mode(overrides: dict) -> FrontendMode:
    configured = overrides.get("frontend", {}).get("mode")
    enabled = overrides.get("cmux", {}).get("enabled")
    legacy = FrontendMode.CMUX if enabled else FrontendMode.NONE
    if configured is not None:
        mode = FrontendMode(configured)
        if enabled is not None and mode != legacy:
            raise StreamError("conflicting frontend.mode and legacy cmux.enabled selectors")
        return mode
    return legacy if enabled is not None else FrontendMode.AUTO


def get_frontend_mode() -> FrontendMode:
    return _frontend_mode(read_overrides())


def load_config() -> dict:
    overrides = read_overrides()
    config = {"notes": {"root": str(Path.home() / "streams")}, "obsidian": {"vault": "streams"},
              "frontend": {"mode": _frontend_mode(overrides).value}}
    for section in ("notes", "obsidian"):
        config[section].update(overrides.get(section, {}))
    return config


def set_config(key: str, text: str) -> None:
    if key not in CONFIG_KEYS:
        raise StreamError(f"unknown streams config key: {key}; use frontend.mode for selection")
    value = validate_value(key, text)
    data = read_overrides()
    section, name = key.split(".")
    data.setdefault(section, {})[name] = value
    if key == "frontend.mode":
        data.pop("cmux", None)
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
    # Note operations must not depend on optional frontend or display policy.
    notes = _read_overrides().get("notes", {})
    if not isinstance(notes, dict):
        raise StreamError(f"{config_path()}: invalid config section 'notes'")
    values = {name: validate_value(f"notes.{name}", value) for name, value in notes.items()}
    return Path(values.get("root", str(Path.home() / "streams")))


def scratch_root() -> Path:
    return notes_root() / "scratch"
