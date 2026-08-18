"""Local repository and working-copy location discovery."""

from __future__ import annotations

import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from .errors import StreamError

CommandRunner = Callable[[list[str], Path], str]


def format_local_path(path: Path, *, home: Path | None = None) -> str:
    """Render a local path with `~` when it is under the user's home."""
    path = path.expanduser()
    home = (home or Path.home()).expanduser()
    try:
        relative = path.relative_to(home)
    except ValueError:
        return str(path)
    return "~" if not relative.parts else f"~/{relative.as_posix()}"


@dataclass(frozen=True)
class StreamLocation:
    """The main checkout and working-copy roots for one stream."""

    vcs: str
    repo_root: Path
    working_root: Path

    @classmethod
    def from_strings(cls, *, vcs: str, repo: str, cwd: str) -> StreamLocation:
        return cls(
            vcs=vcs,
            repo_root=Path(repo).expanduser(),
            working_root=Path(cwd).expanduser(),
        )

    @property
    def is_dedicated(self) -> bool:
        return self.repo_root.resolve() != self.working_root.resolve()

    def matches_working_root(self, path: str | Path) -> bool:
        return self.working_root.resolve() == Path(path).expanduser().resolve()


def discover_stream_location(
    cwd: Path,
    *,
    runner: CommandRunner | None = None,
) -> StreamLocation:
    """Discover main and current working-copy roots containing `cwd`."""
    runner = runner or _run_command
    cwd = cwd.expanduser().resolve()

    try:
        working_root = Path(
            runner(["jj", "workspace", "root", "--ignore-working-copy"], cwd).strip()
        ).resolve()
    except StreamError:
        working_root = None

    if working_root is not None:
        repo_output = runner(
            [
                "jj",
                "workspace",
                "list",
                "--ignore-working-copy",
                "-T",
                'if(name == "default", root.absolute() ++ "\\n")',
            ],
            cwd,
        ).strip()
        if repo_output and not repo_output.startswith("<Error:"):
            repo_root = Path(repo_output).resolve()
        else:
            repo_root = _legacy_jj_repo_root(working_root)
        return StreamLocation(
            vcs="jj",
            repo_root=repo_root,
            working_root=working_root,
        )

    try:
        working_root = Path(
            runner(
                ["git", "rev-parse", "--path-format=absolute", "--show-toplevel"],
                cwd,
            ).strip()
        )
        worktrees = runner(["git", "worktree", "list", "--porcelain"], cwd)
        first_line = next(
            (line for line in worktrees.splitlines() if line.startswith("worktree ")),
            None,
        )
        if first_line is None:
            raise StreamError("git worktree list did not report a main worktree")
        return StreamLocation(
            vcs="git",
            repo_root=Path(first_line.removeprefix("worktree ")).resolve(),
            working_root=working_root.resolve(),
        )
    except StreamError as exc:
        raise StreamError(f"{cwd} is not inside a jj workspace or Git worktree") from exc


def _legacy_jj_repo_root(working_root: Path) -> Path:
    """Recover the main root for workspaces created before jj recorded roots."""
    repo_entry = working_root / ".jj" / "repo"
    if repo_entry.is_dir():
        return working_root
    try:
        shared_store = (repo_entry.parent / repo_entry.read_text().strip()).resolve()
    except OSError as exc:
        raise StreamError("jj default workspace root is unavailable") from exc
    if shared_store.name != "repo" or shared_store.parent.name != ".jj":
        raise StreamError("jj workspace repo pointer has an unexpected shape")
    return shared_store.parent.parent


def _run_command(args: list[str], cwd: Path) -> str:
    try:
        return subprocess.run(
            args,
            cwd=cwd,
            text=True,
            capture_output=True,
            check=True,
        ).stdout
    except (FileNotFoundError, subprocess.CalledProcessError) as exc:
        raise StreamError(f"{' '.join(args)} failed") from exc
