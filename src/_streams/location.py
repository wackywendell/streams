"""Local repository and working-copy location discovery."""

from __future__ import annotations

import subprocess
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from .errors import StreamError

CommandRunner = Callable[[list[str], Path], str]


@dataclass(frozen=True)
class PathAlias:
    """A human-facing path root and its canonical filesystem target."""

    display_root: str
    canonical_root: Path


def discover_home_path_aliases(home: Path | None = None) -> tuple[PathAlias, ...]:
    """Discover direct home-directory symlinks to directories."""
    home = (home or Path.home()).expanduser()
    aliases: list[PathAlias] = []
    for entry in home.iterdir():
        if not entry.is_symlink():
            continue
        try:
            target = entry.resolve(strict=True)
        except (OSError, RuntimeError):
            continue
        if target.is_dir():
            aliases.append(PathAlias(display_root=f"~/{entry.name}", canonical_root=target))
    return tuple(sorted(aliases, key=lambda alias: alias.display_root))


def format_local_path(
    path: Path,
    *,
    home: Path | None = None,
    aliases: tuple[PathAlias, ...] = (),
) -> str:
    """Render a canonical path using a preferred home symlink or `~`."""
    path = path.expanduser().resolve()
    home = (home or Path.home()).expanduser().resolve()
    if path == home:
        return "~"

    matches: list[tuple[int, str]] = []
    for alias in aliases:
        canonical_root = alias.canonical_root.expanduser().resolve()
        try:
            relative = path.relative_to(canonical_root)
        except ValueError:
            continue
        rendered = alias.display_root
        if relative.parts:
            rendered = f"{rendered}/{relative.as_posix()}"
        matches.append((len(canonical_root.parts), rendered))
    if matches:
        matches.sort(key=lambda match: (-match[0], len(match[1]), match[1]))
        return matches[0][1]

    try:
        relative = path.relative_to(home)
    except ValueError:
        return str(path)
    return f"~/{relative.as_posix()}"


@dataclass(frozen=True)
class StreamLocation:
    """A stream's working directory and optional repository roots."""

    vcs: str | None
    repo_root: Path | None
    working_root: Path

    @classmethod
    def from_strings(cls, *, vcs: str | None, repo: str | None, cwd: str) -> StreamLocation:
        return cls(
            vcs=vcs,
            repo_root=Path(repo).expanduser() if repo is not None else None,
            working_root=Path(cwd).expanduser(),
        )

    @property
    def is_dedicated(self) -> bool:
        return self.repo_root is not None and self.repo_root.resolve() != self.working_root.resolve()

    def matches_working_root(self, path: str | Path) -> bool:
        return self.working_root.resolve() == Path(path).expanduser().resolve()


def discover_stream_location(
    cwd: Path,
    *,
    runner: CommandRunner | None = None,
) -> StreamLocation:
    """Discover repository roots containing `cwd`, or keep it as a repo-less directory."""
    runner = runner or _run_command
    cwd = cwd.expanduser().resolve()

    try:
        working_root = Path(
            runner(["jj", "workspace", "root", "--ignore-working-copy"], cwd).strip()
        ).resolve()
    except StreamError:
        if _has_vcs_metadata(cwd, ".jj"):
            raise
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
    except StreamError:
        if _has_vcs_metadata(cwd, ".git"):
            raise
        return StreamLocation(vcs=None, repo_root=None, working_root=cwd)

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


def _has_vcs_metadata(cwd: Path, marker: str) -> bool:
    """Do not mistake broken or inaccessible repository metadata for absence."""
    return any((root / marker).exists() or (root / marker).is_symlink() for root in (cwd, *cwd.parents))


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
