"""Host-local checkout identities and explicit repository-locator bindings."""

from __future__ import annotations

from contextlib import contextmanager
from dataclasses import dataclass, replace
import fcntl
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import tempfile
from uuid import uuid4

from .errors import StreamError


def _text(value: object, name: str) -> str:
    if not isinstance(value, str) or not value.strip() or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise StreamError(f"{name} must be nonempty text without control characters")
    return value


def _directory(path: Path) -> Path:
    try:
        root = path.expanduser().resolve(strict=True)
        if not root.is_dir():
            raise StreamError(f"not a directory: {path}")
        # resolve() follows symlinks but preserves case aliases on macOS. Use
        # directory-entry spellings so every registration stores one exact path.
        canonical = Path(root.anchor)
        for part in root.parts[1:]:
            entries = tuple(canonical.iterdir())
            exact = next((entry for entry in entries if entry.name == part), None)
            if exact is not None:
                canonical = exact
                continue
            target = (canonical / part).stat()
            for entry in entries:
                try:
                    metadata = entry.stat(follow_symlinks=False)
                except OSError:
                    continue
                if (metadata.st_dev, metadata.st_ino) == (target.st_dev, target.st_ino):
                    canonical = entry
                    break
            else:
                raise FileNotFoundError(f"canonical directory entry not found: {canonical / part}")
        _text(str(canonical), "checkout directory")
        return canonical
    except (OSError, RuntimeError, ValueError, StopIteration) as error:
        raise StreamError(f"directory is unavailable: {path}: {error}") from error


def _run(args: list[str], cwd: Path) -> str:
    # Ignore caller Git routing variables: inspection must describe this directory.
    environment = {key: value for key, value in os.environ.items()
                   if not key.startswith("GIT_")}
    environment["GIT_OPTIONAL_LOCKS"] = "0"
    try:
        result = subprocess.run(args, cwd=cwd, env=environment, capture_output=True,
                                text=True, check=True, timeout=15)
        return result.stdout.rstrip("\r\n")
    except (OSError, subprocess.SubprocessError, UnicodeError) as error:
        raise StreamError(f"checkout inspection failed in {cwd}: {' '.join(args)}") from error


def inspect_directory(cwd: Path) -> tuple[Path, str] | None:
    """Find the nearest marked worktree, without snapshotting a jj workspace."""
    current = _directory(cwd)
    for candidate in (current, *current.parents):
        jj = candidate / ".jj"
        git = candidate / ".git"
        try:
            has_jj = jj.exists() or jj.is_symlink()
            has_git = git.exists() or git.is_symlink()
            if not has_jj and not has_git:
                continue
            if has_jj:
                if jj.is_symlink() or not jj.is_dir():
                    raise StreamError(f"malformed jj marker: {jj}")
                output = _run(["jj", "--ignore-working-copy", "--at-operation=@", "workspace", "root"], candidate)
            else:
                if git.is_symlink() or not (git.is_dir() or git.is_file()):
                    raise StreamError(f"malformed Git marker: {git}")
                output = _run(["git", "rev-parse", "--path-format=absolute", "--show-toplevel"], candidate)
            root = _directory(Path(output))
            if not Path(output).is_absolute() or root != candidate:
                raise StreamError(f"VCS marker does not describe its exact checkout root: {candidate}")
            return root, current.relative_to(root).as_posix()
        except (OSError, ValueError, RuntimeError) as error:
            raise StreamError(f"cannot inspect checkout marker in {candidate}: {error}") from error
    return None


def _exact_root(directory: Path) -> Path:
    canonical = _directory(directory)
    found = inspect_directory(canonical)
    if found is None or found[0] != canonical:
        raise StreamError(f"directory must be an exact Git or jj checkout root: {directory}")
    return canonical


@dataclass(frozen=True)
class CheckoutLocation:
    id: str
    directory: str
    repository_urls: tuple[str, ...]
    device: int
    inode: int


@dataclass
class _State:
    checkouts: list[CheckoutLocation]
    defaults: dict[str, str]


def _pairs(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise StreamError(f"duplicate registry JSON key: {key}")
        result[key] = value
    return result


def _relative(value: str, *, allow_root: bool) -> str:
    _text(value, "relative path")
    if value == "." and allow_root:
        return value
    if "\\" in value or any(part in ("", ".", "..") for part in value.split("/")) or Path(value).is_absolute():
        raise StreamError(f"expected a nonescaping portable relative path: {value!r}")
    if ":" in value.split("/")[0]:
        raise StreamError(f"expected a portable relative path: {value!r}")
    return value


class CheckoutRegistry:
    def __init__(self, state_dir: Path, host_id: str):
        self.host_id = _text(host_id, "host ID")
        digest = hashlib.sha256(host_id.encode()).hexdigest()
        self.path = state_dir / f"checkouts-{digest}.json"
        self._lock_path = state_dir / f"checkouts-{digest}.lock"

    def _read(self) -> _State:
        try:
            descriptor = os.open(self.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        except FileNotFoundError:
            return _State([], {})
        except OSError as error:
            raise StreamError(f"cannot read checkout registry {self.path}: {error}") from error
        try:
            with os.fdopen(descriptor) as handle:
                if not stat.S_ISREG(os.fstat(handle.fileno()).st_mode):
                    raise StreamError(f"checkout registry is not a regular file: {self.path}")
                data = json.load(handle, object_pairs_hook=_pairs,
                                 parse_constant=lambda value: self._invalid(f"invalid JSON constant {value}"))
            return self._decode(data)
        except (OSError, ValueError, UnicodeError) as error:
            raise StreamError(f"invalid checkout registry {self.path}: {error}") from error

    @staticmethod
    def _invalid(message: str):
        raise StreamError(f"invalid checkout registry: {message}")

    def _decode(self, data: object) -> _State:
        if (not isinstance(data, dict) or set(data) != {"version", "host_id", "checkouts", "defaults"}
                or type(data["version"]) is not int or data["version"] != 1
                or data["host_id"] != self.host_id
                or not isinstance(data["checkouts"], list) or not isinstance(data["defaults"], dict)):
            self._invalid("unsupported schema or host")
        checkouts = []
        ids, roots, identities = set(), set(), set()
        live_identities = set()
        for record in data["checkouts"]:
            if (not isinstance(record, dict)
                    or set(record) != {"id", "directory", "repository_urls", "device", "inode"}):
                self._invalid("invalid checkout fields")
            identity = _text(record["id"], "checkout ID")
            directory = _text(record["directory"], "checkout directory")
            path = Path(directory)
            if not path.is_absolute() or str(path) != directory or directory.startswith("//") or ".." in path.parts:
                self._invalid("checkout directory must be canonical and absolute")
            if (not isinstance(record["repository_urls"], list)
                    or any(type(record[key]) is not int or record[key] < 0 for key in ("device", "inode"))):
                self._invalid("invalid checkout bindings or filesystem identity")
            urls = tuple(_text(url, "repository URL") for url in record["repository_urls"])
            physical = (record["device"], record["inode"])
            if (identity in ids or directory in roots or physical in identities or len(set(urls)) != len(urls)):
                self._invalid("duplicate checkout ID, root, or binding")
            # A corrupt document may name two case/symlink aliases with different recorded identities.
            try:
                live = path.stat()
                live_identity = (live.st_dev, live.st_ino)
                if live_identity in live_identities:
                    self._invalid("duplicate physical checkout roots")
                live_identities.add(live_identity)
            except FileNotFoundError:
                pass
            ids.add(identity)
            roots.add(directory)
            identities.add(physical)
            checkouts.append(CheckoutLocation(identity, directory, urls, *physical))
        defaults = data["defaults"]
        by_id = {checkout.id: checkout for checkout in checkouts}
        for url, identity in defaults.items():
            _text(url, "repository URL")
            _text(identity, "default checkout ID")
            if identity not in by_id or url not in by_id[identity].repository_urls:
                self._invalid("default must name an explicitly bound checkout")
        return _State(checkouts, defaults)

    @contextmanager
    def _write_state(self):
        temporary = None
        descriptor = None
        try:
            self.path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            descriptor = os.open(self._lock_path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
            if not stat.S_ISREG(os.fstat(descriptor).st_mode):
                raise StreamError("checkout registry lock is not a regular file")
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            state = self._read()
            yield state
            data = {"version": 1, "host_id": self.host_id,
                    "checkouts": [{"id": item.id, "directory": item.directory,
                                   "repository_urls": list(item.repository_urls),
                                   "device": item.device, "inode": item.inode} for item in state.checkouts],
                    "defaults": state.defaults}
            # Never overwrite a state file redirected to another destination.
            if self.path.is_symlink():
                raise StreamError(f"symlinked checkout registry: {self.path}")
            fd, temporary = tempfile.mkstemp(prefix=".checkouts-", suffix=".tmp", dir=self.path.parent)
            with os.fdopen(fd, "w") as handle:
                json.dump(data, handle, indent=2, sort_keys=True, allow_nan=False)
                handle.write("\n")
                handle.flush()
                os.fsync(handle.fileno())
            os.replace(temporary, self.path)
            fd = os.open(self.path.parent, os.O_RDONLY)
            try:
                os.fsync(fd)
            finally:
                os.close(fd)
        except (OSError, ValueError, RuntimeError) as error:
            raise StreamError(f"cannot update checkout registry {self.path}: {error}") from error
        finally:
            if temporary is not None:
                Path(temporary).unlink(missing_ok=True)
            if descriptor is not None:
                os.close(descriptor)

    @staticmethod
    def _lookup(state: _State, checkout_id: str) -> CheckoutLocation:
        _text(checkout_id, "checkout ID")
        for checkout in state.checkouts:
            if checkout.id == checkout_id:
                return checkout
        raise StreamError(f"checkout is not registered: {checkout_id}", kind="not-found")

    @staticmethod
    def _validate(checkout: CheckoutLocation) -> CheckoutLocation:
        try:
            path = Path(checkout.directory)
            root = _directory(path)
            metadata = root.stat()
            if root != path or (metadata.st_dev, metadata.st_ino) != (checkout.device, checkout.inode):
                raise StreamError(f"checkout root was replaced: {checkout.directory}; explicitly move the registration")
            _exact_root(root)
            return checkout
        except OSError as error:
            raise StreamError(f"checkout root is unavailable: {checkout.directory}: {error}") from error

    def list(self) -> tuple[CheckoutLocation, ...]:
        return tuple(self._read().checkouts)

    def get(self, checkout_id: str) -> CheckoutLocation:
        return self._validate(self._lookup(self._read(), checkout_id))

    def _find(self, state: _State, root: Path) -> CheckoutLocation | None:
        metadata = root.stat()
        for checkout in state.checkouts:
            if (Path(checkout.directory) == root
                    or (checkout.device, checkout.inode) == (metadata.st_dev, metadata.st_ino)):
                return self._validate(checkout)
            try:
                if root.samefile(checkout.directory):
                    return self._validate(checkout)
            except FileNotFoundError:
                pass
        return None

    def find(self, directory: Path) -> CheckoutLocation | None:
        state = self._read()
        try:
            return self._find(state, _exact_root(directory))
        except OSError as error:
            raise StreamError(f"cannot find checkout: {error}") from error

    def register(self, directory: Path, *, repository_url: str | None = None) -> CheckoutLocation:
        if repository_url is not None:
            _text(repository_url, "repository URL")
        with self._write_state() as state:
            root = _exact_root(directory)
            checkout = self._find(state, root)
            if checkout is None:
                metadata = root.stat()
                checkout = CheckoutLocation(str(uuid4()), str(root), (), metadata.st_dev, metadata.st_ino)
                state.checkouts.append(checkout)
            if repository_url is not None and repository_url not in checkout.repository_urls:
                updated = replace(checkout, repository_urls=(*checkout.repository_urls, repository_url))
                state.checkouts[state.checkouts.index(checkout)] = updated
                checkout = updated
        return checkout

    def move(self, checkout_id: str, directory: Path) -> CheckoutLocation:
        with self._write_state() as state:
            checkout = self._lookup(state, checkout_id)
            root = _exact_root(directory)
            metadata = root.stat()
            for other in state.checkouts:
                if other.id == checkout.id:
                    continue
                if (other.directory == str(root)
                        or (other.device, other.inode) == (metadata.st_dev, metadata.st_ino)):
                    raise StreamError(f"destination is already registered: {root}")
                try:
                    if root.samefile(other.directory):
                        raise StreamError(f"destination is already registered: {root}")
                except FileNotFoundError:
                    pass
            updated = replace(checkout, directory=str(root), device=metadata.st_dev, inode=metadata.st_ino)
            state.checkouts[state.checkouts.index(checkout)] = updated
        return updated

    def set_default(self, repository_url: str, checkout_id: str) -> None:
        _text(repository_url, "repository URL")
        with self._write_state() as state:
            checkout = self._validate(self._lookup(state, checkout_id))
            if repository_url not in checkout.repository_urls:
                raise StreamError("default requires an explicit repository URL binding")
            state.defaults[repository_url] = checkout.id

    def resolve(self, repository_url: str, *, selected_checkout_id: str | None = None,
                selected_repository_url: str | None = None) -> CheckoutLocation:
        _text(repository_url, "repository URL")
        if selected_repository_url is not None:
            _text(selected_repository_url, "selected repository URL")
        state = self._read()
        if selected_checkout_id is not None:
            selected = self._lookup(state, selected_checkout_id)
            if selected_repository_url is None and not selected.repository_urls:
                raise StreamError("selected checkout lacks an explicit repository URL binding")
            if selected_repository_url == repository_url or repository_url in selected.repository_urls:
                if repository_url not in selected.repository_urls:
                    raise StreamError("selected checkout lacks an explicit repository URL binding")
                return self._validate(selected)
        elif selected_repository_url == repository_url:
            raise StreamError("selected repository requires a selected checkout ID")
        identity = state.defaults.get(repository_url)
        if identity is None:
            candidates = [item.id for item in state.checkouts if repository_url in item.repository_urls]
            raise StreamError(f"no explicit checkout default for repository: {repository_url}",
                              kind="ambiguous" if len(candidates) > 1 else "not-found", candidates=candidates)
        return self._validate(self._lookup(state, identity))

    @staticmethod
    def _contained(checkout: CheckoutLocation, relative: str, *, directory: bool) -> Path:
        _relative(relative, allow_root=directory)
        root = Path(checkout.directory)
        try:
            target = (root / relative).resolve(strict=True)
            if not target.is_relative_to(root):
                raise StreamError(f"path escapes checkout root: {relative}")
            if not (target.is_dir() if directory else target.is_file()):
                raise StreamError(f"not an existing {'directory' if directory else 'regular file'}: {relative}")
            return target
        except (OSError, RuntimeError, ValueError) as error:
            raise StreamError(f"checkout path is unavailable: {relative}: {error}") from error

    def directory(self, checkout_id: str, relative_cwd: str = ".") -> Path:
        return self._contained(self.get(checkout_id), relative_cwd, directory=True)

    def document(self, repository_url: str, path: str, *, selected_checkout_id: str | None = None,
                 selected_repository_url: str | None = None) -> tuple[CheckoutLocation, Path]:
        checkout = self.resolve(repository_url, selected_checkout_id=selected_checkout_id,
                                selected_repository_url=selected_repository_url)
        return checkout, self._contained(checkout, path, directory=False)
