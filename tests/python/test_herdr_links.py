from __future__ import annotations

import dataclasses
import json
import os
import re
import subprocess
import sys
import tempfile
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

SRC = Path(__file__).resolve().parents[2] / "src"
sys.path.insert(0, str(SRC))

from _streams.errors import StreamError
from _streams.herdr_binding import read_binding, write_binding
from _streams.herdr_links import LinksView, NotesConfig, ShortcutInput, note_uri, render
from _streams.models import Link, Stream


def example(root: Path = Path("/notes/scratch"), **fields: object) -> Stream:
    return Stream.parse(root / "example.md", {
        "title": "Example stream", "status": "active", "description": "Task", "state": "Ready for review.",
        "repo": None, "cwd": str(root), "vcs": None, "issues": [], "prs": [], "sessions": [],
        **fields,
    }, "")


class LinksRenderTest(unittest.TestCase):
    def test_stream_without_external_links_still_has_note_action(self) -> None:
        rendered = render(example())
        visible = re.sub(r"\x1b\[[0-9;]*m", "", rendered)
        self.assertEqual(visible.splitlines(), [
            "[n] Example stream · active", "Ready for review.", "No linked issues or PRs.",
        ])
        self.assertIn("\x1b[1mExample stream\x1b[0m", rendered)
        self.assertIn("\x1b[3;90mNo linked issues or PRs.\x1b[0m", rendered)
        self.assertNotIn("Ctrl-click", rendered)

    def test_issue_and_pr_labels_are_osc8_http_links(self) -> None:
        stream = example(issues=[{"url": "https://github.com/example/repo/issues/42", "description": "Fix flaky test"}],
                         prs=[{"url": "https://github.com/example/repo/pull/123", "description": "Add missing coverage"}])
        rendered = render(stream)
        for item, kind, number in ((stream.issues[0], "Issue", "42"), (stream.prs[0], "PR", "123")):
            self.assertIn(f"\x1b]8;;{item.url}\x1b\\{kind} #{number}: {item.description}\x1b]8;;\x1b\\", rendered)
        for index, item in enumerate((*stream.issues, *stream.prs), 1):
            self.assertIn(f"\n[{index}] \x1b[94;4m\x1b]8;;{item.url}", rendered)
        self.assertNotIn("Ctrl-click", rendered)

    def test_only_safe_links_get_shortcuts_and_only_single_digit_keys_are_advertised(self) -> None:
        stream = example(issues=[
            {"url": f"https://github.com/example/repo/issues/{i}", "description": f"Issue {i}"} for i in range(1, 11)
        ])
        stream = dataclasses.replace(stream, issues=(Link("file:///not-web", "Not actionable"), *stream.issues))
        rendered = render(stream)
        self.assertNotIn("[0]", rendered)
        self.assertNotIn("[10]", rendered)
        self.assertIn("[1] \x1b[94;4m\x1b]8;;https://github.com/example/repo/issues/1", rendered)
        self.assertIn("[9] \x1b[94;4m\x1b]8;;https://github.com/example/repo/issues/9", rendered)
        self.assertIn("\x1b]8;;https://github.com/example/repo/issues/10", rendered)
        self.assertIn("Not actionable", rendered)

    def test_short_pane_keeps_header_note_action_and_explicit_overflow(self) -> None:
        stream = example(prs=[
            {"url": f"https://github.com/example/repo/pull/{i}", "description": f"Change {i}"} for i in range(1, 7)
        ])
        for rows in (1, 2, 3, 4, 6, 20):
            with self.subTest(rows=rows):
                rendered = render(stream, rows=rows)
                self.assertLessEqual(len(rendered.splitlines()), rows)
                self.assertTrue(rendered.splitlines()[0].startswith("[n] \x1b[1mExample stream"))
                self.assertEqual(rendered.count("[n]"), 1)
        rendered = render(stream, rows=6)
        self.assertIn("[1]", rendered)
        self.assertIn("[2]", rendered)
        self.assertIn("[3]", rendered)
        self.assertNotIn("[4]", rendered)
        self.assertIn("3 more links", rendered)

    def test_title_shortcut_leaves_three_link_rows_in_five_row_pane(self) -> None:
        stream = example(prs=[
            {"url": f"https://github.com/example/repo/pull/{i}", "description": f"Change {i}"} for i in range(1, 4)
        ])
        rendered = render(stream, rows=5)
        self.assertEqual(len(rendered.splitlines()), 5)
        self.assertIn("[3]", rendered.splitlines()[-1])
        self.assertNotIn("more links", rendered)

    def test_render_reflects_updated_state(self) -> None:
        self.assertNotEqual(render(example()), render(example(state="Review complete.")))
        self.assertIn("Review complete.", render(example(state="Review complete.")))

    def test_note_uri_uses_current_vault_and_relative_path(self) -> None:
        stream = dataclasses.replace(example(), path=Path("/notes/scratch/a note #1.md"))
        parsed = urlsplit(note_uri(stream, NotesConfig(Path("/notes"), "My notes")))
        self.assertEqual((parsed.scheme, parsed.netloc), ("obsidian", "open"))
        self.assertEqual(parse_qs(parsed.query), {"vault": ["My notes"], "file": ["scratch/a note #1.md"]})

    def test_note_root_expands_home_and_rejects_external_notes(self) -> None:
        config = NotesConfig.parse({"notes": {"root": "~/notes"}, "obsidian": {"vault": "Notes"}})
        stream = dataclasses.replace(example(), path=Path.home() / "notes/scratch/example.md")
        self.assertEqual(parse_qs(urlsplit(note_uri(stream, config)).query)["file"], ["scratch/example.md"])
        with self.assertRaisesRegex(StreamError, "outside notes root"):
            note_uri(example(), NotesConfig(Path("/other"), "Notes"))

    def test_text_and_non_web_or_control_containing_urls_cannot_inject_controls(self) -> None:
        stream = example(title="Title\x1b\n\x07", state="State\r\x1b")
        for url in ("obsidian://open?vault=notes", "https://example.com/\x07bad", "https:///missing-host", "not a URL"):
            unsafe = dataclasses.replace(stream, issues=(Link(url, "Description\x1b\x07"),))
            rendered = re.sub(r"\x1b\[[0-9;]*m", "", render(unsafe))
            self.assertNotIn("\x1b", rendered)
            self.assertNotIn("\x07", rendered)
            self.assertNotIn("\r", rendered)
            self.assertIn("Title · active", rendered)

    def test_binding_reader_rejects_missing_malformed_or_empty_reference(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "binding.json"
            with self.assertRaises(StreamError):
                read_binding(path)
            for data in ("not json", "[]", '{"stream":null}', '{"stream":""}'):
                path.write_text(data)
                with self.subTest(data=data), self.assertRaises(StreamError):
                    read_binding(path)

    def test_configuration_decoder_fails_closed(self) -> None:
        for config in ({}, {"notes": {"root": "relative"}, "obsidian": {"vault": "Notes"}},
                       {"notes": {"root": "/notes"}, "obsidian": {"vault": None}}):
            with self.subTest(config=config), self.assertRaises(StreamError):
                NotesConfig.parse(config)


class ShortcutInputTest(unittest.TestCase):
    def test_standalone_shortcuts_are_dispatched(self) -> None:
        self.assertEqual(ShortcutInput().feed(b"x0n123456789"), list("n123456789"))

    def test_fragmented_terminal_sequences_never_dispatch_shortcuts(self) -> None:
        for sequence in (b"\x1b[1;5A", b"\x1b[15~", b"\x1bO1;5P", b"\x1b[110;1u",
                         b"\x1b[<0;15;19M", b"\x1b1", b"\x1bn", b"\x1b]0;title n123\x07",
                         b"\x1bPpayload n123\x1b\\", b"\x1b[200~n123\x1b[201~"):
            with self.subTest(sequence=sequence):
                decoder = ShortcutInput()
                for byte in sequence:
                    self.assertEqual(decoder.feed(bytes([byte])), [])
                self.assertEqual(decoder.feed(b"n"), ["n"])


class LinksRuntimeTest(unittest.TestCase):
    def setUp(self) -> None:
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        self.root = Path(temp.name)
        self.binding = self.root / "binding.json"
        write_binding(self.binding, "example")
        env = patch.dict(os.environ, {"XDG_CONFIG_HOME": str(self.root / "config")}, clear=True)
        env.start()
        self.addCleanup(env.stop)

    def test_note_action_refreshes_binding_stream_and_configuration_at_keypress(self) -> None:
        view = LinksView(self.binding)
        with patch.object(view.runtime, "stream", return_value=example()) as show, \
                patch.object(view.runtime, "config", return_value=NotesConfig(Path("/notes"), "Old")) as config, \
                patch("_streams.herdr_links.subprocess.run") as opener:
            view.refresh()
            write_binding(self.binding, "other")
            config.return_value = NotesConfig(Path("/notes"), "New vault")
            view.open_note()
        self.assertEqual(show.call_args_list[-1].args, ("other",))
        config.assert_called_once_with()
        uri = opener.call_args.args[0][1]
        self.assertEqual(opener.call_args.args[0][0], "open")
        self.assertEqual(parse_qs(urlsplit(uri).query)["vault"], ["New vault"])

    def test_numbered_actions_match_safe_issue_then_pr_order_and_refresh_at_keypress(self) -> None:
        issue = {"url": "https://github.com/example/repo/issues/1", "description": "Test issue"}
        pr = {"url": "https://github.com/example/repo/pull/2", "description": "Test PR"}
        stream = example(issues=[issue], prs=[pr])
        stream = dataclasses.replace(stream, issues=(Link("file:///unsafe", "Not a web link"), *stream.issues))
        view = LinksView(self.binding)
        with patch.object(view.runtime, "stream", return_value=stream) as show, \
                patch("_streams.herdr_links.subprocess.run") as opener:
            view.refresh()
            write_binding(self.binding, "other")
            view.open_link(1)
            self.assertEqual(show.call_args.args, ("other",))
            self.assertEqual(opener.call_args.args[0], ["open", issue["url"]])
            view.open_link(2)
            self.assertEqual(opener.call_args.args[0], ["open", pr["url"]])
            show.return_value = example(prs=[pr])
            opened_count = opener.call_count
            with self.assertRaisesRegex(StreamError, "Links changed"):
                view.open_link(1)
            self.assertEqual(opener.call_count, opened_count)
            view.open_link(1)
            self.assertEqual(opener.call_args.args[0], ["open", pr["url"]])

    def test_hidden_links_have_no_active_shortcuts(self) -> None:
        stream = example(prs=[
            {"url": f"https://github.com/example/repo/pull/{i}", "description": f"Change {i}"} for i in range(1, 7)
        ])
        view = LinksView(self.binding)
        view.rows = 6
        with patch.object(view.runtime, "stream", return_value=stream), \
                patch("_streams.herdr_links.subprocess.run") as opener:
            view.refresh()
            with self.assertRaisesRegex(StreamError, "No link assigned"):
                view.open_link(4)
            opener.assert_not_called()
            view.open_link(3)
            self.assertEqual(opener.call_args.args[0], ["open", stream.prs[2].url])

    def test_missing_or_unsafe_numbered_links_never_open(self) -> None:
        stream = dataclasses.replace(example(), issues=(Link("https://example.com/\x07bad", "Unsafe link"),))
        view = LinksView(self.binding)
        with patch.object(view.runtime, "stream", return_value=stream), \
                patch("_streams.herdr_links.subprocess.run") as opener:
            for index in (0, 1, 9, 10):
                with self.subTest(index=index), self.assertRaisesRegex(StreamError, "No link assigned"):
                    view.open_link(index)
            opener.assert_not_called()

    def test_failed_link_opener_retains_valid_view(self) -> None:
        view = LinksView(self.binding)
        stream = example(prs=[{"url": "https://github.com/example/repo/pull/1", "description": "Test PR"}])
        with patch.object(view.runtime, "stream", return_value=stream), \
                patch("_streams.herdr_links.subprocess.run", side_effect=subprocess.CalledProcessError(2, ["open"])):
            view.refresh()
            with self.assertRaisesRegex(StreamError, "Open link failed"):
                view.open_link(1)
            self.assertIsNotNone(view.stream)

    def test_failed_binding_or_note_refresh_disables_stale_opener(self) -> None:
        for failure in ("binding", "note"):
            with self.subTest(failure=failure):
                write_binding(self.binding, "example")
                view = LinksView(self.binding)
                with patch.object(view.runtime, "stream", return_value=example()) as show, \
                        patch("_streams.herdr_links.subprocess.run") as opener:
                    view.refresh()
                    self.assertIsNotNone(view.stream)
                    if failure == "binding":
                        self.binding.write_text("malformed")
                    else:
                        show.side_effect = StreamError("Note is unavailable")
                    self.assertIn("Stream unavailable:", view.refresh())
                    self.assertIsNone(view.stream)
                    with self.assertRaisesRegex(StreamError, "unavailable"):
                        view.open_note()
                    with self.assertRaisesRegex(StreamError, "unavailable"):
                        view.open_link(1)
                    opener.assert_not_called()

    def test_failed_opener_or_config_does_not_discard_a_valid_view(self) -> None:
        view = LinksView(self.binding)
        with patch.object(view.runtime, "stream", return_value=example()), \
                patch.object(view.runtime, "config", return_value=NotesConfig(Path("/notes"), "Notes")) as config, \
                patch("_streams.herdr_links.subprocess.run", side_effect=subprocess.CalledProcessError(2, ["open"])):
            with self.assertRaisesRegex(StreamError, "Open note failed"):
                view.open_note()
            self.assertIsNotNone(view.stream)
            config.side_effect = StreamError("Config is unavailable")
            with self.assertRaisesRegex(StreamError, "Config is unavailable"):
                view.open_note()
            self.assertIsNotNone(view.stream)


class PackagedViewTest(unittest.TestCase):
    def test_static_manifest_launches_packaged_module(self) -> None:
        plugin = SRC / "_streams" / "herdr_plugin"
        manifest = tomllib.loads((plugin / "herdr-plugin.toml").read_text())
        self.assertEqual(manifest["id"], "streams.links")
        self.assertEqual(manifest["min_herdr_version"], "0.9.3")
        self.assertEqual(manifest["panes"][0]["command"], ["/bin/sh", "launch.sh"])
        self.assertEqual(manifest["panes"][0]["placement"], "split")

    def test_generic_cli_does_not_import_the_view_or_unix_terminal_modules(self) -> None:
        result = subprocess.run(
            [sys.executable, "-P", "-c", "import sys; import _streams.cli; assert '_streams.herdr_links' not in sys.modules; assert 'termios' not in sys.modules; assert 'tty' not in sys.modules"],
            env={"PYTHONPATH": str(SRC)}, check=True, capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.stdout, "")

    def test_launcher_refuses_missing_runtime_instead_of_guessing_python(self) -> None:
        plugin = SRC / "_streams" / "herdr_plugin"
        result = subprocess.run(["/bin/sh", str(plugin / "launch.sh")], env={}, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("STREAMS_PYTHON", result.stderr)

    def test_manifest_argv_resolves_launcher_from_native_plugin_cwd(self) -> None:
        import_root = Path(os.environ.get("STREAMS_TEST_IMPORT_ROOT", str(SRC)))
        interpreter = os.environ.get("STREAMS_TEST_PYTHON", sys.executable)
        plugin = import_root / "_streams" / "herdr_plugin"
        manifest = tomllib.loads((plugin / "herdr-plugin.toml").read_text())
        result = subprocess.run(
            manifest["panes"][0]["command"], cwd=plugin,
            env={"STREAMS_PYTHON": interpreter, "STREAMS_IMPORT_ROOT": str(import_root), "STREAMS_BINDING": "/absent.json",
                 "PYTHONPATH": "/misleading", "PYTHONHOME": "/misleading"}, capture_output=True, text=True, timeout=10,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("requires an interactive terminal", result.stderr)


if __name__ == "__main__":
    unittest.main()
