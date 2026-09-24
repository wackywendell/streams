"""Filesystem access for scratch streams."""

from __future__ import annotations

from datetime import date
import hashlib
import os
from pathlib import Path
import re
import tempfile

from .check import check_content
from .config import ARCHIVE_DIRNAME, DEFAULT_STATE, DEFAULT_STATUS, SCRATCH_ROOT
from .errors import StreamError
from .frontmatter import render_frontmatter, split_frontmatter
from .location import discover_home_path_aliases, discover_stream_location, format_local_path
from .models import Stream
from .schema import SchemaFinding, error_messages
from .slug import normalized_slug


def stream_paths(root: Path = SCRATCH_ROOT) -> list[Path]:
    """Return current scratch-first stream files.

    Legacy scratch notes without YAML frontmatter are ignored so `streams list`
    can coexist with older worknotes during migration.
    """
    root = root.expanduser()
    if not root.exists():
        return []
    archive = root / ARCHIVE_DIRNAME
    paths: list[Path] = []
    for path in root.glob("*.md"):
        if archive in path.parents or path.is_symlink():
            continue
        if _has_frontmatter(path):
            paths.append(path)
    return sorted(paths, key=lambda item: item.stem)


def check_paths(root: Path = SCRATCH_ROOT) -> list[Path]:
    """Return all current scratch markdown files for schema checks.

    Unlike `stream_paths()`, this includes legacy files without frontmatter so
    `streams check` reports them as invalid stream records.
    """
    root = root.expanduser()
    if not root.exists():
        return []
    return sorted(
        (path for path in root.glob("*.md") if not path.is_symlink()),
        key=lambda item: item.stem,
    )


def _has_frontmatter(path: Path) -> bool:
    try:
        with path.open() as handle:
            return handle.readline() == "---\n"
    except OSError:
        return False


def load_stream(path: Path) -> Stream:
    try:
        frontmatter, body = split_frontmatter(path.read_text())
    except OSError as exc:
        raise StreamError(f"could not read {path}: {exc}") from exc
    stream = Stream(path=path, frontmatter=frontmatter, body=body)
    stream.validate()
    return stream


def list_streams(root: Path = SCRATCH_ROOT) -> list[Stream]:
    return [load_stream(path) for path in stream_paths(root)]


def show_stream(slug: str, root: Path = SCRATCH_ROOT) -> Stream:
    slug = normalized_slug(slug)
    path = root.expanduser() / f"{slug}.md"
    if not path.exists():
        raise StreamError(f"no stream found for slug {slug!r}")
    return load_stream(path)


def start_stream(title: str, *, root: Path = SCRATCH_ROOT, cwd: Path | None = None) -> Stream:
    title = title.strip()
    if not title:
        raise StreamError("title cannot be empty")
    slug = normalized_slug(title)
    root = root.expanduser()
    path = root / f"{slug}.md"
    if path.exists():
        raise StreamError(f"stream {slug!r} already exists at {path}")

    location = discover_stream_location(cwd or Path.cwd())
    path_aliases = discover_home_path_aliases()
    today = date.today().isoformat()
    frontmatter = {
        "title": title,
        "status": DEFAULT_STATUS,
        "created": today,
        "updated": today,
        "description": "",
        "state": DEFAULT_STATE,
        "repo": format_local_path(location.repo_root, aliases=path_aliases),
        "cwd": format_local_path(location.working_root, aliases=path_aliases),
        "vcs": location.vcs,
        "issues": [],
        "prs": [],
        "sessions": [],
    }
    body = f"""# {title}

## Goal

_Describe the outcome this stream is pursuing._

## Status

This stream has been created and is ready for scoping.

## Decisions

_Record important decisions, reasoning, tradeoffs, and deferred choices._

## Open Questions

_List unresolved questions that affect how to continue._
"""
    try:
        root.mkdir(parents=True, exist_ok=True)
        path.write_text(render_frontmatter(frontmatter, body))
    except OSError as exc:
        raise StreamError(f"could not create {path}: {exc}") from exc
    return load_stream(path)


def update_stream(
    stream: Stream, content: str, *, dry_run: bool = False, if_match: str | None = None
) -> list[SchemaFinding]:
    """Validate complete Markdown and replace an existing stream, optionally guarding its base."""
    path = stream.path
    if if_match is not None and not re.fullmatch(r"[0-9a-f]{64}", if_match):
        raise StreamError("--if-match must be a lowercase SHA-256 digest")
    if path.is_symlink():
        raise StreamError(f"stream source is a symlink: {path}")
    try:
        original = path.read_bytes()
        mode = path.stat().st_mode & 0o777
    except OSError as exc:
        raise StreamError(f"could not read {path}: {exc}") from exc
    if if_match is not None and hashlib.sha256(original).hexdigest() != if_match:
        raise StreamError(f"stream {stream.slug!r} changed since the reviewed draft")

    findings = check_content(path, content)
    errors = error_messages(findings)
    if errors:
        raise StreamError(f"invalid stream {stream.slug!r}: {'; '.join(errors)}")
    if dry_run or original == content.encode("utf-8"):
        return findings

    temporary: Path | None = None
    try:
        fd, filename = tempfile.mkstemp(prefix=f".{stream.slug}.", suffix=".tmp", dir=path.parent)
        temporary = Path(filename)
        with os.fdopen(fd, "wb") as handle:
            handle.write(content.encode("utf-8"))
            handle.flush()
            os.fchmod(handle.fileno(), mode)
            os.fsync(handle.fileno())
        if path.is_symlink() or path.read_bytes() != original:
            raise StreamError(f"stream {stream.slug!r} changed since the reviewed draft")
        temporary.replace(path)
    except OSError as exc:
        raise StreamError(f"could not update {path}: {exc}") from exc
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return findings


def rename_stream(stream: Stream, title: str) -> Stream:
    title = title.strip()
    if not title:
        raise StreamError("title cannot be empty")

    old_path = stream.path
    old_slug = stream.slug
    new_slug = normalized_slug(title)
    new_path = old_path.with_name(f"{new_slug}.md")
    if new_path != old_path and (new_path.exists() or new_path.is_symlink()):
        raise StreamError(f"stream {new_slug!r} already exists at {new_path}")

    artifacts_root = old_path.parent / "artifacts"
    old_artifacts = artifacts_root / old_slug
    new_artifacts = artifacts_root / new_slug
    has_artifacts = old_artifacts.exists() or old_artifacts.is_symlink()
    if has_artifacts and new_artifacts != old_artifacts and (new_artifacts.exists() or new_artifacts.is_symlink()):
        raise StreamError(f"artifact destination already exists: {new_artifacts}")

    if old_path.is_symlink():
        raise StreamError(f"stream source is a symlink: {old_path}")
    if old_artifacts.is_symlink():
        raise StreamError(f"artifact source is a symlink: {old_artifacts}")

    frontmatter = dict(stream.frontmatter)
    frontmatter["title"] = title
    frontmatter["updated"] = date.today().isoformat()
    body = stream.body
    old_heading = f"# {stream.title}\n"
    if body.startswith(old_heading):
        body = f"# {title}\n{body[len(old_heading):]}"
    body = body.replace(
        f"scratch/artifacts/{old_slug}/",
        f"scratch/artifacts/{new_slug}/",
    )
    renamed_text = render_frontmatter(frontmatter, body)

    if new_path == old_path:
        try:
            old_path.write_text(renamed_text)
        except OSError as exc:
            raise StreamError(f"could not rename stream {old_slug!r}: {exc}") from exc
        return load_stream(old_path)

    try:
        original_text = old_path.read_text()
    except OSError as exc:
        raise StreamError(f"could not read stream {old_slug!r}: {exc}") from exc

    file_moved = False
    artifacts_moved = False
    try:
        old_path.write_text(renamed_text)
        old_path.rename(new_path)
        file_moved = True
        old_path.symlink_to(new_path.name)
        if has_artifacts:
            old_artifacts.rename(new_artifacts)
            artifacts_moved = True
            old_artifacts.symlink_to(new_artifacts.name, target_is_directory=True)
    except OSError as exc:
        rollback_errors: list[str] = []

        def rollback(label: str, operation) -> None:
            try:
                operation()
            except OSError as rollback_exc:
                rollback_errors.append(f"{label}: {rollback_exc}")

        if old_artifacts.is_symlink():
            rollback("remove artifact symlink", old_artifacts.unlink)
        if artifacts_moved:
            rollback("restore artifact directory", lambda: new_artifacts.rename(old_artifacts))
        if old_path.is_symlink():
            rollback("remove stream symlink", old_path.unlink)
        if file_moved:
            rollback("restore stream file", lambda: new_path.rename(old_path))
        if old_path.exists() and not old_path.is_symlink():
            rollback("restore stream contents", lambda: old_path.write_text(original_text))

        detail = f"; rollback failed: {'; '.join(rollback_errors)}" if rollback_errors else ""
        raise StreamError(f"could not rename stream {old_slug!r} to {new_slug!r}: {exc}{detail}") from exc
    return load_stream(new_path)


def archive_stream(
    stream: Stream,
    *,
    month: str | None = None,
    force: bool = False,
    dry_run: bool = False,
) -> Path:
    if stream.status != "done" and not force:
        raise StreamError(f"stream {stream.slug!r} has status {stream.status!r}; use --force to archive it")

    month = month or date.today().strftime("%Y-%m")
    if not _valid_archive_month(month):
        raise StreamError(f"archive month {month!r} must look like YYYY-MM")

    archive_dir = stream.path.parent / ARCHIVE_DIRNAME / month
    destination = archive_dir / stream.path.name
    if destination.exists():
        raise StreamError(f"archive destination already exists: {destination}")

    if not dry_run:
        try:
            archive_dir.mkdir(parents=True, exist_ok=True)
            stream.path.rename(destination)
        except OSError as exc:
            raise StreamError(f"could not archive {stream.path}: {exc}") from exc
    return destination


def _valid_archive_month(month: str) -> bool:
    parts = month.split("-")
    if len(parts) != 2:
        return False
    year, month_number = parts
    if len(year) != 4 or len(month_number) != 2:
        return False
    if not year.isdigit() or not month_number.isdigit():
        return False
    return 1 <= int(month_number) <= 12
