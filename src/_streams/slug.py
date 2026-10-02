"""Slug conventions for scratch streams."""

from __future__ import annotations

import re
import unicodedata

from .errors import StreamError

SLUG_RE = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,62}[a-z0-9])?$")


class SlugError(StreamError):
    def __init__(self, message: str) -> None:
        super().__init__(message, kind="invalid-ref")


def slugify(text: str) -> str:
    value = unicodedata.normalize("NFKD", text).encode("ascii", "ignore").decode("ascii")
    value = value.strip().lower()
    value = re.sub(r"[^a-z0-9]+", "-", value)
    value = re.sub(r"-+", "-", value).strip("-")
    return value


def validate_slug(slug: str) -> None:
    if not SLUG_RE.fullmatch(slug):
        raise SlugError(f"invalid slug {slug!r}; use lowercase kebab-case")


def normalized_slug(text: str) -> str:
    slug = slugify(text)
    validate_slug(slug)
    return slug

