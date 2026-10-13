from __future__ import annotations

import io
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.cli import main
from _streams.config import set_config
from _streams.errors import StreamError
from _streams.frontmatter import render_frontmatter
from _streams.resolve import resolve_stream_ref
from _streams.store import list_streams, rename_stream, show_stream


class RenameAliasTest(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)

    def write(self, slug: str) -> Path:
        path = self.root / f"{slug}.md"
        path.write_text(render_frontmatter({
            "title": slug, "status": "active", "description": "Task", "state": "Ready",
            "repo": None, "cwd": str(self.root), "vcs": None, "issues": [], "prs": [], "sessions": [],
        }, ""))
        return path

    def test_exact_alias_follows_repeated_renames_and_returns_canonical_identity(self) -> None:
        self.write("old")
        first = rename_stream(show_stream("old", self.root), "First")
        last = rename_stream(first, "Last")
        self.assertEqual(resolve_stream_ref("old", root=self.root).path, last.path)
        self.assertEqual(resolve_stream_ref("first", root=self.root).slug, "last")
        for slug in ("old", "first", "last"):
            with self.subTest(slug=slug):
                self.assertEqual(resolve_stream_ref(slug, root=self.root, exact=True).path, last.path)
        self.assertEqual([stream.slug for stream in list_streams(self.root)], ["last"])

    def test_invalid_alias_target_reports_invalid_stream(self) -> None:
        target = self.write("target")
        target.write_text(target.read_text().replace("status: active", "status: nope"))
        (self.root / "old.md").symlink_to(target.name)
        with self.assertRaises(StreamError) as error:
            resolve_stream_ref("old", root=self.root)
        self.assertEqual(error.exception.kind, "invalid-stream")
        self.assertIn("target", str(error.exception))

    def test_broken_cyclic_archived_and_external_aliases_fail_closed(self) -> None:
        archive = self.root / "archive"
        archive.mkdir()
        self.write("archived").rename(archive / "archived.md")
        (self.root / "missing.md").symlink_to("absent.md")
        (self.root / "cycle.md").symlink_to("cycle.md")
        (self.root / "old.md").symlink_to("archive/archived.md")
        with tempfile.TemporaryDirectory() as outside:
            external = Path(outside) / "external.md"
            external.write_text("not a current stream")
            (self.root / "external.md").symlink_to(external)
            for slug in ("missing", "cycle", "old", "external"):
                with self.subTest(slug=slug), self.assertRaises(StreamError):
                    resolve_stream_ref(slug, root=self.root)

    def test_broken_exact_alias_does_not_fall_through_to_fuzzy_match(self) -> None:
        self.write("old-other")
        (self.root / "old.md").symlink_to("absent.md")
        with self.assertRaises(StreamError):
            resolve_stream_ref("old", root=self.root)

    def test_cli_show_and_read_follow_alias_to_canonical_note(self) -> None:
        self.write("old")
        renamed = rename_stream(show_stream("old", self.root), "New")
        with patch.dict(os.environ, {"XDG_CONFIG_HOME": str(self.root / "config")}, clear=True):
            set_config("notes.root", str(self.root))
            # This fixture's scratch directory is the configured root's child.
            scratch = self.root / "scratch"
            scratch.mkdir()
            (self.root / "old.md").rename(scratch / "old.md")
            renamed.path.rename(scratch / "new.md")
            for command in ("show", "read"):
                output = io.StringIO()
                with self.subTest(command=command), patch("sys.stdout", output):
                    self.assertEqual(main([command, "old", "--json"]), 0)
                payload = json.loads(output.getvalue())
                self.assertEqual(payload["slug"], "new")

    def test_update_through_alias_preserves_symlink_and_writes_canonical_note(self) -> None:
        self.write("old")
        renamed = rename_stream(show_stream("old", self.root), "New")
        candidate = renamed.path.read_text().replace("state: Ready", "state: Updated")
        from _streams.resolve import resolve_note_path
        self.assertEqual(resolve_note_path("old", root=self.root), renamed.path)
        from _streams.store import update_stream
        update_stream(resolve_note_path("old", root=self.root), candidate)
        self.assertTrue((self.root / "old.md").is_symlink())
        self.assertEqual(resolve_stream_ref("old", root=self.root).state, "Updated")

    def test_aliases_are_not_fuzzy_matched(self) -> None:
        self.write("target")
        (self.root / "old-name.md").symlink_to("target.md")
        with self.assertRaises(StreamError) as error:
            resolve_stream_ref("old", root=self.root)
        self.assertEqual(error.exception.kind, "not-found")


if __name__ == "__main__":
    unittest.main()
