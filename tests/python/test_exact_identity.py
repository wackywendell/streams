from __future__ import annotations

from dataclasses import replace
import sys
import tempfile
import unittest
from pathlib import Path

SRC = Path(__file__).resolve().parents[2] / "src"
sys.path.insert(0, str(SRC))

from _streams.errors import StreamError
from _streams.frontmatter import render_frontmatter
from _streams.catalog.model import Session, Stream as CatalogStream
from _streams.cli import resolve
from _streams.models import Stream
from _streams.resolve import resolve_stream_ref
from _streams.store import archive_stream, rename_stream


class ScratchExactIdentityTest(unittest.TestCase):
    """Retained scratch resolver contract, not the catalog CLI interface."""

    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.scratch = self.root / "notes" / "scratch"
        self.scratch.mkdir(parents=True)

    def write(self, slug: str, title: str = "Topic") -> Stream:
        stream = Stream.parse(self.scratch / f"{slug}.md", {
            "title": title, "status": "active", "description": "Task", "state": "Ready",
            "repo": None, "cwd": str(self.root), "vcs": None,
            "issues": [], "prs": [], "sessions": [],
        }, "")
        stream.path.write_text(render_frontmatter(stream.frontmatter, stream.body))
        return stream

    def test_exact_missing_or_archived_identity_never_uses_title_or_substring(self) -> None:
        for archived in (False, True):
            for title in ("Topic", "Related topic"):
                with self.subTest(archived=archived, title=title):
                    bound = self.write("topic")
                    if archived:
                        archive_stream(bound, month="2026-01" if title == "Topic" else "2026-02", force=True)
                    else:
                        bound.path.unlink()
                    competitor = self.write("topic-other", title)
                    self.assertEqual(resolve_stream_ref("topic", root=self.scratch).slug, competitor.slug)
                    with self.assertRaises(StreamError) as error:
                        resolve_stream_ref("topic", root=self.scratch, exact=True)
                    self.assertEqual(error.exception.kind, "not-found")

    def test_exact_input_is_validated_without_normalizing_titles_or_paths(self) -> None:
        self.write("topic")
        for ref in ("Topic", " topic ", "Topic!", "../topic", "topic.md", "a/../topic", "", "a" * 65):
            with self.subTest(ref=ref):
                with self.assertRaises(StreamError) as error:
                    resolve_stream_ref(ref, root=self.scratch, exact=True)
                self.assertEqual(error.exception.kind, "invalid-ref")
        self.assertEqual(resolve_stream_ref("Topic!", root=self.scratch).slug, "topic")

    def test_exact_canonical_and_repeated_rename_aliases_return_current_identity(self) -> None:
        current = self.write("topic")
        self.assertEqual(resolve_stream_ref("topic", root=self.scratch, exact=True).path, current.path)
        first = rename_stream(current, "First")
        current = rename_stream(first, "Last")
        for ref in ("topic", "first", "last"):
            with self.subTest(ref=ref):
                self.assertEqual(resolve_stream_ref(ref, root=self.scratch, exact=True).path, current.path)


class CatalogReferenceTest(unittest.TestCase):
    def test_id_precedes_exact_title_and_survives_repeated_title_updates(self):
        original = CatalogStream("stable-id", "Original", None, "host")
        competitor = CatalogStream("competitor", original.id, None, "host")
        for title in ("Original", "First", "Last"):
            renamed = replace(original, title=title)
            self.assertIs(resolve((competitor, renamed), original.id), renamed)
            self.assertIs(resolve((competitor, renamed), title), renamed)

    def test_title_matching_is_literal_and_does_not_normalize_slug_or_paths(self):
        record = CatalogStream("stable-id", "Topic!", None, "host")
        self.assertIs(resolve((record,), "TOPIC!"), record)
        for ref in ("topic.md", "../topic", " topic ", "topic-title"):
            with self.subTest(ref=ref), self.assertRaises(StreamError) as raised:
                resolve((record,), ref)
            self.assertEqual(raised.exception.kind, "not-found")
        for ref in ("", " "):
            with self.subTest(ref=ref), self.assertRaises(StreamError) as raised:
                resolve((record,), ref)
            self.assertEqual(raised.exception.kind, "invalid-ref")

    def test_ambiguous_titles_return_stable_candidate_ids(self):
        first = CatalogStream("one", "Topic", None, "host")
        second = CatalogStream("two", "Topic", None, "host")
        for ref in ("Topic", "top"):
            with self.subTest(ref=ref), self.assertRaises(StreamError) as raised:
                resolve((first, second), ref)
            self.assertEqual(raised.exception.kind, "ambiguous")
            self.assertEqual(raised.exception.candidates, ["one", "two"])

    def test_host_qualified_session_identity_disambiguates_shared_local_id(self):
        first = Session("session", "host-one", "stream", "pi", "native", "Implementation")
        second = replace(first, host_id="host-two")
        self.assertIs(resolve((first, second), "host-one/session"), first)
        with self.assertRaises(StreamError) as raised:
            resolve((first, second), "session")
        self.assertEqual(raised.exception.candidates, ["host-one/session", "host-two/session"])


if __name__ == "__main__":
    unittest.main()
