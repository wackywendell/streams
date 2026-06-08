"""Filesystem access for scratch streams."""

from __future__ import annotations

import os
from datetime import date
from pathlib import Path

from .config import ARCHIVE_DIRNAME, DEFAULT_STATE, DEFAULT_STATUS, DEFAULT_VCS, SCRATCH_ROOT
from .errors import StreamError
from .frontmatter import render_frontmatter, split_frontmatter
from .models import Stream
from .slug import normalized_slug


def stream_paths(root: Path = SCRATCH_ROOT) -> list[Path]:
    root = root.expanduser()
    if not root.exists():
        return []
    archive = root / ARCHIVE_DIRNAME
    paths: list[Path] = []
    for path in root.glob("*.md"):
        if archive in path.parents:
            continue
        if _has_frontmatter(path):
            paths.append(path)
    return sorted(paths, key=lambda item: item.stem)


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


def detect_vcs(cwd: Path) -> str:
    current = cwd
    for parent in (current, *current.parents):
        if (parent / ".jj").exists():
            return "jj"
        if (parent / ".git").exists():
            return "git"
    return DEFAULT_VCS


def start_stream(title: str, *, root: Path = SCRATCH_ROOT, cwd: Path | None = None) -> Stream:
    title = title.strip()
    if not title:
        raise StreamError("title cannot be empty")
    slug = normalized_slug(title)
    root = root.expanduser()
    path = root / f"{slug}.md"
    if path.exists():
        raise StreamError(f"stream {slug!r} already exists at {path}")

    cwd = (cwd or Path.cwd()).expanduser().resolve()
    today = date.today().isoformat()
    frontmatter = {
        "title": title,
        "status": DEFAULT_STATUS,
        "created": today,
        "updated": today,
        "description": "",
        "state": DEFAULT_STATE,
        "repo": str(cwd),
        "cwd": str(cwd),
        "vcs": detect_vcs(cwd),
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
