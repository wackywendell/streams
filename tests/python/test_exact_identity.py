from __future__ import annotations

from dataclasses import replace
import sys
import unittest
from pathlib import Path

SRC = Path(__file__).resolve().parents[2] / "src"
sys.path.insert(0, str(SRC))

from _streams.errors import StreamError
from _streams.catalog.model import Session, Stream as CatalogStream
from _streams.cli import resolve


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
