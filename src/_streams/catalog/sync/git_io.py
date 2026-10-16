"""Bounded Git subprocesses and object/index operations for a managed repository."""

import os
from pathlib import Path
import subprocess

from ..storage import StorageError


class GitError(StorageError):
    def __init__(self, message: str, *, code: int | None = None):
        super().__init__(message, uncertain=code is None)
        self.code = code


class Git:
    def __init__(self, root: Path, timeout: float):
        self.root, self.timeout = root, timeout
        self.histories = {}
        self._blobs = {}

    def run(self, *args: str, input: bytes | None = None, index: Path | None = None,
            allowed: tuple[int, ...] = (0,)) -> bytes:
        # Inherited index/worktree/object overrides can otherwise redirect a save
        # into the caller's source repository. Keep explicit test/global config.
        env = {key: value for key, value in os.environ.items()
               if not key.startswith("GIT_") or key in ("GIT_CONFIG_GLOBAL", "GIT_CONFIG_SYSTEM", "GIT_CONFIG_NOSYSTEM")}
        env.update(GIT_TERMINAL_PROMPT="0", GIT_LITERAL_PATHSPECS="1")
        if index is not None:
            env["GIT_INDEX_FILE"] = str(index)
        try:
            result = subprocess.run(["git", "-C", str(self.root), *args], input=input,
                                    capture_output=True, timeout=self.timeout, env=env)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise GitError(f"Git {args[0]} could not complete: {error}") from error
        if result.returncode not in allowed:
            raise GitError(f"Git {args[0]} failed: {result.stderr.decode(errors='replace').strip()}", code=result.returncode)
        return result.stdout

    def text(self, *args: str, **kwargs) -> str:
        return self.run(*args, **kwargs).decode().strip()

    def head(self) -> str:
        return self.text("rev-parse", "--verify", "HEAD")

    def ancestor(self, before: str, after: str) -> bool:
        # Histories are linear; no merge base is a normal false result, while
        # missing objects and repository errors must remain visible.
        return self.text("merge-base", before, after, allowed=(0, 1)) == before

    def files(self, revision: str) -> dict[str, bytes]:
        paths = {}
        for entry in self.run("ls-tree", "-rz", "--full-tree", revision).split(b"\0"):
            if not entry:
                continue
            description, path = entry.split(b"\t", 1)
            mode, kind, oid = description.decode().split()
            if mode != "100644" or kind != "blob":
                raise StorageError("catalog history contains a non-regular record")
            paths[path.decode()] = oid
        missing = sorted(set(paths.values()) - self._blobs.keys())
        if missing:
            output = self.run("cat-file", "--batch", input=("\n".join(missing) + "\n").encode())
            offset = 0
            for expected in missing:
                end = output.index(b"\n", offset)
                oid, kind, size = output[offset:end].decode().split()
                size = int(size)
                if oid != expected or kind != "blob":
                    raise StorageError("invalid Git blob batch response")
                offset = end + 1
                self._blobs[oid] = output[offset:offset + size]
                offset += size + 1
        return {path: self._blobs[oid] for path, oid in paths.items()}

    def commit(self, tree: str, message: str, parent: str | None = None) -> str:
        args = ["commit-tree", tree]
        if parent is not None:
            args.extend(("-p", parent))
        # Plumbing does not enable commit.gpgSign automatically. Honour the
        # configured policy rather than silently emitting unsigned catalog saves.
        signed = self.text("config", "--bool", "--get", "commit.gpgsign", allowed=(0, 1))
        if signed == "true":
            args.append("-S")
        return self.text(*args, input=message.encode())
