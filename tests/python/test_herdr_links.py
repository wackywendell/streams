from __future__ import annotations

import dataclasses
import json
import os
import pty
import re
import select
import signal
import struct
import subprocess
import sys
import tempfile
import time
import tomllib
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit

SRC = Path(__file__).resolve().parents[2] / "src"
sys.path.insert(0, str(SRC))

from _streams.errors import StreamError
from _streams.frontmatter import render_frontmatter
from _streams.herdr_binding import read_binding, write_binding
from _streams.herdr_links import LinksView, NotesConfig, ShortcutInput, StreamsRuntime, note_uri, render
from _streams.models import Link, Stream
from _streams.store import rename_stream


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

    def test_cli_queries_pin_interpreter_and_import_root_without_global_streams(self) -> None:
        result = subprocess.CompletedProcess([], 0, json.dumps(example().as_json()), "")
        with patch.dict(os.environ, {"PYTHONPATH": "/wrong/package", "PYTHONHOME": "/wrong/home"}), \
                patch("_streams.herdr_links.subprocess.run", return_value=result) as run:
            runtime = StreamsRuntime()
            self.assertEqual(runtime.stream("example").title, "Example stream")
            args, kwargs = run.call_args
        self.assertEqual(args[0], [sys.executable, "-P", "-m", "_streams.cli", "show", "example", "--exact", "--json"])
        self.assertEqual(Path(kwargs["env"]["PYTHONPATH"]), SRC.resolve())
        self.assertNotIn("PYTHONHOME", kwargs["env"])
        self.assertEqual(kwargs["timeout"], 10)

    def test_cli_failure_invalid_json_and_invalid_stream_are_expected_errors(self) -> None:
        for failure in (subprocess.CalledProcessError(2, [], stderr="bad note"), FileNotFoundError(),
                        subprocess.TimeoutExpired([], 10)):
            with self.subTest(failure=failure), patch("_streams.herdr_links.subprocess.run", side_effect=failure), \
                    self.assertRaises(StreamError):
                StreamsRuntime().stream("example")
        for output in ("not json", "[]", "{}", json.dumps({"error": {"message": "bad note"}})):
            with self.subTest(output=output), patch("_streams.herdr_links.subprocess.run", return_value=subprocess.CompletedProcess([], 0, output, "")), \
                    self.assertRaises(StreamError):
                StreamsRuntime().stream("example")

    def test_rebinding_and_renames_are_read_fresh(self) -> None:
        notes = self.root / "notes"
        scratch = notes / "scratch"
        scratch.mkdir(parents=True)
        stream = example(scratch)
        stream.path.write_text(render_frontmatter(stream.frontmatter, stream.body))
        from _streams.config import set_config
        set_config("notes.root", str(notes))
        view = LinksView(self.binding)
        self.assertIn("Example stream", view.refresh())
        renamed = rename_stream(stream, "Renamed stream")
        self.assertIn("Renamed stream", view.refresh())
        self.assertEqual(view.stream.path, renamed.path)
        renamed = rename_stream(renamed, "Renamed again")
        self.assertIn("Renamed again", view.refresh())
        self.assertEqual(view.stream.path, renamed.path)
        other = dataclasses.replace(example(scratch, title="Other stream"), path=scratch / "other.md")
        other.path.write_text(render_frontmatter(other.frontmatter, other.body))
        write_binding(self.binding, "other")
        self.assertIn("Other stream", view.refresh())
        self.assertEqual(view.stream.slug, "other")

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

    def test_live_view_refresh_opener_failure_and_pinned_runtime_over_pty(self) -> None:
        # Set these only when verifying the wheel in a disposable environment.
        import_root = Path(os.environ.get("STREAMS_TEST_IMPORT_ROOT", str(SRC)))
        interpreter = os.environ.get("STREAMS_TEST_PYTHON", sys.executable)
        launcher = import_root / "_streams" / "herdr_plugin" / "launch.sh"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            notes = root / "notes"
            scratch = notes / "scratch"
            scratch.mkdir(parents=True)
            config = root / "config" / "streams" / "config.json"
            config.parent.mkdir(parents=True)
            config.write_text(json.dumps({"notes": {"root": str(notes)}, "obsidian": {"vault": "Notes"}}))
            stream = example(scratch)
            stream.path.write_text(render_frontmatter(stream.frontmatter, stream.body))
            binding = root / "binding.json"
            write_binding(binding, stream.slug)
            evil = root / "_streams"
            evil.mkdir()
            (evil / "__init__.py").write_text("raise RuntimeError('wrong import root')\n")
            bin = root / "bin"
            bin.mkdir()
            for name, source in {
                "streams": "from pathlib import Path\nPath('wrong-streams').touch()\nraise SystemExit(7)\n",
                "open": "import json,sys\nfrom pathlib import Path\nif Path('fail-open').exists(): raise SystemExit(2)\nPath('opened.json').write_text(json.dumps(sys.argv[1:]))\nwith Path('opened.jsonl').open('a') as log: log.write(json.dumps(sys.argv[1:]) + '\\n')\n",
            }.items():
                path = bin / name
                path.write_text(f"#!{sys.executable}\n" + source)
                path.chmod(0o755)
            master, slave = pty.openpty()
            import fcntl
            import termios
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 6, 80, 0, 0))
            process = subprocess.Popen(
                ["/bin/sh", str(launcher)], stdin=slave, stdout=slave, stderr=slave, cwd=root,
                env={"PATH": str(bin), "HOME": str(root), "XDG_CONFIG_HOME": str(root / "config"),
                     "STREAMS_PYTHON": interpreter, "STREAMS_IMPORT_ROOT": str(import_root), "STREAMS_BINDING": str(binding),
                     "PYTHONPATH": str(root), "PYTHONHOME": str(root / "wrong-python")},
            )
            # Keep a slave handle open: macOS can discard unread final output
            # when the child's last slave handle closes.
            output = b""

            def await_condition(condition) -> None:
                nonlocal output
                deadline = time.monotonic() + 7
                while time.monotonic() < deadline:
                    if condition():
                        return
                    if process.poll() is not None:
                        self.fail(f"View exited early: {output!r}")
                    ready, _, _ = select.select([master], [], [], 0.05)
                    if ready:
                        try:
                            output += os.read(master, 65536)
                        except OSError:
                            self.fail(f"View closed PTY early: {output!r}")
                self.fail(f"Timed out waiting for view: {output!r}")

            try:
                await_condition(lambda: b"No linked issues or PRs." in output)
                self.assertIn(b"\x1b[?1049h\x1b[?25l\x1b[?7l", output)
                os.write(master, b"n")
                await_condition(lambda: (root / "opened.json").exists())
                uri = json.loads((root / "opened.json").read_text())[0]
                self.assertEqual(parse_qs(urlsplit(uri).query), {"vault": ["Notes"], "file": ["scratch/example.md"]})
                stream.path.write_text(render_frontmatter({**stream.frontmatter, "state": "State changed while pane is open."}, ""))
                await_condition(lambda: b"State changed while pane is open." in output)
                issue_url = "https://github.com/example/repo/issues/42"
                stream.path.write_text(render_frontmatter({**stream.frontmatter, "issues": [
                    {"url": issue_url, "description": "Test numbered action"},
                ]}, ""))
                await_condition(lambda: b"Test numbered action" in output)
                os.write(master, b"1")
                await_condition(lambda: json.loads((root / "opened.json").read_text()) == [issue_url])
                event_log = root / "opened.jsonl"
                await_condition(lambda: len(event_log.read_text().splitlines()) == 2)
                os.write(master, b"\x1b[1;5A\x1b[15~\x1bO1;5P\x1b[200~n123\x1b[201~xn")
                await_condition(lambda: len(event_log.read_text().splitlines()) >= 3)
                self.assertEqual(len(event_log.read_text().splitlines()), 3)
                self.assertEqual(json.loads(event_log.read_text().splitlines()[-1]), [uri])
                prs = [
                    {"url": f"https://github.com/example/repo/pull/{i}", "description": f"Change {i}"} for i in range(1, 7)
                ]
                stream.path.write_text(render_frontmatter({**stream.frontmatter, "prs": prs}, ""))
                await_condition(lambda: b"3 more links" in output)
                frame = output.rsplit(b"\x1b[2J\x1b[H", 1)[-1]
                self.assertLessEqual(len(frame.splitlines()), 6)
                self.assertIn(b"Example stream", frame)
                self.assertIn(b"[n]", frame)
                prs[1] = {"url": "https://github.com/example/repo/pull/99", "description": "Changed target"}
                stream.path.write_text(render_frontmatter({**stream.frontmatter, "prs": prs}, ""))
                os.write(master, b"12")
                await_condition(lambda: b"Links changed; press the key again" in output)
                self.assertEqual(len(event_log.read_text().splitlines()), 4)
                self.assertEqual(json.loads(event_log.read_text().splitlines()[-1]), [prs[0]["url"]])
                (root / "fail-open").touch()
                os.write(master, b"n")
                await_condition(lambda: b"Open note failed:" in output)
                self.assertIsNone(process.poll())
                self.assertFalse((root / "wrong-streams").exists())
                process.send_signal(signal.SIGINT)
                process.wait(timeout=5)
                self.assertEqual(process.returncode, 0)
                # Drain final terminal restoration without waiting for another refresh.
                deadline = time.monotonic() + 1
                while time.monotonic() < deadline and select.select([master], [], [], 0.1)[0]:
                    try:
                        chunk = os.read(master, 65536)
                    except OSError:
                        break
                    if not chunk:
                        break
                    output += chunk
                self.assertIn(b"\x1b[?7h\x1b[?25h\x1b[?1049l", output)
            finally:
                if process.poll() is None:
                    process.terminate()
                    process.wait(timeout=5)
                os.close(slave)
                os.close(master)


if __name__ == "__main__":
    unittest.main()
