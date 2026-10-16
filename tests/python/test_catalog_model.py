from __future__ import annotations

from dataclasses import FrozenInstanceError, replace
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.catalog.model import (
    Catalog, Checkout, Document, Link, Repository, Session, Stream, ValidationError,
)


def stream(**changes):
    return replace(Stream(id="stream-1", title="Catalog", goal="Understand ongoing work", owner_host_id="laptop"), **changes)


def session(**changes):
    return replace(Session(id="session-1", host_id="laptop", stream_id="stream-1", harness="pi",
                           harness_session_id="native-1", title="Lifecycle design"), **changes)


class CatalogModelTest(unittest.TestCase):
    def test_minimal_records_do_not_require_storage_or_runtime_state(self):
        effort, contribution = stream(), session()
        catalog = Catalog(streams=(effort,), sessions=(contribution,))
        self.assertEqual(catalog.stream("stream-1"), effort)
        self.assertEqual(catalog.session("laptop", "session-1"), contribution)
        self.assertEqual(contribution.state, "")
        self.assertEqual(contribution.remaining, "")
        self.assertFalse(hasattr(effort, "state"))
        self.assertFalse(hasattr(effort, "path"))
        self.assertFalse(hasattr(contribution, "sequence"))

    def test_archive_is_independent_of_lifecycle(self):
        for status in ("active", "waiting", "blocked", "parked", "done"):
            self.assertEqual(stream(status=status, archived=True).status, status)

    def test_records_own_their_immutable_collections(self):
        repositories = {"code": Repository(url="https://example.org/code")}
        documents = [Document(repository="code", ref="feature", path="docs/design.md", description="Design")]
        effort = stream(repositories=repositories, documents=documents)
        repositories.clear()
        documents.clear()
        self.assertIn("code", effort.repositories)
        self.assertEqual(len(effort.documents), 1)
        with self.assertRaises(TypeError):
            effort.repositories["other"] = Repository(url="https://example.org/other")
        with self.assertRaises(FrozenInstanceError):
            effort.title = "Changed"

    def test_ids_are_logical_values_not_filenames(self):
        effort = stream(id="effort / planning")
        self.assertEqual(effort.id, "effort / planning")

    def test_invalid_scalars_are_rejected_at_construction(self):
        for changes in ({"id": " "}, {"title": ""}, {"goal": 42}, {"status": "running"},
                        {"archived": 1}, {"title_mode": "other"}, {"goal_short": " "},
                        {"owner_host_id": None}, {"repositories": {"code": "not a repository"}},
                        {"issues": ["https://example.org"]}):
            with self.subTest(changes=changes), self.assertRaises(ValidationError):
                stream(**changes)
        for changes in ({"state": None}, {"remaining": False}, {"harness_session_id": ""},
                        {"status": "running"}, {"checkout": "local-path"}):
            with self.subTest(changes=changes), self.assertRaises(ValidationError):
                session(**changes)

    def test_portable_document_and_checkout_paths(self):
        for path in ("/tmp/plan.md", "../plan.md", "docs/../../plan.md", "C:\\plan.md", "docs\\plan.md"):
            with self.subTest(path=path), self.assertRaises(ValidationError):
                Document(repository="code", ref="main", path=path, description="Plan")
        self.assertEqual(Checkout(repository="code", ref="main").directory_within_checkout, ".")

    def test_references_must_resolve_within_the_stream(self):
        doc = Document(repository="missing", ref="main", path="plan.md", description="Plan")
        with self.assertRaises(ValidationError):
            stream(documents=(doc,))
        checkout = Checkout(repository="missing", ref="main")
        with self.assertRaises(ValidationError):
            Catalog(streams=(stream(),), sessions=(session(checkout=checkout),))

    def test_native_session_identity_and_record_identity_are_unique(self):
        original = session()
        for other in (replace(original, title="Duplicate"), replace(original, id="another")):
            with self.subTest(other=other), self.assertRaises(ValidationError):
                Catalog(streams=(stream(),), sessions=(original, other))
        # Native identities are host-bound, not globally unique.
        Catalog(streams=(stream(),), sessions=(original, replace(original, host_id="server")))

    def test_sessions_require_an_existing_stream_but_may_be_historical(self):
        with self.assertRaises(ValidationError):
            Catalog(sessions=(session(),))
        Catalog(streams=(stream(owner_host_id="server"),), sessions=(session(),))

    def test_link_and_reference_shapes_are_validated(self):
        with self.assertRaises(ValidationError):
            Link(url="", description="Issue")
        with self.assertRaises(ValidationError):
            stream(issues=(Link(url="https://example.org/1", description="Issue"),) * 2)


if __name__ == "__main__":
    unittest.main()
