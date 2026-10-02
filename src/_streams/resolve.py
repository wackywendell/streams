"""Stream reference resolution for user-facing commands."""

from __future__ import annotations

from pathlib import Path
from typing import Sequence

from .errors import StreamError
from .models import Stream
from .schema import error_messages
from .slug import normalized_slug, slugify
from .store import check_paths, scan_streams


def resolve_stream_ref(ref: str, *, root: Path | None = None) -> Stream:
    """Resolve a user-facing stream title or slug to one current, valid stream.

    Invalid notes are skipped, except that naming one by its exact slug reports
    why it's invalid rather than matching some other stream.
    """
    streams, invalid = scan_streams(root)
    normalized_ref = normalized_slug(ref)

    for stream in streams:
        if stream.slug == normalized_ref:
            return stream
    broken = [finding for finding in invalid if finding.slug == normalized_ref]
    if broken:
        raise StreamError(
            f"stream {normalized_ref!r} is invalid: {'; '.join(error_messages(broken))}; "
            f"see it with `streams read {normalized_ref}`",
            kind="invalid-stream",
        )

    title_matches = [stream for stream in streams if slugify(stream.title) == normalized_ref]
    if len(title_matches) == 1:
        return title_matches[0]
    if len(title_matches) > 1:
        raise _ambiguous(f"stream reference {ref!r}", [stream.slug for stream in title_matches])

    matches = [
        stream
        for stream in streams
        if normalized_ref in stream.slug or normalized_ref in slugify(stream.title)
    ]
    if len(matches) == 1:
        return matches[0]
    if not matches:
        raise StreamError(f"no stream matches {ref!r} (normalized to {normalized_ref!r})", kind="not-found")
    raise _ambiguous(f"stream reference {ref!r}", [stream.slug for stream in matches])


def resolve_note_path(ref: str, *, root: Path | None = None) -> Path:
    """Resolve a reference to a scratch file, valid or not, so a broken note can be read and fixed.

    Only an exact slug reaches an invalid note: its title may not parse, so title
    and substring matches cover valid streams only.
    """
    slug = normalized_slug(ref)
    for path in check_paths(root):
        if path.stem == slug:
            return path
    return resolve_stream_ref(ref, root=root).path


def resolve_check_path_ref(ref: str, *, root: Path | None = None) -> Path:
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
        raise StreamError(f"no {noun} matches {ref!r} (normalized to {slug!r})", kind="not-found")
    raise _ambiguous(f"{noun} reference {ref!r}", matches)


def _ambiguous(subject: str, slugs: Sequence[str]) -> StreamError:
    candidates = sorted(slugs)
    return StreamError(f"{subject} is ambiguous: {', '.join(candidates)}", kind="ambiguous", candidates=candidates)
