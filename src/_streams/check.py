"""Check command support for scratch stream frontmatter."""

from __future__ import annotations

from pathlib import Path

import yaml

from .errors import StreamError
from .frontmatter import split_frontmatter
from .schema import SchemaFinding, validate_frontmatter


def check_path(path: Path) -> list[SchemaFinding]:
    """Validate one file without constructing a Stream.

    Check must report malformed frontmatter, so it intentionally avoids
    `store.load_stream()`, which fails fast on the first validation error.
    """
    try:
        content = path.read_text()
    except OSError as exc:
        return [SchemaFinding(level="error", slug=path.stem, path=str(path), message=f"could not parse frontmatter: {exc}")]
    return check_content(path, content)


def check_content(path: Path, content: str) -> list[SchemaFinding]:
    """Validate candidate markdown against its intended scratch path without writing it."""
    try:
        frontmatter, _body = split_frontmatter(content)
    except (StreamError, yaml.YAMLError) as exc:
        return [SchemaFinding(level="error", slug=path.stem, path=str(path), message=f"could not parse frontmatter: {exc}")]
    return validate_frontmatter(path, frontmatter)


def check_streams(paths: list[Path]) -> list[SchemaFinding]:
    """Validate paths and return all findings instead of failing fast."""
    return [finding for path in paths for finding in check_path(path)]


def findings_json(findings: list[SchemaFinding]) -> str:
    import json

    return json.dumps([finding.as_json() for finding in findings], indent=2, sort_keys=True)
