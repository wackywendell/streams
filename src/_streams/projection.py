"""Frontend-neutral stream projection helpers."""

from __future__ import annotations

from pathlib import Path
from urllib.parse import urlencode

from .errors import StreamError
from .models import Stream


def stream_cwd(stream: Stream) -> Path:
    path = stream.location.working_root
    if not path.exists():
        raise StreamError(f"stream {stream.slug!r} cwd does not exist: {path}")
    if not path.is_dir():
        raise StreamError(f"stream {stream.slug!r} cwd is not a directory: {path}")
    return path


def obsidian_uri_for_stream(stream: Stream, *, root: Path, vault: str) -> str:
    try:
        file = stream.path.relative_to(root).as_posix()
    except ValueError as exc:
        raise StreamError(f"stream {stream.slug!r} is outside notes root {root}") from exc
    return f"obsidian://open?{urlencode({'vault': vault, 'file': file})}"


def stream_for_hints(streams: list[Stream], *, title: str, cwd: str, source: str) -> Stream:
    """Use an exact title or unique dedicated cwd, never a fuzzy or shared hint."""
    title_matches = [stream for stream in streams if stream.title == title]
    if len(title_matches) == 1:
        return title_matches[0]
    if len(title_matches) > 1:
        choices = ", ".join(sorted(stream.slug for stream in title_matches))
        raise StreamError(f"{source} title matches multiple streams: {choices}")

    cwd_matches = [stream for stream in streams if cwd and stream.location.matches_working_root(cwd)]
    dedicated_matches = [stream for stream in cwd_matches if stream.location.is_dedicated]
    if len(dedicated_matches) == 1:
        return dedicated_matches[0]
    if len(dedicated_matches) > 1:
        choices = ", ".join(sorted(stream.slug for stream in dedicated_matches))
        raise StreamError(f"{source} matches multiple streams by dedicated cwd; pass a slug: {choices}")
    if cwd_matches:
        raise StreamError(
            f"{source} is in a main checkout or a directory without a repository, which does not identify one stream; "
            "pass a slug"
        )
    raise StreamError(f"{source} title {title!r} does not identify one stream; pass a slug")
