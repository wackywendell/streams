from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

SRC = Path(__file__).resolve().parents[2] / "src"
sys.path.insert(0, str(SRC))

from _streams.errors import StreamError
from _streams.frontmatter import render_frontmatter
from _streams.herdr_binding import write_binding
from _streams.herdr_links import LinksView
from _streams.models import Stream
from _streams.resolve import resolve_stream_ref
from _streams.store import archive_stream, rename_stream


class ExactIdentityTest(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.scratch = self.root / "notes" / "scratch"
        self.scratch.mkdir(parents=True)
        config = self.root / "config" / "streams" / "config.json"
        config.parent.mkdir(parents=True)
        config.write_text(json.dumps({"notes": {"root": str(self.scratch.parent)}}))
        environment = patch.dict(os.environ, {
            "HOME": str(self.root), "XDG_CONFIG_HOME": str(config.parents[1]),
            "PYTHONPATH": str(SRC),
        }, clear=True)
        environment.start()
        self.addCleanup(environment.stop)
        self.binding = self.root / "binding.json"
        write_binding(self.binding, "topic")

    def write(self, slug: str, title: str = "Topic") -> Stream:
        stream = Stream.parse(self.scratch / f"{slug}.md", {
            "title": title, "status": "active", "description": "Task", "state": "Ready",
            "repo": None, "cwd": str(self.root), "vcs": None,
            "issues": [], "prs": [], "sessions": [],
        }, "")
        stream.path.write_text(render_frontmatter(stream.frontmatter, stream.body))
        return stream

    def cli(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, "-P", "-m", "_streams.cli", *args],
            env=dict(os.environ), check=False, capture_output=True, text=True, timeout=10,
        )

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
                    result = self.cli("show", "topic", "--exact", "--json")
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(json.loads(result.stdout)["error"]["kind"], "not-found")

    def test_exact_input_is_validated_without_normalizing_titles_or_paths(self) -> None:
        self.write("topic")
        for ref in ("Topic", " topic ", "Topic!", "../topic", "topic.md", "a/../topic", "", "a" * 65):
            with self.subTest(ref=ref):
                with self.assertRaises(StreamError) as error:
                    resolve_stream_ref(ref, root=self.scratch, exact=True)
                self.assertEqual(error.exception.kind, "invalid-ref")
                result = self.cli("show", ref, "--exact", "--json")
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(json.loads(result.stdout)["error"]["kind"], "invalid-ref")
        self.assertEqual(resolve_stream_ref("Topic!", root=self.scratch).slug, "topic")

    def test_exact_canonical_and_repeated_rename_aliases_return_current_identity(self) -> None:
        current = self.write("topic")
        self.assertEqual(resolve_stream_ref("topic", root=self.scratch, exact=True).path, current.path)
        first = rename_stream(current, "First")
        current = rename_stream(first, "Last")
        for ref in ("topic", "first", "last"):
            with self.subTest(ref=ref):
                self.assertEqual(resolve_stream_ref(ref, root=self.scratch, exact=True).path, current.path)
                result = self.cli("show", ref, "--exact", "--json")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(json.loads(result.stdout)["slug"], "last")
                text = self.cli("show", ref, "--exact")
                self.assertEqual(text.returncode, 0, text.stderr)
                self.assertIn("slug:        last", text.stdout)

    def test_real_cli_view_refuses_open_after_bound_identity_disappears(self) -> None:
        real_run = subprocess.run
        for archived in (False, True):
            for title in ("Topic", "Related topic"):
                with self.subTest(archived=archived, title=title):
                    bound = self.write("topic")
                    view = LinksView(self.binding)
                    self.assertIn("Topic · active", view.refresh())
                    if archived:
                        archive_stream(bound, month="2026-01" if title == "Topic" else "2026-02", force=True)
                    else:
                        bound.path.unlink()
                    self.write("topic-other", title)

                    def cli_only(argv, **kwargs):
                        if argv[:4] != [sys.executable, "-P", "-m", "_streams.cli"]:
                            raise AssertionError(f"Unexpected native command or OS opener: {argv}")
                        return real_run(argv, **kwargs)

                    with patch("_streams.herdr_links.subprocess.run", side_effect=cli_only) as run:
                        self.assertIn("Stream unavailable:", view.refresh())
                        self.assertIsNone(view.stream)
                        with self.assertRaisesRegex(StreamError, "unavailable"):
                            view.open_note()
                    self.assertTrue(all(call.args[0][4] == "show" for call in run.call_args_list))

    def test_real_cli_view_rejects_invalid_notes_and_unsafe_aliases(self) -> None:
        competitor = self.write("topic-other")
        real_run = subprocess.run

        def cli_only(argv, **kwargs):
            if argv[:4] != [sys.executable, "-P", "-m", "_streams.cli"]:
                raise AssertionError(f"Unexpected native command or OS opener: {argv}")
            return real_run(argv, **kwargs)

        target = self.write("target", "Target")
        archive = self.scratch / "archive"
        archive.mkdir()
        archived = archive / "archived.md"
        archived.write_text(target.path.read_text())
        external = self.root / "external.md"
        external.write_text(target.path.read_text())
        for kind in ("invalid", "broken", "external", "cyclic", "archive", "invalid-alias", "invalid-ref"):
            with self.subTest(kind=kind):
                bound = self.scratch / "topic.md"
                bound.unlink(missing_ok=True)
                write_binding(self.binding, "topic")
                if kind == "invalid":
                    bound.write_text(render_frontmatter({**competitor.frontmatter, "status": "nope"}, ""))
                elif kind == "invalid-ref":
                    self.binding.write_text(json.dumps({"stream": "../topic-other"}))
                else:
                    destination = {
                        "broken": "absent.md", "external": external, "cyclic": "topic.md",
                        "archive": "archive/archived.md", "invalid-alias": "invalid.md",
                    }[kind]
                    if kind == "invalid-alias":
                        (self.scratch / "invalid.md").write_text(render_frontmatter({**target.frontmatter, "status": "nope"}, ""))
                    bound.symlink_to(destination)
                view = LinksView(self.binding)
                with patch("_streams.herdr_links.subprocess.run", side_effect=cli_only):
                    self.assertIn("Stream unavailable:", view.refresh())
                    self.assertIsNone(view.stream)
                    with self.assertRaisesRegex(StreamError, "unavailable"):
                        view.open_note()


if __name__ == "__main__":
    unittest.main()
