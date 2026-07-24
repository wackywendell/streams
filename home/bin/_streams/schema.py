"""Shared scratch stream frontmatter schema checks."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re
from typing import Any, Literal

from .config import VALID_STATUSES
from .slug import slugify

REQUIRED_FIELDS = ("title", "status", "description", "state", "repo", "cwd", "vcs", "issues", "prs", "sessions")
STRING_FIELDS = ("title", "status", "description", "state", "repo", "cwd", "vcs")
LIST_FIELDS = ("issues", "prs", "sessions")
FORBIDDEN_FIELDS = ("slug", "cmux", "taskwarrior", "current")
SESSION_PREFIXES = ("codex:", "claude:", "pi:")
GITHUB_ISSUE_RE = re.compile(r"^https://github\.com/[^/]+/[^/]+/issues/[0-9]+(?:[/?#].*)?$")
GITHUB_PR_RE = re.compile(r"^https://github\.com/[^/]+/[^/]+/pull/[0-9]+(?:[/?#].*)?$")
FindingLevel = Literal["error", "warn"]


@dataclass(frozen=True)
class SchemaFinding:
    level: FindingLevel
    slug: str
    path: str
    message: str

    def as_json(self) -> dict[str, str]:
        return {
            "level": self.level,
            "slug": self.slug,
            "path": self.path,
            "message": self.message,
        }


def validate_frontmatter(path: Path, frontmatter: dict[str, Any]) -> list[SchemaFinding]:
    findings: list[SchemaFinding] = []
    for field in REQUIRED_FIELDS:
        if field not in frontmatter:
            findings.append(_finding("error", path, f"missing required field {field!r}"))

    for field in FORBIDDEN_FIELDS:
        if field in frontmatter:
            findings.append(_finding("error", path, f"field {field!r} does not belong in stream frontmatter"))

    for field in STRING_FIELDS:
        if field in frontmatter and not isinstance(frontmatter[field], str):
            findings.append(_finding("error", path, f"field {field!r} must be a string"))

    for field in ("title", "status", "state", "cwd", "vcs"):
        if isinstance(frontmatter.get(field), str) and not frontmatter[field].strip():
            findings.append(_finding("error", path, f"field {field!r} cannot be empty"))

    if isinstance(frontmatter.get("status"), str) and frontmatter["status"] not in VALID_STATUSES:
        allowed = ", ".join(sorted(VALID_STATUSES))
        findings.append(_finding("error", path, f"invalid status {frontmatter['status']!r}; expected one of {allowed}"))

    for field in LIST_FIELDS:
        if field in frontmatter and not isinstance(frontmatter[field], list):
            findings.append(_finding("error", path, f"field {field!r} must be a list"))

    for field in ("issues", "prs"):
        if isinstance(frontmatter.get(field), list):
            _check_link_items(path, field, frontmatter[field], findings)

    if isinstance(frontmatter.get("sessions"), list):
        for index, session in enumerate(frontmatter["sessions"]):
            if not isinstance(session, str):
                findings.append(_finding("error", path, f"sessions[{index}] must be a string"))
            elif not session.startswith(SESSION_PREFIXES) or session.endswith(":"):
                findings.append(_finding("error", path, f"sessions[{index}] must look like codex:<id>, claude:<id>, or pi:<id>"))

    title = frontmatter.get("title")
    if isinstance(title, str) and title.strip() and slugify(title) != path.stem:
        findings.append(_finding("warn", path, f"filename slug {path.stem!r} differs from slugified title {slugify(title)!r}"))

    return findings


def error_messages(findings: list[SchemaFinding]) -> list[str]:
    return [finding.message for finding in findings if finding.level == "error"]


def _check_link_items(path: Path, field: str, values: list[Any], findings: list[SchemaFinding]) -> None:
    pattern = GITHUB_ISSUE_RE if field == "issues" else GITHUB_PR_RE
    expected = "GitHub issue URL" if field == "issues" else "GitHub pull request URL"
    for index, value in enumerate(values):
        if not isinstance(value, dict):
            findings.append(_finding("error", path, f"{field}[{index}] must be a mapping with url and description"))
            continue
        url = value.get("url")
        if not isinstance(url, str) or not pattern.fullmatch(url):
            findings.append(_finding("error", path, f"{field}[{index}].url must be a {expected}"))
        description = value.get("description")
        if not isinstance(description, str):
            findings.append(_finding("error", path, f"{field}[{index}].description must be a string"))
        elif not 2 <= len(description.split()) <= 5:
            findings.append(_finding("error", path, f"{field}[{index}].description must contain 2 to 5 words"))


def _finding(level: FindingLevel, path: Path, message: str) -> SchemaFinding:
    return SchemaFinding(level=level, slug=path.stem, path=str(path), message=message)
