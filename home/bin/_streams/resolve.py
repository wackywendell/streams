"""Stream reference resolution for user-facing commands."""

from __future__ import annotations

from pathlib import Path
from typing import Sequence

from .config import SCRATCH_ROOT
from .errors import StreamError
from .models import Stream
from .slug import normalized_slug, slugify
from .store import check_paths, list_streams


def resolve_stream_ref(ref: str, *, root: Path = SCRATCH_ROOT) -> Stream:
    """Resolve a user-facing stream title or slug to one current stream."""
    streams = list_streams(root)
    normalized_ref = normalized_slug(ref)

    for stream in streams:
        if stream.slug == normalized_ref:
            return stream

    title_matches = [stream for stream in streams if slugify(stream.title) == normalized_ref]
    if len(title_matches) == 1:
        return title_matches[0]
    if len(title_matches) > 1:
        formatted = ", ".join(sorted(stream.slug for stream in title_matches))
        raise StreamError(f"stream reference {ref!r} is ambiguous: {formatted}")

    matches = [
        stream
        for stream in streams
        if normalized_ref in stream.slug or normalized_ref in slugify(stream.title)
    ]
    if len(matches) == 1:
        return matches[0]
    if not matches:
        raise StreamError(f"no stream matches {ref!r} (normalized to {normalized_ref!r})")
    formatted = ", ".join(sorted(stream.slug for stream in matches))
    raise StreamError(f"stream reference {ref!r} is ambiguous: {formatted}")


def resolve_check_path_ref(ref: str, *, root: Path = SCRATCH_ROOT) -> Path:
    """Resolve a user-facing stream reference to a scratch file path for checks."""
    paths = check_paths(root)
    slug = resolve_slug_ref(ref, [path.stem for path in paths], noun="scratch file")
    for path in paths:
        if path.stem == slug:
            return path
    raise StreamError(f"resolved scratch file {slug!r} was not found")


def resolve_slug_ref(ref: str, choices: Sequence[str], *, noun: str = "stream") -> str:
    """Resolve `ref` by normalized exact match, then unique substring match."""
    slug = normalized_slug(ref)
    if slug in choices:
        return slug

    matches = [choice for choice in choices if slug in choice]
    if len(matches) == 1:
        return matches[0]
    if not matches:
        raise StreamError(f"no {noun} matches {ref!r} (normalized to {slug!r})")
    formatted = ", ".join(sorted(matches))
    raise StreamError(f"{noun} reference {ref!r} is ambiguous: {formatted}")
