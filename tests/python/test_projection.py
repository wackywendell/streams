from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
import sys
from urllib.parse import parse_qs, urlparse

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))

from _streams.errors import StreamError
from _streams.models import Stream
from _streams.projection import obsidian_uri_for_stream, stream_cwd, stream_for_hints


def make_stream(slug: str, *, root: Path = Path("/notes/scratch"), **fields: object) -> Stream:
    return Stream.parse(root / f"{slug}.md", {
        "title": slug, "status": "active", "description": "Task", "state": "Ready",
        "vcs": "jj", "repo": "/repo", "cwd": "/repo", "issues": [], "prs": [], "sessions": [],
        **fields,
    }, "")


class ProjectionTest(unittest.TestCase):
    def test_cwd_validation_preserves_recorded_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp)
            stream = make_stream("writing", vcs=None, repo=None, cwd=str(path))
            self.assertEqual(stream_cwd(stream), path)

    def test_cwd_validation_rejects_missing_path_and_file(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            file = Path(tmp) / "file"
            file.write_text("text")
            for path, message in ((Path(tmp) / "missing", "does not exist"), (file, "not a directory")):
                with self.subTest(path=path), self.assertRaisesRegex(StreamError, message):
                    stream_cwd(make_stream("stream", cwd=str(path)))

    def test_raw_uri_encodes_vault_and_relative_note_without_markdown(self) -> None:
        stream = make_stream("note & question", root=Path("/notes/scratch"))
        uri = obsidian_uri_for_stream(stream, root=Path("/notes"), vault="Work & notes")
        parsed = urlparse(uri)
        self.assertEqual((parsed.scheme, parsed.netloc), ("obsidian", "open"))
        self.assertEqual(parse_qs(parsed.query), {"vault": ["Work & notes"], "file": ["scratch/note & question.md"]})
        self.assertNotIn("[", uri)

    def test_raw_uri_rejects_notes_outside_root(self) -> None:
        with self.assertRaisesRegex(StreamError, "outside notes root"):
            obsidian_uri_for_stream(make_stream("stream"), root=Path("/other"), vault="Notes")

    def test_exact_title_takes_precedence_over_cwd(self) -> None:
        titled = make_stream("titled", title="Exact")
        by_cwd = make_stream("by-cwd", cwd="/repo/topic")
        self.assertIs(stream_for_hints([titled, by_cwd], title="Exact", cwd="/repo/topic", source="workspace w1"), titled)

    def test_duplicate_titles_are_not_disambiguated_by_cwd(self) -> None:
        streams = [make_stream("first", title="Exact"), make_stream("second", title="Exact", cwd="/repo/topic")]
        with self.assertRaisesRegex(StreamError, "title matches multiple streams: first, second"):
            stream_for_hints(streams, title="Exact", cwd="/repo/topic", source="workspace w1")

    def test_unique_dedicated_cwd_matches_without_fuzzy_title(self) -> None:
        stream = make_stream("stream", title="Code review", cwd="/repo/topic")
        self.assertIs(stream_for_hints([stream], title="Unrelated", cwd="/repo/topic", source="workspace w1"), stream)

    def test_duplicate_dedicated_cwd_requires_slug(self) -> None:
        streams = [make_stream("second", cwd="/repo/topic"), make_stream("first", cwd="/repo/topic")]
        with self.assertRaisesRegex(StreamError, "dedicated cwd; pass a slug: first, second"):
            stream_for_hints(streams, title="Other", cwd="/repo/topic", source="workspace w1")

    def test_shared_main_or_repo_less_cwd_never_identifies_stream(self) -> None:
        for stream in (make_stream("main"), make_stream("writing", vcs=None, repo=None)):
            with self.subTest(slug=stream.slug), self.assertRaisesRegex(StreamError, "main checkout.*pass a slug"):
                stream_for_hints([stream], title="Other", cwd="/repo", source="workspace w1")

    def test_no_hint_or_partial_title_requires_slug(self) -> None:
        stream = make_stream("review", title="Code review", cwd="/repo/topic")
        for title, cwd in (("review", "/missing"), ("", "")):
            with self.subTest(title=title), self.assertRaisesRegex(StreamError, "workspace w1.*does not identify one stream; pass a slug"):
                stream_for_hints([stream], title=title, cwd=cwd, source="workspace w1")


if __name__ == "__main__":
    unittest.main()
