"""Explicit Herdr workspace-to-stream associations, outside scratch notes."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from dataclasses import dataclass
from pathlib import Path

from .errors import StreamError
from .slug import validate_slug


@dataclass(frozen=True)
class HerdrEndpoint:
    socket: str
    session: str | None


def binding_path(endpoint: HerdrEndpoint, workspace_id: str) -> Path:
    namespace = json.dumps({"socket": endpoint.socket, "session": endpoint.session}, sort_keys=True)
    server_key = hashlib.sha256(namespace.encode()).hexdigest()
    workspace_key = hashlib.sha256(workspace_id.encode()).hexdigest()
    root = Path(os.environ.get("XDG_STATE_HOME") or Path.home() / ".local/state")
    return root / "streams" / "herdr" / server_key / f"{workspace_key}.json"


def read_binding(path: Path) -> str:
    try:
        data = json.loads(path.read_text())
    except (OSError, ValueError) as exc:
        raise StreamError(f"could not read Herdr association {path}: {exc}") from exc
    ref = data.get("stream") if isinstance(data, dict) else None
    if not isinstance(ref, str) or not ref.strip():
        raise StreamError(f"Herdr association {path} must contain a nonempty stream slug")
    validate_slug(ref)
    return ref


def read_optional_binding(path: Path) -> str | None:
    """Only an absent association permits workspace-hint inference."""
    try:
        path.lstat()  # A dangling symlink is a broken association, not absence.
    except FileNotFoundError:
        return None
    except OSError as exc:
        raise StreamError(f"could not inspect Herdr association {path}: {exc}") from exc
    return read_binding(path)


def write_binding(path: Path, stream_ref: str) -> None:
    validate_slug(stream_ref)
    temporary = None
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        fd, temporary = tempfile.mkstemp(prefix=".binding-", suffix=".tmp", dir=path.parent)
        with os.fdopen(fd, "w") as handle:
            json.dump({"stream": stream_ref}, handle)
            handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    except OSError as exc:
        raise StreamError(f"could not write Herdr association {path}: {exc}") from exc
    finally:
        if temporary is not None:
            Path(temporary).unlink(missing_ok=True)
