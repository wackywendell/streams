from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))
from _streams.catalog.model import Document, Repository, Session, Stream
from _streams.catalog_checkouts import CheckoutRegistry
from _streams.catalog_locations import DirectoryBindings
from _streams.errors import StreamError


class DirectoryBindingsTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.registry = CheckoutRegistry(self.root / "registry", "host")
        self.locations = DirectoryBindings(self.root / "runtime", self.registry)
        self.stream = Stream("stream", "Work", None, "host", repositories={"code": Repository("repo"), "notes": Repository("notes")},
                             documents=(Document("code", "old-ref", "doc.md", "Design"), Document("notes", "main", "doc.md", "Notes")))
        self.session = Session("session", "host", "stream", "pi", "native", "Work")

    def repo(self, name, url):
        path = self.root / name
        subprocess.run(["git", "init", "--quiet", str(path)], check=True, capture_output=True, timeout=30)
        (path / "doc.md").write_text(name)
        return path, self.registry.register(path, repository_url=url)

    def context(self):
        return self.locations.document_context(self.stream, self.session)

    def test_selected_directory_is_a_checkout_id_and_relative_cwd(self):
        root, checkout = self.repo("code", "repo")
        (root / "src").mkdir()
        selected = self.locations.remember("stream", root / "src", session_id="session", repository_url="repo")
        self.assertEqual((selected.checkout_id, selected.relative_cwd), (checkout.id, "src"))
        data = json.loads(self.locations.path("stream", "session").read_text())
        self.assertEqual(data["location"], {"kind": "checkout", "id": checkout.id, "within": "src", "repository_url": "repo"})
        moved = self.root / "moved"
        root.rename(moved)
        self.registry.move(checkout.id, moved)
        self.assertEqual(self.locations.read("stream", "session").cwd, moved / "src")

    def test_exact_session_precedence_external_default_and_missing_file_no_fallback(self):
        root, selected = self.repo("selected", "repo")
        _, fallback = self.repo("fallback", "repo")
        notes, external = self.repo("notes", "notes")
        self.registry.set_default("repo", fallback.id)
        self.registry.set_default("notes", external.id)
        self.locations.remember("stream", root, session_id="session")
        self.assertEqual([row["file"] for row in self.context()["document_locations"]], [str(root / "doc.md"), str(notes / "doc.md")])
        (root / "doc.md").unlink()
        document = self.context()["document_locations"][0]
        self.assertEqual(document["checkout_id"], selected.id)
        self.assertIsNone(document["file"])
        self.assertIn("Document not present in this checkout", document["error"])

    def test_two_sessions_in_one_stream_have_independent_roots(self):
        first, _ = self.repo("first", "repo")
        second, _ = self.repo("second", "repo")
        other = Session("second-session", "host", "stream", "pi", "native-2", "Other")
        self.locations.remember("stream", first, session_id=self.session.id)
        self.locations.remember("stream", second, session_id=other.id)
        one = self.locations.document_context(self.stream, self.session)["document_locations"][0]
        two = self.locations.document_context(self.stream, other)["document_locations"][0]
        self.assertNotEqual(one["checkout_id"], two["checkout_id"])
        self.assertEqual(one["file"], str(first / "doc.md"))
        self.assertEqual(two["file"], str(second / "doc.md"))

    def test_read_does_not_register_bindings_and_does_not_use_another_sessions_cwd(self):
        root, checkout = self.repo("default", "repo")
        self.registry.set_default("repo", checkout.id)
        self.locations.remember("stream", root)
        before = self.registry.path.read_bytes()
        context = self.context()
        self.assertEqual(context["checkouts"], [])
        self.assertTrue(all(row["file"] is None for row in context["document_locations"]))
        self.assertFalse(self.locations.path("stream", "session").exists())
        self.assertEqual(self.registry.path.read_bytes(), before)

    def test_if_missing_preserves_explicit_choice(self):
        first, _ = self.repo("first", "repo")
        second, _ = self.repo("second", "repo")
        self.locations.remember("stream", first, session_id="session")
        before = self.locations.path("stream", "session").read_bytes()
        self.locations.remember("stream", second, session_id="session", if_missing=True)
        self.assertEqual(self.locations.path("stream", "session").read_bytes(), before)
        self.assertEqual(self.locations.read("stream", "session").cwd, first)

    def test_explicit_selection_repairs_deleted_directory_without_replacing_corrupt_intent(self):
        old = self.root / "deleted"
        old.mkdir()
        self.locations.remember("stream", old, session_id="session")
        path = self.locations.path("stream", "session")
        old.rmdir()
        before = path.read_bytes()
        with self.assertRaises(StreamError):
            self.locations.read("stream", "session")
        with self.assertRaises(StreamError):
            self.locations.remember("stream", self.root, session_id="session", if_missing=True)
        self.assertEqual(path.read_bytes(), before)
        self.locations.remember("stream", self.root, session_id="session")
        self.assertEqual(self.locations.read("stream", "session").cwd, self.root)

    def test_explicit_selection_repairs_missing_checkout_or_within_directory(self):
        for remove_root in (False, True):
            with self.subTest(remove_root=remove_root):
                root, checkout = self.repo(f"repair-{remove_root}", "repo")
                (root / "src").mkdir()
                self.locations.remember("stream", root / "src", session_id="session")
                if remove_root:
                    root.rename(self.root / f"moved-{remove_root}")
                else:
                    (root / "src").rmdir()
                with self.assertRaises(StreamError):
                    self.locations.read("stream", "session")
                self.locations.remember("stream", self.root, session_id="session")
                self.assertEqual(self.locations.read("stream", "session").cwd, self.root)

    def test_invalid_old_relative_selection_cannot_be_overwritten(self):
        root, checkout = self.repo("invalid-old-selection", "repo")
        self.locations.remember("stream", root, session_id="session")
        path = self.locations.path("stream", "session")
        data = json.loads(path.read_text())
        data["location"]["within"] = "../escape"
        path.write_text(json.dumps(data))
        before = path.read_bytes()
        with self.assertRaisesRegex(StreamError, "nonescaping"):
            self.locations.remember("stream", self.root, session_id="session")
        self.assertEqual(path.read_bytes(), before)

    def test_unbound_checkout_does_not_guess_repository_from_sole_default(self):
        path = self.root / "unbound"
        subprocess.run(["git", "init", "--quiet", str(path)], check=True, capture_output=True, timeout=30)
        self.locations.remember("stream", path, session_id="session")
        _, fallback = self.repo("fallback", "repo")
        self.registry.set_default("repo", fallback.id)
        self.assertIn("no repository binding", self.context()["document_locations"][0]["error"])

    def test_corrupt_binding_is_not_overwritten_and_failed_selection_remains_visible(self):
        path = self.locations.path("stream", "session")
        path.parent.mkdir(parents=True)
        path.write_text('{"version":2,"stream_id":"stream","session_id":"session","location":{"kind":"guess"}}')
        before = path.read_bytes()
        with self.assertRaises(StreamError):
            self.locations.remember("stream", self.root, session_id="session")
        self.assertEqual(path.read_bytes(), before)
        self.assertIn("invalid checkout directory binding", self.context()["document_locations"][0]["error"])

    def test_directories_outside_vcs_are_valid_and_use_external_defaults(self):
        self.locations.remember("stream", self.root, session_id="session")
        root, checkout = self.repo("external", "repo")
        self.registry.set_default("repo", checkout.id)
        self.assertEqual(self.context()["document_locations"][0]["file"], str(root / "doc.md"))

    def test_document_fallback_resolves_origin_session_checkout_and_vcs_commit(self):
        from dataclasses import replace
        one_root, _ = self.repo("one", "repo")
        two_root, two_chk = self.repo("two", "repo")
        (two_root / "branch-only.md").write_text("from worktree two\n")
        subprocess.run(["git", "add", "branch-only.md"], cwd=two_root, check=True, capture_output=True, timeout=30)
        subprocess.run(["git", "-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-m", "add branch-only"], cwd=two_root, check=True, capture_output=True, timeout=30)
        commit_sha = subprocess.run(["git", "rev-parse", "HEAD"], cwd=two_root, check=True, capture_output=True, text=True, timeout=30).stdout.strip()
        other = replace(self.session, id="other", harness_session_id="native-2")
        self.locations.remember("stream", one_root, session_id="session", repository_url="repo")
        self.locations.remember("stream", two_root, session_id="other", repository_url="repo")
        stream = replace(
            self.stream,
            documents=(Document("code", "main", "branch-only.md", "Branch doc", commit=commit_sha, source_session_id="other"),),
        )
        ctx = self.locations.document_context(stream, self.session, sessions=(self.session, other))
        loc = ctx["document_locations"][0]
        self.assertIsNone(loc["error"])
        self.assertEqual(loc["checkout_id"], two_chk.id)
        self.assertEqual(loc["file"], str(two_root / "branch-only.md"))
        self.assertEqual([a["kind"] for a in loc["alternates"]], ["origin", "revision"])
        (two_root / "branch-only.md").unlink()
        ctx_after_unlink = self.locations.document_context(stream, self.session, sessions=(self.session, other))
        loc_after = ctx_after_unlink["document_locations"][0]
        self.assertIsNone(loc_after["error"])
        self.assertEqual(loc_after["alternates"][0]["kind"], "revision")
        self.assertEqual(Path(loc_after["file"]).read_text(), "from worktree two\n")



if __name__ == "__main__":
    unittest.main()
