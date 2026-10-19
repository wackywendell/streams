"""Session directory selection and document locations behind the application boundary.

Only explicit setup/enrollment changes selections. Reading a card never registers
checkouts, changes defaults, or substitutes another checkout for a missing one.
"""

from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass
import fcntl
import hashlib
import json
import os
from pathlib import Path

from .catalog.model import Session, Stream
from .catalog.sync.envelope import load_json
from .catalog_checkouts import CheckoutRegistry, _relative, inspect_directory
from .catalog_config import _atomic_json
from .errors import StreamError


@dataclass(frozen=True)
class DirectoryBinding:
    cwd: Path
    checkout_id: str | None = None
    relative_cwd: str | None = None
    repository_url: str | None = None


def _text(value: object) -> bool:
    return isinstance(value, str) and bool(value.strip()) and not any(ord(char) < 32 or ord(char) == 127 for char in value)


class DirectoryBindings:
    def __init__(self, state_dir: Path, checkouts: CheckoutRegistry):
        self.state_dir, self.checkouts = state_dir, checkouts

    def path(self, stream_id: str, session_id: str | None) -> Path:
        digest = hashlib.sha256(json.dumps((stream_id, session_id), ensure_ascii=True).encode()).hexdigest()
        return self.state_dir / "directories" / (digest + ".json")

    @staticmethod
    def checked(cwd: Path) -> Path:
        try:
            directory = Path(cwd).expanduser().resolve(strict=True)
            if not directory.is_dir():
                raise ValueError("not a directory")
            return directory
        except (OSError, RuntimeError, ValueError) as error:
            raise StreamError(f"cwd must be an existing directory: {cwd}: {error}; pass --cwd") from error

    def _selection(self, stream_id: str, session_id: str | None) -> dict | None:
        """Validate saved intent without requiring its old target to still exist."""
        path = self.path(stream_id, session_id)
        try:
            if path.is_symlink():
                raise ValueError("directory binding must not be a symlink")
            data = load_json(path.read_text())
        except FileNotFoundError:
            return None
        except (OSError, ValueError) as error:
            raise StreamError(f"could not read directory binding {path}: {error}") from error
        if not isinstance(data, dict) or data.get("stream_id") != stream_id or data.get("session_id") != session_id:
            raise StreamError(f"invalid directory binding identity: {path}")
        base = {"version", "stream_id", "session_id"}
        if type(data.get("version")) is not int:
            raise StreamError(f"invalid directory binding version: {path}")
        if data["version"] == 1 and set(data) == base | {"cwd"}:
            if not _text(data["cwd"]) or not Path(data["cwd"]).is_absolute():
                raise StreamError(f"invalid directory binding cwd: {path}")
            return data
        if data["version"] != 2 or set(data) != base | {"location"} or not isinstance(data["location"], dict):
            raise StreamError(f"invalid directory binding schema: {path}")
        location = data["location"]
        if location.get("kind") == "directory" and set(location) == {"kind", "cwd"}:
            if not _text(location["cwd"]) or not Path(location["cwd"]).is_absolute():
                raise StreamError(f"invalid directory binding cwd: {path}")
        elif (location.get("kind") != "checkout" or set(location) != {"kind", "id", "within", "repository_url"}
              or not _text(location["id"]) or not _text(location["within"])
              or location["repository_url"] is not None and not _text(location["repository_url"])):
            raise StreamError(f"invalid checkout directory binding: {path}")
        else:
            _relative(location["within"], allow_root=True)
        return data

    def read(self, stream_id: str, session_id: str | None) -> DirectoryBinding | None:
        data = self._selection(stream_id, session_id)
        if data is None:
            return None
        if data["version"] == 1:
            # Legacy absolute selections can use already registered roots, but
            # reading them never registers a root or changes repository bindings.
            directory = self.checked(Path(data["cwd"]))
            found = inspect_directory(directory)
            registered = self.checkouts.find(found[0]) if found else None
            return DirectoryBinding(directory, registered.id if registered else None,
                                    found[1] if registered else None)
        location = data["location"]
        if location["kind"] == "directory":
            return DirectoryBinding(self.checked(Path(location["cwd"])))
        return DirectoryBinding(self.checkouts.directory(location["id"], location["within"]),
                                location["id"], location["within"], location["repository_url"])

    @contextmanager
    def _lock(self, path: Path):
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        descriptor = os.open(path.with_suffix(".lock"), os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            yield
        finally:
            os.close(descriptor)

    def remember(self, stream_id: str, cwd: Path, *, session_id: str | None = None,
                 if_missing: bool = False, repository_url: str | None = None,
                 checkout_id: str | None = None) -> DirectoryBinding:
        if not _text(stream_id) or session_id is not None and not _text(session_id):
            raise StreamError("directory mapping requires nonempty stream/session IDs")
        directory = self.checked(cwd)
        path = self.path(stream_id, session_id)
        with self._lock(path):
            previous = self._selection(stream_id, session_id)
            if if_missing and previous is not None:
                return self.read(stream_id, session_id)
            found = inspect_directory(directory)
            if checkout_id is not None:
                selected = self.checkouts.get(checkout_id)
                if found is None or Path(selected.directory) != found[0]:
                    raise StreamError("selected checkout does not contain the requested cwd")
            elif found is not None:
                selected = self.checkouts.register(found[0], repository_url=repository_url)
            else:
                selected = None
            if selected is None:
                if repository_url is not None:
                    raise StreamError("repository binding requires a Git or jj checkout")
                location = {"kind": "directory", "cwd": str(directory)}
                binding = DirectoryBinding(directory)
            else:
                if repository_url is not None and repository_url not in selected.repository_urls:
                    raise StreamError("selected checkout lacks the explicit repository binding")
                location = {"kind": "checkout", "id": selected.id, "within": found[1], "repository_url": repository_url}
                binding = DirectoryBinding(directory, selected.id, found[1], repository_url)
            _atomic_json(path, {"version": 2, "stream_id": stream_id, "session_id": session_id, "location": location})
            return binding

    def document_context(self, stream: Stream, session: Session) -> dict:
        """Return current-file targets and per-reference errors, without native effects."""
        selected = None
        selection_error = None
        try:
            selected = self.read(stream.id, session.id)
            if selected is None:
                raise StreamError("no directory is selected for this Session; use streams session directory")
            if selected.checkout_id is None and inspect_directory(selected.cwd) is not None:
                raise StreamError("the Session checkout is not registered; use streams checkout register")
            primary = self.checkouts.get(selected.checkout_id) if selected.checkout_id else None
            if primary is not None and not primary.repository_urls:
                raise StreamError("the Session checkout has no repository binding; register its repository URL explicitly")
        except (StreamError, OSError, ValueError) as error:
            selection_error = str(error)
        repository_url = None
        if session.checkout is not None:
            repository = stream.repositories.get(session.checkout.repository)
            repository_url = repository.url if repository is not None else None
        if repository_url is None and selected is not None:
            repository_url = selected.repository_url
        kwargs = {"selected_checkout_id": selected.checkout_id if selected else None,
                  "selected_repository_url": repository_url}
        checkouts, locations = [], []
        resolved, errors = {}, {}
        for alias, repository in stream.repositories.items():
            try:
                if selection_error:
                    raise StreamError(selection_error)
                checkout = self.checkouts.resolve(repository.url, **kwargs)
                resolved[alias] = checkout
                checkouts.append({"repository": alias, "checkout_id": checkout.id, "root": checkout.directory})
            except (StreamError, OSError, ValueError) as error:
                errors[alias] = str(error)
        for document in stream.documents:
            checkout = resolved.get(document.repository)
            location = {"repository": document.repository, "path": document.path,
                        "checkout_id": checkout.id if checkout else None,
                        "root": checkout.directory if checkout else None, "file": None, "error": errors.get(document.repository)}
            if checkout is not None:
                try:
                    current, file = self.checkouts.document(stream.repositories[document.repository].url, document.path, **kwargs)
                    if current != checkout:
                        raise StreamError("checkout binding changed during document resolution; refresh to retry")
                    location["file"] = str(file)
                except (StreamError, OSError, ValueError) as error:
                    location["error"] = f"Document not present in this checkout: {document.path}. {error}"
            locations.append(location)
        return {"checkouts": checkouts, "document_locations": locations}
